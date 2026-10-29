//! Native Windows ConPTY backend.
//!
//! Why not portable-pty: it never sets `PSEUDOCONSOLE_PASSTHROUGH_MODE`, and
//! its sideload logic is a bare `LoadLibrary("conpty.dll")` that depends on the
//! DLL search path. Stock Windows 10 ConPTY (conhost 10.0.19041) strips APC
//! sequences (kitty graphics) and swallows `CSI ? u`; the ConPTY shipped in the
//! `Microsoft.Windows.Console.ConPTY` NuGet package (`conpty.dll` plus
//! `OpenConsole.exe`) does not. This module therefore:
//!
//! 1. Loads `CreatePseudoConsole` / `ResizePseudoConsole` / `ClosePseudoConsole`
//!    dynamically, preferring `conpty.dll` in the running executable's
//!    directory over the in-box `kernel32.dll`, and logs which one it got.
//! 2. Creates the pseudoconsole itself, optionally in passthrough mode (0x8 is
//!    passthrough only on the in-box ConPTY; see `flags`).
//! 3. Starts the child with a `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` attribute.
//!
//! # Handle ownership
//!
//! Every `HANDLE` lives in an [`OwnedHandle`] (closed exactly once on drop).
//! The pty-side pipe ends are closed right after `CreatePseudoConsole` (it
//! duplicates them). Our ends move to their users: the output read end to the
//! reader thread, the input write end to the writer thread, the process handle
//! (shared by `Arc`) to the wait thread and the killer.
//!
//! # Close order
//!
//! Per Microsoft's `ClosePseudoConsole` docs, the pseudoconsole is closed
//! *first* (kill the child, then close the pty), while our reader thread keeps
//! draining the output pipe: before Windows 11 24H2 `ClosePseudoConsole` blocks
//! until conhost has flushed its output, so closing the pipes first (or
//! waiting on the process first) is the classic hang. For the same reason the
//! call is made from a short-lived helper thread so a stalled close can never
//! block the GUI thread.

use std::ffi::{c_void, OsStr, OsString};
use std::io::{self, Read, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::{mem, ptr, thread};

use windows_sys::Win32::Foundation::{
    CloseHandle, FreeLibrary, HANDLE, HMODULE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows_sys::Win32::System::Console::{COORD, HPCON};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
    LOAD_WITH_ALTERED_SEARCH_PATH,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    InitializeProcThreadAttributeList, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, INFINITE,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
    STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

use super::cmdline::{command_line, resolve_program, EnvBlock};
use crate::{default_env, default_shell, ChildWait, Killer, Master, SpawnConfig, Spawned};

use super::flags::{pseudoconsole_flags, BASE_FLAGS};

const ERROR_BROKEN_PIPE: i32 = 109;
const ERROR_INSUFFICIENT_BUFFER: i32 = 122;

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().collect()
}

fn wide_nul(s: &OsStr) -> io::Result<Vec<u16>> {
    let mut v = wide(s);
    if v.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "string contains a NUL",
        ));
    }
    v.push(0);
    Ok(v)
}

// ---------------------------------------------------------------------------
// Handles
// ---------------------------------------------------------------------------

/// A Win32 `HANDLE` closed exactly once, on drop.
struct OwnedHandle(HANDLE);

// SAFETY: a kernel handle value may be used and closed from any thread.
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl OwnedHandle {
    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            // SAFETY: we own the handle and this is the only close.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// An anonymous pipe: `(read end, write end)`. Not inheritable (we pass
/// `bInheritHandles = FALSE` to `CreateProcessW`; the child gets its console
/// through the pseudoconsole attribute).
fn create_pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let mut read: HANDLE = ptr::null_mut();
    let mut write: HANDLE = ptr::null_mut();
    // SAFETY: both out-pointers are valid for writes.
    if unsafe { CreatePipe(&mut read, &mut write, ptr::null(), 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((OwnedHandle(read), OwnedHandle(write)))
}

/// Blocking reader over the pseudoconsole's output pipe.
struct PipeReader(OwnedHandle);

impl Read for PipeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(u32::MAX as usize) as u32;
        let mut n: u32 = 0;
        // SAFETY: `buf` is valid for `len` bytes; `n` for writes; no OVERLAPPED
        // because the pipe is synchronous.
        let ok = unsafe { ReadFile(self.0.raw(), buf.as_mut_ptr(), len, &mut n, ptr::null_mut()) };
        if ok == 0 {
            let e = io::Error::last_os_error();
            // The write end closing is EOF, not a failure.
            return if e.raw_os_error() == Some(ERROR_BROKEN_PIPE) {
                Ok(0)
            } else {
                Err(e)
            };
        }
        Ok(n as usize)
    }
}

