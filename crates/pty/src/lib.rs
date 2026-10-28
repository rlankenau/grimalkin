//! grimalkin-pty: a thin PTY layer (a native ConPTY backend on Windows, a
//! [`portable_pty`] `forkpty` backend on Unix) that gives the GUI everything it
//! needs and nothing else:
//!
//! * [`Pty::spawn`] / [`Pty::spawn_with`] start a shell (or any command) on a
//!   fresh pseudo-terminal of a given size.
//! * A reader thread forwards child output as [`PtyEvent::Output`] chunks (at
//!   most 64 KiB each) over a [`std::sync::mpsc::Sender`], followed by exactly
//!   one [`PtyEvent::Exited`] when the child has gone away and its output has
//!   been drained.
//! * [`PtyWriter`] is a cloneable, thread-safe handle for sending input.
//! * [`Pty::resize`], [`Pty::kill`], [`Pty::try_wait`] do what they say.
//! * An optional `on_wake` callback is invoked after every event is sent so a
//!   GUI can wake its event loop (e.g. a winit `EventLoopProxy`).
//!
//! # Environment
//!
//! Every child gets `COLORTERM=truecolor`, `TERM_PROGRAM=grimalkin` and
//! `TERM_PROGRAM_VERSION=<crate version>`. On Unix it also gets
//! `TERM=xterm-kitty` so kitty-aware tools (`kitten icat`, etc.) enable their
//! graphics/keyboard-protocol paths.
//!
//! `TERM=xterm-kitty` is set on Windows too (as the architecture contract
//! requires): kitty-graphics aware tools (`kitten icat`, `yazi`, `chafa`, ...)
//! detect support primarily through `TERM`, and grimalkin has no other
//! discoverable identity. Callers can override any variable through the
//! `env` argument (e.g. `TERM=xterm-256color` for MSYS/Cygwin shells that lack
//! a matching terminfo entry).
//!
//! On macOS, `LANG=en_US.UTF-8` is supplied when neither `LANG` nor `LC_ALL`
//! is set (GUI-launched apps inherit no locale, which breaks UTF-8 in shells).
//!
//! # Ordering and shutdown
//!
//! No [`PtyEvent::Output`] is ever delivered after [`PtyEvent::Exited`]. If the
//! child exits while background processes keep the terminal open, output that
//! arrives after the drain grace period is discarded.
//!
//! [`PtyWriter`] never blocks the caller: input is queued to a dedicated writer
//! thread, so a GUI thread pasting a large buffer into a child that is not
//! reading cannot freeze.
//!
//! On Unix, [`Pty::kill`] sends `SIGHUP` and escalates to `SIGKILL` if the
//! child is still alive 2 seconds later, so ignoring `SIGHUP` cannot leak a
//! process or its threads.
//!
//! ConPTY needs the initial `cols`/`rows` at creation time, so they are
//! mandatory arguments to spawn.
//!
//! # Windows backend
//!
//! Windows uses a native ConPTY implementation (`win/conpty.rs`) rather than
//! portable-pty so that grimalkin controls the pseudoconsole flags. It loads
//! `conpty.dll` from the executable's directory when present (with
//! `OpenConsole.exe` next to it) and otherwise the in-box `kernel32.dll`
//! ConPTY. Stock Windows 10 ConPTY strips APC sequences (kitty graphics) and
//! the kitty keyboard query; a sideloaded modern ConPTY, optionally with
//! [`SpawnConfig::passthrough`], lets them through.

use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
mod unix;
#[cfg(any(windows, test))]
mod win;

/// Maximum size of one [`PtyEvent::Output`] chunk.
pub const MAX_CHUNK: usize = 64 * 1024;

/// How long, after the child exits, we wait for the reader thread to drain
/// remaining output before reporting [`PtyEvent::Exited`] anyway. This covers
/// ConPTY (whose output pipe may not close until the pseudo-console is closed)
/// and background grandchildren that keep the slave side open on Unix.
const DRAIN_GRACE: Duration = Duration::from_millis(300);

