//! Console-subsystem launcher, shipped as `grimalkin.com` next to
//! `grimalkin.exe`.
//!
//! cmd and PowerShell do not wait for GUI-subsystem programs, so output
//! ordering and exit codes break for a bare `grimalkin --list-skins`. Shells
//! resolve `grimalkin` to the `.com` before the `.exe` (PATHEXT order); this
//! tiny console program runs the real exe with the same arguments, waits, and
//! returns its exit code.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(windows)]
const TARGET: &str = "grimalkin.exe";
#[cfg(not(windows))]
const TARGET: &str = "grimalkin";

/// Pick the program to run: `grimalkin(.exe)` in the directory of
/// `current_exe` if it exists there, else the bare name for a PATH lookup.
/// On Windows the fallback keeps the `.exe` extension so the search cannot
/// resolve back to this launcher (`grimalkin.com`).
fn resolve_target(current_exe: Option<&Path>, exists: impl Fn(&Path) -> bool) -> PathBuf {
    if let Some(dir) = current_exe.and_then(Path::parent) {
        let candidate = dir.join(TARGET);
        if exists(&candidate) {
            return candidate;
        }
    }
    PathBuf::from(TARGET)
}

/// The part of a Windows command line after the program token, following the
/// CreateProcess/CommandLineToArgvW rule for argv[0]: a leading quote ends at
/// the next quote (no escapes), otherwise at the first space or tab.
#[cfg_attr(not(windows), allow(dead_code))]
fn command_line_tail(cmdline: &[u16]) -> &[u16] {
    let q = u16::from(b'"');
    let end = if cmdline.first() == Some(&q) {
        cmdline[1..]
            .iter()
            .position(|&c| c == q)
            .map_or(cmdline.len(), |i| i + 2)
    } else {
        cmdline
            .iter()
            .position(|&c| c == u16::from(b' ') || c == u16::from(b'\t'))
            .unwrap_or(cmdline.len())
    };
    let rest = &cmdline[end..];
    let skip = rest
        .iter()
        .take_while(|&&c| c == u16::from(b' ') || c == u16::from(b'\t'))
        .count();
    &rest[skip..]
}

#[cfg(windows)]
fn raw_command_line_tail() -> Option<std::ffi::OsString> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Environment::GetCommandLineW;
    // SAFETY: GetCommandLineW returns a NUL-terminated string owned by the
    // process that lives for the whole run.
    let wide = unsafe {
        let p = GetCommandLineW();
        if p.is_null() {
            return None;
        }
        let mut len = 0;
        while *p.add(len) != 0 {
            len += 1;
        }
        std::slice::from_raw_parts(p, len)
    };
    Some(std::ffi::OsString::from_wide(command_line_tail(wide)))
}

#[cfg(windows)]
unsafe extern "system" fn ctrl_handler(event: u32) -> i32 {
    use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT};
    // Ctrl+C / Ctrl+Break go to every process on the console; swallow them
    // here so only the child reacts and we report its exit code.
    i32::from(event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT)
}

fn build_command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        match raw_command_line_tail() {
            Some(tail) => {
                if !tail.is_empty() {
                    cmd.raw_arg(tail);
                }
            }
            None => {
                cmd.args(std::env::args_os().skip(1));
            }
        }
    }
    #[cfg(not(windows))]
    cmd.args(std::env::args_os().skip(1));
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    cmd
}

fn main() {
    #[cfg(windows)]
    // SAFETY: installing a handler that only returns a constant.
    unsafe {
        windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(ctrl_handler), 1);
    }

    let exe = std::env::current_exe().ok();
    let program = resolve_target(exe.as_deref(), |p| p.is_file());
    let code = match build_command(&program).status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(e) => {
            eprintln!("grimalkin: failed to start {}: {e}", program.display());
            1
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn prefers_sibling_when_present() {
        let exe = Path::new("/opt/g/grimalkin-launcher");
        let got = resolve_target(Some(exe), |p| p == Path::new("/opt/g").join(TARGET));
        assert_eq!(got, Path::new("/opt/g").join(TARGET));
    }

    #[test]
    fn falls_back_to_path_lookup() {
        let exe = Path::new("/opt/g/grimalkin-launcher");
        assert_eq!(resolve_target(Some(exe), |_| false), PathBuf::from(TARGET));
        assert_eq!(resolve_target(None, |_| true), PathBuf::from(TARGET));
    }

    #[test]
    fn tail_after_program_token() {
        let t = |s: &str| String::from_utf16(command_line_tail(&w(s))).unwrap();
        assert_eq!(t(r#"grimalkin.com --a "b c""#), r#"--a "b c""#);
        assert_eq!(
            t(r#""C:\Program Files\g\grimalkin.com"   -e "x y""#),
            r#"-e "x y""#
        );
        assert_eq!(t("grimalkin.com"), "");
        assert_eq!(t(r#""C:\g\grimalkin.com""#), "");
    }
}