/// Blocking writer over the pseudoconsole's input pipe.
struct PipeWriter(OwnedHandle);

impl Write for PipeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(u32::MAX as usize) as u32;
        let mut n: u32 = 0;
        // SAFETY: `buf` is valid for `len` bytes; `n` for writes.
        let ok = unsafe { WriteFile(self.0.raw(), buf.as_ptr(), len, &mut n, ptr::null_mut()) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ConPTY entry points
// ---------------------------------------------------------------------------

type CreatePseudoConsoleFn =
    unsafe extern "system" fn(COORD, HANDLE, HANDLE, u32, *mut HPCON) -> i32;
type ResizePseudoConsoleFn = unsafe extern "system" fn(HPCON, COORD) -> i32;
type ClosePseudoConsoleFn = unsafe extern "system" fn(HPCON);
type RawProc = unsafe extern "system" fn() -> isize;

/// The three ConPTY functions from whichever library supplied them. The module
/// is intentionally never unloaded (the pointers live for the process).
struct Api {
    create: CreatePseudoConsoleFn,
    resize: ResizePseudoConsoleFn,
    close: ClosePseudoConsoleFn,
    /// True for a sideloaded `conpty.dll`, false for the in-box kernel32 one.
    sideloaded: bool,
}

static API: OnceLock<Result<Api, String>> = OnceLock::new();

fn api() -> io::Result<&'static Api> {
    API.get_or_init(load_api)
        .as_ref()
        .map_err(|e| io::Error::new(io::ErrorKind::Unsupported, e.clone()))
}

fn load_api() -> Result<Api, String> {
    // Prefer a sideloaded conpty.dll (with OpenConsole.exe) next to our exe.
    match exe_dir() {
        Some(dir) => {
            let dll = dir.join("conpty.dll");
            if dll.is_file() {
                match load_from(&dll, LOAD_WITH_ALTERED_SEARCH_PATH) {
                    Ok(api) => {
                        if !dir.join("OpenConsole.exe").is_file() {
                            log::warn!(
                                "ConPTY: conpty.dll loaded but OpenConsole.exe is missing next to it ({}); \
                                 pseudoconsole creation will likely fail",
                                dir.display()
                            );
                        }
                        return Ok(api);
                    }
                    Err(e) => log::warn!(
                        "ConPTY: could not use sideloaded {}: {e}; falling back to kernel32.dll",
                        dll.display()
                    ),
                }
            } else {
                log::debug!(
                    "ConPTY: no conpty.dll next to the executable ({})",
                    dir.display()
                );
            }
        }
        None => log::debug!("ConPTY: cannot determine the executable directory"),
    }
    load_from(Path::new("kernel32.dll"), LOAD_LIBRARY_SEARCH_SYSTEM32).map_err(|e| {
        format!("this system does not support ConPTY (Windows 10 1809 or newer is required): {e}")
    })
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

/// Load `path` and resolve the three entry points. Logs the library actually
/// loaded at info level.
fn load_from(path: &Path, flags: u32) -> io::Result<Api> {
    let wpath = wide_nul(path.as_os_str())?;
    // SAFETY: `wpath` is NUL-terminated; the file handle must be NULL.
    let module = unsafe { LoadLibraryExW(wpath.as_ptr(), ptr::null_mut(), flags) };
    if module.is_null() {
        return Err(io::Error::last_os_error());
    }
    let sideloaded = path
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("conpty.dll"));
    match resolve(module, sideloaded) {
        Ok(api) => {
            let loaded = module_path(module).unwrap_or_else(|| path.to_path_buf());
            let kind = if sideloaded {
                "sideloaded conpty.dll"
            } else {
                "in-box ConPTY"
            };
            log::info!("ConPTY: using {kind} from {}", loaded.display());
            Ok(api)
        }
        Err(e) => {
            // SAFETY: `module` came from LoadLibraryExW above and nothing
            // retained pointers into it (resolve failed).
            unsafe { FreeLibrary(module) };
            Err(e)
        }
    }
}