/// Upper bound on the total drain wait, even if output keeps trickling in.
const DRAIN_MAX: Duration = Duration::from_secs(3);

/// How long after a kill request a Unix child may ignore `SIGHUP` before it is
/// sent `SIGKILL`.
#[cfg(unix)]
const KILL_ESCALATION: Duration = Duration::from_secs(2);

/// Events delivered over the channel given to [`Pty::spawn`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyEvent {
    /// Bytes the child wrote to the terminal (at most [`MAX_CHUNK`] long).
    Output(Vec<u8>),
    /// The child exited. Carries the exit code if it could be determined.
    /// Sent exactly once, after all output that could be read.
    Exited(Option<u32>),
}

/// Callback invoked (from a background thread) after each event is sent.
pub type WakeFn = Box<dyn Fn() + Send + Sync>;

/// Full spawn configuration. See [`Pty::spawn`] for the short form.
pub struct SpawnConfig {
    /// Program to run. Empty means "the default shell" (see
    /// [`default_shell`]).
    pub cmd: String,
    /// Arguments (not including argv[0]).
    pub args: Vec<String>,
    /// Working directory; `None` means the user's home (Unix) / inherited.
    pub cwd: Option<PathBuf>,
    /// Extra environment variables; these override grimalkin's defaults.
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
    /// Pixel size of the whole terminal area (0 = unknown).
    pub pixel_width: u16,
    pub pixel_height: u16,
    /// Unix only: run as a login shell (argv[0] prefixed with `-`).
    pub login_shell: bool,
    /// Windows only: create the pseudoconsole with
    /// `PSEUDOCONSOLE_PASSTHROUGH_MODE`, so a new enough ConPTY forwards the
    /// child's output sequences (APC, DCS, ...) unmodified instead of
    /// re-rendering them. Ignored elsewhere. Falls back to a normal
    /// pseudoconsole if the ConPTY in use rejects the flag. Default `false`.
    pub passthrough: bool,
    /// Channel receiving [`PtyEvent`]s.
    pub tx: Sender<PtyEvent>,
    /// Called after every event is sent.
    pub on_wake: Option<WakeFn>,
}

/// Cloneable, thread-safe, non-blocking handle to the child's stdin (the pty
/// master). Writes are queued and performed by a background thread in order.
/// If that thread hits an I/O error (typically because the child is gone),
/// later writes fail with [`io::ErrorKind::BrokenPipe`]. Writes also fail
/// with `BrokenPipe` once the child has exited (no later than the delivery of
/// [`PtyEvent::Exited`]).
#[derive(Clone)]
pub struct PtyWriter {
    tx: Sender<Vec<u8>>,
    failed: Arc<AtomicBool>,
}

impl PtyWriter {
    fn new(mut w: Box<dyn Write + Send>) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let failed = Arc::new(AtomicBool::new(false));
        let flag = failed.clone();
        thread::Builder::new()
            .name("grimalkin-pty-writer".into())
            .spawn(move || {
                for buf in rx {
                    if w.write_all(&buf).and_then(|_| w.flush()).is_err() {
                        flag.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            })?;
        Ok(Self { tx, failed })
    }

    /// Queue all of `buf` for the child. Never blocks on the child.
    pub fn write_all(&self, buf: &[u8]) -> io::Result<()> {
        if self.failed.load(Ordering::SeqCst) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        if buf.is_empty() {
            return Ok(());
        }
        self.tx
            .send(buf.to_vec())
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
    }

    /// Input is handed to the writer thread immediately, so there is nothing
    /// to flush; this only reports whether the writer has failed.
    pub fn flush(&self) -> io::Result<()> {
        if self.failed.load(Ordering::SeqCst) {
            Err(io::ErrorKind::BrokenPipe.into())
        } else {
            Ok(())
        }
    }
}

impl Write for PtyWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        PtyWriter::write_all(self, buf)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        PtyWriter::flush(self)
    }
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        PtyWriter::write_all(self, buf)
    }
}

impl std::fmt::Debug for PtyWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PtyWriter")
    }
}

// ---------------------------------------------------------------------------
// Platform backend interface
// ---------------------------------------------------------------------------

/// The pty master side: resizing, and (on drop) tearing the pty down.
trait Master: Send {
    fn resize(&self, cols: u16, rows: u16, pixel_w: u16, pixel_h: u16) -> io::Result<()>;
}

/// Blocks until the child exits.
trait ChildWait: Send {
    /// Wait for exit; the exit code if it could be determined.
    fn wait(&mut self) -> Option<u32>;
}

/// Terminates the child from any thread.
trait Killer: Send + Sync {
    fn kill(&mut self) -> io::Result<()>;
}

/// Everything a backend hands back from a successful spawn.
struct Spawned {
    master: Box<dyn Master>,
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn ChildWait>,
    killer: Box<dyn Killer>,
    pid: Option<u32>,
}

#[cfg(unix)]
use unix::open as backend_open;
#[cfg(windows)]
use win::open as backend_open;

/// A running child attached to a pseudo-terminal.
pub struct Pty {
    // Field order matters: on drop the pseudoconsole (`master`) must be
    // closed before anything else (see `win/conpty.rs`).
    master: Box<dyn Master>,
    writer: PtyWriter,
    killer: Box<dyn Killer>,
    status: Arc<Mutex<Option<Option<u32>>>>,
    pid: Option<u32>,
    #[cfg(unix)]
    escalating: bool,
}

#[derive(Default)]
struct ReaderState {
    done: bool,
    /// Number of chunks forwarded so far (progress indicator).
    reads: u64,
}

/// Wait for the reader to finish, giving up after [`DRAIN_GRACE`] without
/// progress or [`DRAIN_MAX`] in total.
fn wait_for_drain(state: &(Mutex<ReaderState>, Condvar)) {
    let (lock, cv) = state;
    let start = Instant::now();
    let mut guard = lock.lock().unwrap_or_else(|p| p.into_inner());
    let mut seen = guard.reads;
    while !guard.done && start.elapsed() < DRAIN_MAX {
        let (g, res) = cv
            .wait_timeout(guard, DRAIN_GRACE)
            .unwrap_or_else(|p| p.into_inner());
        guard = g;
        if res.timed_out() && guard.reads == seen {
            break;
        }
        seen = guard.reads;
    }
}

impl Pty {
    /// Spawn `cmd` with `args` on a new pty of `cols` x `rows` cells.
    ///
    /// An empty `cmd` selects the default shell. On Unix, a shell (sh, bash,
    /// zsh, fish, ...) started without arguments is launched as a login shell.
    /// `env` entries override grimalkin's defaults. Events are sent to `tx`.
    pub fn spawn(
        cmd: &str,
        args: &[String],
        cwd: Option<&Path>,
        env: &[(String, String)],
        cols: u16,
        rows: u16,
        tx: Sender<PtyEvent>,
    ) -> io::Result<Pty> {
        Self::spawn_with(SpawnConfig {
            cmd: cmd.to_string(),
            args: args.to_vec(),
            cwd: cwd.map(Path::to_path_buf),
            env: env.to_vec(),
            cols,
            rows,
            pixel_width: 0,
            pixel_height: 0,
            login_shell: args.is_empty() && (cmd.is_empty() || is_known_shell(cmd)),
            passthrough: false,
            tx,
            on_wake: None,
        })
    }