fn sym(module: HMODULE, name: &'static [u8]) -> io::Result<RawProc> {
    debug_assert!(name.ends_with(&[0]));
    // SAFETY: `name` is a NUL-terminated ASCII string.
    unsafe { GetProcAddress(module, name.as_ptr()) }.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "{} is not exported: {}",
                String::from_utf8_lossy(&name[..name.len() - 1]),
                io::Error::last_os_error()
            ),
        )
    })
}

fn resolve(module: HMODULE, sideloaded: bool) -> io::Result<Api> {
    let create = sym(module, b"CreatePseudoConsole\0")?;
    let resize = sym(module, b"ResizePseudoConsole\0")?;
    let close = sym(module, b"ClosePseudoConsole\0")?;
    // SAFETY: these exports have exactly these signatures (wincon.h).
    unsafe {
        Ok(Api {
            create: mem::transmute::<RawProc, CreatePseudoConsoleFn>(create),
            resize: mem::transmute::<RawProc, ResizePseudoConsoleFn>(resize),
            close: mem::transmute::<RawProc, ClosePseudoConsoleFn>(close),
            sideloaded,
        })
    }
}

fn module_path(module: HMODULE) -> Option<PathBuf> {
    let mut len = 512usize;
    while len <= 32 * 1024 {
        let mut buf = vec![0u16; len];
        // SAFETY: `buf` is valid for `len` u16s.
        let n = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), len as u32) } as usize;
        if n == 0 {
            return None;
        }
        if n < len {
            return Some(PathBuf::from(OsString::from_wide(&buf[..n])));
        }
        len *= 2; // truncated; retry with a bigger buffer
    }
    None
}

fn hresult_error(what: &str, hr: i32) -> io::Error {
    io::Error::other(format!("{what} failed: HRESULT 0x{:08X}", hr as u32))
}

fn coord(cols: u16, rows: u16) -> COORD {
    COORD {
        X: cols.clamp(1, i16::MAX as u16) as i16,
        Y: rows.clamp(1, i16::MAX as u16) as i16,
    }
}

// ---------------------------------------------------------------------------
// Pseudoconsole
// ---------------------------------------------------------------------------

/// An `HPCON`. Dropping it closes the pseudoconsole (see the module docs on
/// close order).
struct PseudoConsole {
    hpc: HPCON,
    api: &'static Api,
}

impl PseudoConsole {
    fn create(
        api: &'static Api,
        cols: u16,
        rows: u16,
        input_read: &OwnedHandle,
        output_write: &OwnedHandle,
        flags: u32,
    ) -> io::Result<Self> {
        let mut hpc: HPCON = 0;
        // SAFETY: handles are valid pipe ends; `hpc` is valid for writes.
        let hr = unsafe {
            (api.create)(
                coord(cols, rows),
                input_read.raw(),
                output_write.raw(),
                flags,
                &mut hpc,
            )
        };
        if hr < 0 {
            return Err(hresult_error("CreatePseudoConsole", hr));
        }
        Ok(PseudoConsole { hpc, api })
    }

    fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        // SAFETY: `self.hpc` is a live pseudoconsole (closed only in Drop).
        let hr = unsafe { (self.api.resize)(self.hpc, coord(cols, rows)) };
        if hr < 0 {
            return Err(hresult_error("ResizePseudoConsole", hr));
        }
        Ok(())
    }
}

impl Drop for PseudoConsole {
    fn drop(&mut self) {
        let hpc = self.hpc;
        let close = self.api.close;
        // Closing can block until conhost has flushed output (pre-24H2), and
        // the reader thread is what drains it, so never do it on a thread the
        // caller cares about. If no thread can be created, close inline: the
        // reader is still running, so it cannot deadlock.
        let spawned = thread::Builder::new()
            .name("grimalkin-conpty-close".into())
            .spawn(move || {
                // SAFETY: `hpc` is closed exactly once, here.
                unsafe { close(hpc) };
            });
        if spawned.is_err() {
            // SAFETY: as above; the failed spawn never ran the closure.
            unsafe { close(hpc) };
        }
    }
}

struct ConPtyMaster(PseudoConsole);

impl Master for ConPtyMaster {
    fn resize(&self, cols: u16, rows: u16, _pixel_w: u16, _pixel_h: u16) -> io::Result<()> {
        // ConPTY has no notion of pixel sizes.
        self.0.resize(cols, rows)
    }
}

// ---------------------------------------------------------------------------
// Attribute list
// ---------------------------------------------------------------------------