    /// Spawn using a full [`SpawnConfig`].
    pub fn spawn_with(cfg: SpawnConfig) -> io::Result<Pty> {
        let Spawned {
            master,
            reader,
            writer,
            mut child,
            killer,
            pid,
        } = backend_open(&cfg)?;
        let writer = PtyWriter::new(writer)?;

        let dead = writer.failed.clone();
        let wake: Option<Arc<dyn Fn() + Send + Sync>> = cfg.on_wake.map(Arc::from);
        let status = Arc::new(Mutex::new(None));
        let reader_state = Arc::new((Mutex::new(ReaderState::default()), Condvar::new()));
        // Set (under the lock) when Exited is sent; the reader checks it under
        // the same lock so Output can never follow Exited.
        let closed = Arc::new(Mutex::new(false));

        // Reader thread.
        {
            let tx = cfg.tx.clone();
            let wake = wake.clone();
            let state = reader_state.clone();
            let closed = closed.clone();
            thread::Builder::new()
                .name("grimalkin-pty-reader".into())
                .spawn(move || {
                    read_loop(reader, &tx, wake.as_deref(), &closed, &state);
                    let (lock, cv) = &*state;
                    lock.lock().unwrap_or_else(|p| p.into_inner()).done = true;
                    cv.notify_all();
                })?;
        }

        // Wait thread: owns the child, reports exit after output is drained.
        {
            let tx = cfg.tx;
            let status = status.clone();
            thread::Builder::new()
                .name("grimalkin-pty-wait".into())
                .spawn(move || {
                    let code = child.wait();
                    *status.lock().unwrap_or_else(|p| p.into_inner()) = Some(code);
                    wait_for_drain(&reader_state);
                    {
                        let mut c = closed.lock().unwrap_or_else(|p| p.into_inner());
                        *c = true;
                        // Writes after exit fail deterministically on every
                        // platform (Linux may otherwise accept them into the
                        // pty buffer indefinitely).
                        dead.store(true, Ordering::SeqCst);
                        let _ = tx.send(PtyEvent::Exited(code));
                    }
                    if let Some(w) = &wake {
                        w();
                    }
                })?;
        }

        Ok(Pty {
            master,
            writer,
            killer,
            status,
            pid,
            #[cfg(unix)]
            escalating: false,
        })
    }

    /// A cloneable handle for writing input to the child.
    pub fn writer(&self) -> PtyWriter {
        self.writer.clone()
    }

    /// Convenience for `self.writer().write_all(buf)`.
    pub fn write_all(&self, buf: &[u8]) -> io::Result<()> {
        self.writer.write_all(buf)
    }

    /// Flush buffered input.
    pub fn flush(&self) -> io::Result<()> {
        self.writer.flush()
    }

    /// Resize the pty. Pixel sizes may be 0 if unknown.
    pub fn resize(&self, cols: u16, rows: u16, pixel_w: u16, pixel_h: u16) -> io::Result<()> {
        self.master
            .resize(cols.max(1), rows.max(1), pixel_w, pixel_h)
    }

    /// Terminate the child (SIGHUP then, after 2 s, SIGKILL on Unix;
    /// `TerminateProcess` on Windows). Killing an already-exited child is not
    /// an error.
    pub fn kill(&mut self) -> io::Result<()> {
        if self.try_wait().is_some() {
            return Ok(());
        }
        let r = self.killer.kill();
        #[cfg(unix)]
        if !self.escalating {
            if let Some(pid) = self.pid {
                self.escalating = true;
                let status = self.status.clone();
                // Best effort; failing to spawn just skips the escalation.
                let _ = thread::Builder::new()
                    .name("grimalkin-pty-reaper".into())
                    .spawn(move || {
                        let deadline = Instant::now() + KILL_ESCALATION;
                        while Instant::now() < deadline {
                            if status.lock().unwrap_or_else(|p| p.into_inner()).is_some() {
                                return;
                            }
                            thread::sleep(Duration::from_millis(25));
                        }
                        // `status` is only set after the child is reaped, so
                        // while it is None the pid cannot have been recycled.
                        let guard = status.lock().unwrap_or_else(|p| p.into_inner());
                        if guard.is_none() {
                            unsafe {
                                libc::kill(pid as libc::pid_t, libc::SIGKILL);
                            }
                        }
                    });
            }
        }
        r
    }