/// A `PROC_THREAD_ATTRIBUTE_LIST` with one attribute: the pseudoconsole.
struct AttributeList {
    // u64 storage guarantees the pointer alignment the API expects.
    buf: Vec<u64>,
}

impl AttributeList {
    fn with_pseudoconsole(hpc: HPCON) -> io::Result<Self> {
        let mut size: usize = 0;
        // The sizing call is expected to fail with ERROR_INSUFFICIENT_BUFFER.
        // SAFETY: NULL list + valid size out-pointer is the documented probe.
        let probe = unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size) };
        if probe == 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER) {
                return Err(e);
            }
        }
        if size == 0 {
            return Err(io::Error::other(
                "InitializeProcThreadAttributeList reported size 0",
            ));
        }
        let mut buf = vec![0u64; size.div_ceil(mem::size_of::<u64>())];
        let list_ptr: LPPROC_THREAD_ATTRIBUTE_LIST = buf.as_mut_ptr().cast();
        // SAFETY: `buf` has at least `size` bytes and 8-byte alignment.
        if unsafe { InitializeProcThreadAttributeList(list_ptr, 1, 0, &mut size) } == 0 {
            // Not initialized: the Vec just frees its memory.
            return Err(io::Error::last_os_error());
        }
        // From here the list is initialized, so Drop must delete it.
        let mut list = AttributeList { buf };
        // For PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE the value *is* the HPCON
        // (not a pointer to it) and its size is sizeof(HPCON).
        // SAFETY: the list is initialized; `hpc` stays valid until after
        // CreateProcessW (the caller owns the PseudoConsole).
        let ok = unsafe {
            UpdateProcThreadAttribute(
                list.as_mut_ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                hpc as usize as *const c_void,
                mem::size_of::<HPCON>(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error()); // Drop deletes the list
        }
        Ok(list)
    }

    fn as_mut_ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.buf.as_mut_ptr().cast()
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        // SAFETY: a constructed AttributeList is always initialized.
        unsafe { DeleteProcThreadAttributeList(self.as_mut_ptr()) };
    }
}

// ---------------------------------------------------------------------------
// Child process
// ---------------------------------------------------------------------------

const STILL_ACTIVE: u32 = 259;

struct WinChild(Arc<OwnedHandle>);

impl ChildWait for WinChild {
    fn wait(&mut self) -> Option<u32> {
        // SAFETY: the handle is a live process handle owned by `self.0`.
        if unsafe { WaitForSingleObject(self.0.raw(), INFINITE) } != WAIT_OBJECT_0 {
            return None;
        }
        exit_code(&self.0)
    }
}

fn exit_code(proc: &OwnedHandle) -> Option<u32> {
    let mut code: u32 = 0;
    // SAFETY: live process handle; `code` valid for writes.
    if unsafe { GetExitCodeProcess(proc.raw(), &mut code) } == 0 {
        return None;
    }
    Some(code)
}

struct WinKiller(Arc<OwnedHandle>);

impl Killer for WinKiller {
    fn kill(&mut self) -> io::Result<()> {
        // SAFETY: live process handle (the Arc keeps it open).
        if unsafe { TerminateProcess(self.0.raw(), 1) } != 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        // Terminating an already-exited process fails with ACCESS_DENIED;
        // that is not an error for us.
        match exit_code(&self.0) {
            Some(code) if code != STILL_ACTIVE => Ok(()),
            _ => Err(err),
        }
    }
}

/// Build the child's environment: this process's, then grimalkin's defaults,
/// then the caller's overrides.
fn build_env(cfg: &SpawnConfig) -> EnvBlock {
    let mut env = EnvBlock::default();
    for (k, v) in std::env::vars_os() {
        env.set(wide(&k), wide(&v));
    }
    for (k, v) in default_env() {
        env.set(wide(&k), wide(&v));
    }
    for (k, v) in &cfg.env {
        env.set(wide(OsStr::new(k)), wide(OsStr::new(v)));
    }
    env
}

pub(crate) fn open(cfg: &SpawnConfig) -> io::Result<Spawned> {
    let api = api()?;
    let cols = cfg.cols.max(1);
    let rows = cfg.rows.max(1);

    // Command line, program and environment first: failures here need no
    // cleanup.
    let program = if cfg.cmd.is_empty() {
        default_shell()
    } else {
        PathBuf::from(&cfg.cmd)
    };
    let env = build_env(cfg);
    let path_var = env.get("PATH").map(OsString::from_wide);
    let resolved = resolve_program(&program, cfg.cwd.as_deref(), path_var.as_deref(), |p| {
        p.is_file()
    });
    let program_str = resolved
        .as_deref()
        .unwrap_or(&program)
        .to_string_lossy()
        .into_owned();
    let mut cmdline = wide_nul(OsStr::new(&command_line(&program_str, &cfg.args)?))?;
    let app_name = match &resolved {
        Some(p) => Some(wide_nul(p.as_os_str())?),
        None => None,
    };
    let cwd = match &cfg.cwd {
        Some(c) => Some(wide_nul(c.as_os_str())?),
        None => None,
    };
    let env_block = env.to_block();

    // Pipes. The `*_pty` ends belong to the pseudoconsole once it exists.
    let (input_read_pty, input_write) = create_pipe()?;
    let (output_read, output_write_pty) = create_pipe()?;

    // Bit 0x8 is passthrough only on the in-box ConPTY; on the redistributable
    // it is GLYPH_WIDTH_GRAPHEMES, so never send it there (see `flags`).
    let passthrough = cfg.passthrough && !api.sideloaded;
    if cfg.passthrough && api.sideloaded {
        log::info!(
            "ConPTY: passthrough is not applicable to the sideloaded ConPTY (it already \
             forwards unknown sequences); not setting flag 0x8"
        );
    }
    let flags = pseudoconsole_flags(api.sideloaded, cfg.passthrough);
    let con = match PseudoConsole::create(
        api,
        cols,
        rows,
        &input_read_pty,
        &output_write_pty,
        flags,
    ) {
        Ok(c) => {
            if passthrough {
                log::info!("ConPTY: pseudoconsole created in passthrough mode");
            }
            c
        }
        Err(e) if passthrough => {
            log::warn!(
                "ConPTY: CreatePseudoConsole rejected passthrough mode ({e}); retrying without \
                 it. This in-box ConPTY is too old: sideload conpty.dll + OpenConsole.exe."
            );
            PseudoConsole::create(
                api,
                cols,
                rows,
                &input_read_pty,
                &output_write_pty,
                BASE_FLAGS,
            )?
        }
        Err(e) => return Err(e),
    };
    // The pseudoconsole duplicated its ends; close ours now so our reader sees
    // EOF once conhost goes away.
    drop(input_read_pty);
    drop(output_write_pty);

    let mut attrs = AttributeList::with_pseudoconsole(con.hpc)?;

    // SAFETY: STARTUPINFOEXW is plain data; all-zero is a valid initial state.
    let mut si: STARTUPINFOEXW = unsafe { mem::zeroed() };
    si.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;
    // Don't let the child inherit our (possibly redirected) std handles.
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    si.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
    si.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
    si.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
    si.lpAttributeList = attrs.as_mut_ptr();

    // SAFETY: PROCESS_INFORMATION is plain data.
    let mut pi: PROCESS_INFORMATION = unsafe { mem::zeroed() };
    // SAFETY: every pointer is either NULL or points to a NUL-terminated
    // buffer / valid struct that outlives the call; `cmdline` is mutable as
    // CreateProcessW requires; `env_block` is a double-NUL-terminated UTF-16
    // block matching CREATE_UNICODE_ENVIRONMENT. bInheritHandles is FALSE.
    let ok = unsafe {
        CreateProcessW(
            app_name.as_ref().map_or(ptr::null(), |v| v.as_ptr()),
            cmdline.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
            env_block.as_ptr().cast(),
            cwd.as_ref().map_or(ptr::null(), |v| v.as_ptr()),
            &si.StartupInfo,
            &mut pi,
        )
    };
    if ok == 0 {
        let e = io::Error::last_os_error();
        log::error!(
            "ConPTY: CreateProcessW({program_str:?}, cwd {:?}) failed: {e}",
            cfg.cwd
        );
        return Err(io::Error::new(
            e.kind(),
            format!("cannot start {program_str:?}: {e}"),
        ));
    }
    drop(attrs);
    // We never use the primary thread handle.
    drop(OwnedHandle(pi.hThread));
    let process = Arc::new(OwnedHandle(pi.hProcess));

    Ok(Spawned {
        master: Box::new(ConPtyMaster(con)),
        reader: Box::new(PipeReader(output_read)),
        writer: Box::new(PipeWriter(input_write)),
        child: Box::new(WinChild(process.clone())),
        killer: Box::new(WinKiller(process)),
        pid: Some(pi.dwProcessId),
    })
}