    /// Kill the child and wait up to `grace` for it to exit; on Unix a child
    /// that ignores SIGHUP is then SIGKILLed. Use this on application exit,
    /// where the detached escalation thread started by `kill` would be torn
    /// down with the process and leave the child orphaned.
    pub fn shutdown(&mut self, grace: Duration) {
        let _ = self.kill();
        let deadline = Instant::now() + grace;
        while self.try_wait().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        #[cfg(unix)]
        if self.try_wait().is_none() {
            if let Some(pid) = self.pid {
                // `status` is only set after the child is reaped, so the pid
                // cannot have been recycled while it is still None.
                let guard = self.status.lock().unwrap_or_else(|p| p.into_inner());
                if guard.is_none() {
                    // The child is a session leader (its pgid is its pid), so
                    // this also takes down whatever it started.
                    unsafe {
                        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
                        libc::kill(pid as libc::pid_t, libc::SIGKILL);
                    }
                }
            }
            // Give the wait thread a moment to reap it.
            let reap_deadline = Instant::now() + Duration::from_millis(200);
            while self.try_wait().is_none() && Instant::now() < reap_deadline {
                thread::sleep(Duration::from_millis(2));
            }
        }
    }

    /// Non-blocking exit check. `None` while the child runs; `Some(code)` once
    /// it has exited, where `code` is `None` if it could not be determined.
    pub fn try_wait(&self) -> Option<Option<u32>> {
        *self.status.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// True while the child has not exited.
    pub fn is_alive(&self) -> bool {
        self.try_wait().is_none()
    }

    /// OS process id of the child, if known.
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

fn read_loop(
    mut reader: Box<dyn Read + Send>,
    tx: &Sender<PtyEvent>,
    wake: Option<&(dyn Fn() + Send + Sync)>,
    closed: &Mutex<bool>,
    state: &(Mutex<ReaderState>, Condvar),
) {
    let mut buf = vec![0u8; MAX_CHUNK];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                {
                    let c = closed.lock().unwrap_or_else(|p| p.into_inner());
                    if *c || tx.send(PtyEvent::Output(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
                {
                    let (lock, cv) = state;
                    lock.lock().unwrap_or_else(|p| p.into_inner()).reads += 1;
                    cv.notify_all();
                }
                if let Some(w) = wake {
                    w();
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            // EIO on Linux once the slave closes; any other error is terminal too.
            Err(_) => break,
        }
    }
}

// ---------------------------------------------------------------------------
// Shell resolution and environment
// ---------------------------------------------------------------------------

/// Shell names that should be started as login shells on Unix when no
/// arguments are given.
fn is_known_shell(cmd: &str) -> bool {
    let base = Path::new(cmd)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or(cmd);
    matches!(
        base,
        "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "tcsh" | "csh" | "nu" | "elvish" | "xonsh"
    )
}

/// Pure Unix shell resolution: `$SHELL` if it is set and non-empty and passes
/// `usable`, otherwise `/bin/sh`.
pub fn resolve_unix_shell(shell_env: Option<&OsStr>, usable: impl Fn(&Path) -> bool) -> PathBuf {
    match shell_env {
        Some(s) if !s.is_empty() && usable(Path::new(s)) => PathBuf::from(s),
        _ => PathBuf::from("/bin/sh"),
    }
}

/// Pure Windows shell resolution: the first of `pwsh.exe`, `powershell.exe`,
/// `cmd.exe` found in a directory of `path_var` (as accepted by
/// [`std::env::split_paths`]) for which `exists` is true. Falls back to the bare
/// name `cmd.exe`.
pub fn resolve_windows_shell(path_var: &OsStr, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let dirs: Vec<PathBuf> = std::env::split_paths(path_var).collect();
    for name in ["pwsh.exe", "powershell.exe", "cmd.exe"] {
        for d in &dirs {
            let p = d.join(name);
            if exists(&p) {
                return p;
            }
        }
    }
    PathBuf::from("cmd.exe")
}

/// The platform's default shell: pwsh > powershell > cmd on Windows (PATH
/// lookup), `$SHELL` or `/bin/sh` elsewhere.
pub fn default_shell() -> PathBuf {
    #[cfg(windows)]
    {
        let path = std::env::var_os("PATH").unwrap_or_default();
        resolve_windows_shell(&path, |p| p.is_file())
    }
    #[cfg(not(windows))]
    {
        let sh = std::env::var_os("SHELL");
        resolve_unix_shell(sh.as_deref(), is_executable)
    }
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// The environment grimalkin sets for every child, before caller overrides.
pub fn default_env() -> Vec<(OsString, OsString)> {
    #[allow(unused_mut)]
    let mut v: Vec<(OsString, OsString)> = vec![("COLORTERM".into(), "truecolor".into())];
    v.push(("TERM".into(), "xterm-kitty".into()));
    #[cfg(target_os = "macos")]
    if std::env::var_os("LANG").is_none_or(|v| v.is_empty())
        && std::env::var_os("LC_ALL").is_none_or(|v| v.is_empty())
    {
        v.push(("LANG".into(), "en_US.UTF-8".into()));
    }
    v.push(("TERM_PROGRAM".into(), "grimalkin".into()));
    v.push((
        "TERM_PROGRAM_VERSION".into(),
        env!("CARGO_PKG_VERSION").into(),
    ));
    v
}

/// Resolve `cmd` to an absolute executable path: names containing a separator
/// are taken relative to `cwd`, bare names are searched in `$PATH`. Returns
/// `cmd` unchanged if nothing is found.
#[cfg(unix)]
fn resolve_in_path(cmd: &str, cwd: Option<&Path>) -> PathBuf {
    let p = Path::new(cmd);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    if cmd.contains('/') {
        let base = cwd
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        return base.join(p);
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let cand = dir.join(cmd);
            if is_executable(&cand) {
                return cand;
            }
        }
    }
    p.to_path_buf()
}

#[cfg(unix)]
fn build_command(cfg: &SpawnConfig) -> portable_pty::CommandBuilder {
    use portable_pty::CommandBuilder;
    let mut builder;
    if cfg.cmd.is_empty() || (cfg.login_shell && cfg.args.is_empty()) {
        // portable-pty's default-prog path execs `$SHELL` (from this
        // builder's env) with argv[0] = "-<basename>", i.e. a login shell.
        let shell = if cfg.cmd.is_empty() {
            default_shell()
        } else {
            // portable-pty only honors an absolute, executable $SHELL.
            resolve_in_path(&cfg.cmd, cfg.cwd.as_deref())
        };
        builder = CommandBuilder::new_default_prog();
        builder.env("SHELL", shell);
    } else {
        builder = CommandBuilder::new(&cfg.cmd);
        builder.args(&cfg.args);
    }
    for (k, v) in default_env() {
        builder.env(k, v);
    }
    for (k, v) in &cfg.env {
        builder.env(k, v);
    }
    if let Some(cwd) = &cfg.cwd {
        builder.cwd(cwd);
    }
    builder
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn unix_shell_prefers_env() {
        let p = resolve_unix_shell(Some(OsStr::new("/bin/zsh")), |_| true);
        assert_eq!(p, PathBuf::from("/bin/zsh"));
    }

    #[test]
    fn unix_shell_falls_back() {
        assert_eq!(resolve_unix_shell(None, |_| true), PathBuf::from("/bin/sh"));
        assert_eq!(
            resolve_unix_shell(Some(OsStr::new("")), |_| true),
            PathBuf::from("/bin/sh")
        );
        assert_eq!(
            resolve_unix_shell(Some(OsStr::new("/nope")), |_| false),
            PathBuf::from("/bin/sh")
        );
    }

    fn sep_path(dirs: &[&str]) -> OsString {
        std::env::join_paths(dirs).unwrap()
    }

    #[test]
    fn windows_shell_order() {
        let path = sep_path(&["/a", "/b"]);
        let present: HashSet<PathBuf> = ["/b/pwsh.exe", "/a/powershell.exe", "/a/cmd.exe"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let got = resolve_windows_shell(&path, |p| present.contains(p));
        assert_eq!(got, PathBuf::from("/b/pwsh.exe"));

        let present: HashSet<PathBuf> = ["/b/powershell.exe", "/a/cmd.exe"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let got = resolve_windows_shell(&path, |p| present.contains(p));
        assert_eq!(got, PathBuf::from("/b/powershell.exe"));

        let present: HashSet<PathBuf> = ["/a/cmd.exe"].iter().map(PathBuf::from).collect();
        let got = resolve_windows_shell(&path, |p| present.contains(p));
        assert_eq!(got, PathBuf::from("/a/cmd.exe"));
    }

    #[test]
    fn windows_shell_fallback() {
        let path = sep_path(&["/a"]);
        assert_eq!(
            resolve_windows_shell(&path, |_| false),
            PathBuf::from("cmd.exe")
        );
    }

    #[test]
    fn known_shells() {
        assert!(is_known_shell("/bin/zsh"));
        assert!(is_known_shell("bash"));
        assert!(!is_known_shell("cat"));
        assert!(!is_known_shell("/usr/bin/vim"));
    }

    #[test]
    fn default_env_contents() {
        let env = default_env();
        let get = |k: &str| {
            env.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.to_string_lossy().into_owned())
        };
        assert_eq!(get("COLORTERM").as_deref(), Some("truecolor"));
        assert_eq!(get("TERM_PROGRAM").as_deref(), Some("grimalkin"));
        assert_eq!(
            get("TERM_PROGRAM_VERSION").as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(get("TERM").as_deref(), Some("xterm-kitty"));
    }

    #[cfg(unix)]
    fn cfg(cmd: &str, args: &[&str], env: &[(&str, &str)]) -> SpawnConfig {
        let (tx, _rx) = std::sync::mpsc::channel();
        SpawnConfig {
            cmd: cmd.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: None,
            env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
            login_shell: false,
            passthrough: false,
            tx,
            on_wake: None,
        }
    }

    #[cfg(unix)]
    #[test]
    fn caller_env_overrides_defaults() {
        let b = build_command(&cfg("prog", &[], &[("TERM_PROGRAM", "other"), ("FOO", "bar")]));
        assert_eq!(b.get_env("TERM_PROGRAM"), Some(OsStr::new("other")));
        assert_eq!(b.get_env("FOO"), Some(OsStr::new("bar")));
        assert_eq!(b.get_env("COLORTERM"), Some(OsStr::new("truecolor")));
    }

    #[cfg(unix)]
    #[test]
    fn empty_cmd_is_default_prog_login_shell() {
        let b = build_command(&cfg("", &[], &[]));
        assert!(b.is_default_prog());
        assert!(b.get_env("SHELL").is_some());
    }

    #[cfg(unix)]
    #[test]
    fn explicit_cmd_with_args_not_default_prog() {
        let b = build_command(&cfg("/bin/sh", &["-c", "true"], &[]));
        assert!(!b.is_default_prog());
        assert_eq!(b.get_argv().len(), 3);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_in_path_finds_bare_names() {
        let p = resolve_in_path("sh", None);
        assert!(p.is_absolute(), "{p:?}");
        assert!(is_executable(&p));
        // Unknown names are returned unchanged rather than panicking.
        assert_eq!(
            resolve_in_path("definitely-not-a-program-xyz", None),
            PathBuf::from("definitely-not-a-program-xyz")
        );
        assert_eq!(resolve_in_path("/bin/sh", None), PathBuf::from("/bin/sh"));
    }

    #[test]
    fn writer_reports_broken_pipe_after_writer_thread_dies() {
        struct Failing;
        impl Write for Failing {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("nope"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let w = PtyWriter::new(Box::new(Failing)).unwrap();
        w.write_all(b"a").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while w.write_all(b"b").is_ok() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(w.flush().unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    }
}
