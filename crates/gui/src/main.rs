//! Grimalkin: a GPU-accelerated terminal emulator.
//!
//! Module map (each is meant to be extended independently):
//! * `config`        TOML configuration; `config_edit` edits the file in place
//! * `menu`          native menu bar (macOS) and context menu
//! * `prefs`         the Preferences window (egui, second winit window)
//! * `app`           window/event loop glue, pty + terminal ownership
//! * `renderer`      wgpu renderer (grid instances, cursor); `glyphs` fonts/atlas;
//!   `boxdraw` procedural box/block characters
//! * `input`         keyboard/wheel encoding, mouse + kitty keyboard hooks
//! * `selection`     text selection; `links` hyperlink detection; `bell` audible bell
//! * `theme`         color themes (TOML + kitty .conf); `reload` config hot-reload
//! * `graphics_glue` kitty graphics protocol glue (stub)
//! * `debug_script` env-driven test hooks (scale override, scripted input)
//! * `skin`         undecorated, non-rectangular window skins (docs/SKINS.md)

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod bell;
mod boxdraw;
mod config;
mod config_edit;
mod debug_script;
mod fonts;
mod fsutil;
mod glyphs;
mod graphics_glue;
mod icon;
mod images;
mod input;
mod links;
mod menu;
mod pointer;
mod prefs;
mod reload;
mod renderer;
mod selection;
mod skin;
mod terminfo;
mod textsafe;
mod theme;

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use winit::event_loop::EventLoop;

use crate::app::{App, UserEvent};
use crate::config::Config;

/// Command line options.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LaunchOptions {
    /// Program and arguments given with `-e` / `--command`.
    pub command: Option<Vec<String>>,
    pub config_path: Option<PathBuf>,
    pub working_directory: Option<PathBuf>,
    /// `--theme NAME`: overrides `theme` in the config file (also on reload).
    pub theme: Option<String>,
    /// Window skin given with `--skin` (overrides the config file).
    pub skin: Option<String>,
    /// `--install-skin PATH_OR_URL`: install a WinAMP `.wsz` and exit.
    pub install_skin: Option<String>,
    /// `--force`: let `--install-skin` overwrite an installed skin.
    pub force: bool,
}

pub enum Parsed {
    Run(LaunchOptions),
    /// Print this text and exit successfully.
    Info(String),
    Error(String),
}

const USAGE: &str = "\
Usage: grimalkin [OPTIONS] [-e COMMAND [ARGS...]]

Options:
  -e, --command CMD [ARGS...]   Run CMD instead of the shell (consumes the rest of the arguments)
      --config PATH             Use this config file
      --working-directory DIR   Start in DIR
      --theme NAME              Use this color theme (see --list-themes)
      --list-themes             List the available themes and exit
      --skin NAME               Use the window skin NAME (none = normal window;
                                winamp:NAME or a .wsz path for a WinAMP skin)
      --list-skins              List the built-in and installed skins and exit
      --install-skin SRC        Install a WinAMP .wsz (file path or https URL) and exit
      --force                   Overwrite an already installed skin
  -h, --help                    Show this help
  -V, --version                 Show the version
";

/// Split `--flag=value` at the first `=` without requiring valid Unicode (a
/// folder name can legally contain unpaired surrogates / non-UTF-8 bytes).
/// Returns the flag text (lossy: only compared against known ASCII flags) and
/// the raw value when the argument was `--flag=value`.
fn split_flag(a: &OsStr) -> (String, Option<OsString>) {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let b = a.as_bytes();
        if b.starts_with(b"--") {
            if let Some(i) = b.iter().position(|&c| c == b'=') {
                return (
                    String::from_utf8_lossy(&b[..i]).into_owned(),
                    Some(OsString::from_vec(b[i + 1..].to_vec())),
                );
            }
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let w: Vec<u16> = a.encode_wide().collect();
        let eq = u16::from(b'=');
        if w.len() > 2 && w[0] == u16::from(b'-') && w[1] == u16::from(b'-') {
            if let Some(i) = w.iter().position(|&c| c == eq) {
                return (
                    String::from_utf16_lossy(&w[..i]),
                    Some(OsString::from_wide(&w[i + 1..])),
                );
            }
        }
    }
    (a.to_string_lossy().into_owned(), None)
}

/// A value that must be Unicode (names, URLs, the command line).
fn unicode_value(flag: &str, v: OsString) -> Result<String, String> {
    v.into_string()
        .map_err(|_| format!("{flag}: argument is not valid Unicode"))
}

pub fn parse_args<I, S>(args: I) -> Parsed
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    macro_rules! unicode {
        ($flag:expr, $v:expr) => {
            match unicode_value($flag, $v) {
                Ok(s) => s,
                Err(e) => return Parsed::Error(e),
            }
        };
    }
    let mut opts = LaunchOptions::default();
    let mut it = args.into_iter().map(Into::into);
    while let Some(a) = it.next() {
        let a: OsString = a;
        let (flag, inline) = split_flag(&a);
        match flag.as_str() {
            "-h" | "--help" => return Parsed::Info(USAGE.to_string()),
            "-V" | "--version" => return Parsed::Info(format!("grimalkin {}\n", env!("CARGO_PKG_VERSION"))),
            "-e" | "--command" => {
                let mut cmd: Vec<String> = Vec::new();
                for v in inline.into_iter().chain(it.by_ref()) {
                    cmd.push(unicode!(&flag, v));
                }
                if cmd.is_empty() {
                    return Parsed::Error(format!("{flag} requires a command"));
                }
                opts.command = Some(cmd);
                break;
            }
            "--config" => match inline.or_else(|| it.next()) {
                Some(p) => opts.config_path = Some(PathBuf::from(p)),
                None => return Parsed::Error("--config requires a path".into()),
            },
            "--skin" => match inline.or_else(|| it.next()) {
                Some(s) => opts.skin = Some(unicode!(&flag, s)),
                None => return Parsed::Error("--skin requires a name".into()),
            },
            "--working-directory" => match inline.or_else(|| it.next()) {
                Some(p) => opts.working_directory = Some(PathBuf::from(p)),
                None => return Parsed::Error("--working-directory requires a path".into()),
            },
            "--theme" => match inline.or_else(|| it.next()) {
                Some(n) => opts.theme = Some(unicode!(&flag, n)),
                None => return Parsed::Error("--theme requires a name".into()),
            },
            "--install-skin" => match inline.or_else(|| it.next()) {
                Some(s) => opts.install_skin = Some(unicode!(&flag, s)),
                None => return Parsed::Error("--install-skin requires a path or URL".into()),
            },
            "--force" => opts.force = true,
            "--list-skins" => return Parsed::Info(skin::winamp::install::list_text(skin::user_skin_dir().as_deref())),
            "--list-themes" => return Parsed::Info(theme::list_text(theme::user_dir().as_deref())),
            other => return Parsed::Error(format!("unknown argument {other:?}")),
        }
    }
    Parsed::Run(opts)
}

/// Console output for the Windows release build, which is a GUI-subsystem
/// executable and so starts with no console. Elsewhere this is a no-op.
mod console {
    #[cfg(windows)]
    static ATTACHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// Attach to the parent's console (if any) and point stdout/stderr at it
    /// so `println!`/`eprintln!` are visible in PowerShell/cmd. Fails
    /// harmlessly when launched from Explorer (no parent console).
    #[cfg(windows)]
    pub fn attach() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::System::Console::{
            ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
        };
        // SAFETY: plain Win32 console calls; the opened CONOUT$ file is
        // leaked on purpose so the handle stays valid for the process.
        unsafe {
            if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                return;
            }
            ATTACHED.store(true, std::sync::atomic::Ordering::Relaxed);
            for std_handle in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
                // Keep handles the parent redirected to a file or pipe.
                let cur = GetStdHandle(std_handle);
                if !cur.is_null() && cur != INVALID_HANDLE_VALUE {
                    continue;
                }
                if let Ok(f) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
                    SetStdHandle(std_handle, f.as_raw_handle());
                    std::mem::forget(f);
                }
            }
        }
    }

    /// The parent shell already drew its prompt, so end our output with a
    /// newline when we wrote to its console.
    #[cfg(windows)]
    pub fn finish() {
        if ATTACHED.load(std::sync::atomic::Ordering::Relaxed) {
            println!();
        }
    }

    #[cfg(not(windows))]
    pub fn attach() {}
    #[cfg(not(windows))]
    pub fn finish() {}

    /// `process::exit` after `finish`.
    pub fn exit(code: i32) -> ! {
        finish();
        std::process::exit(code)
    }
}

/// Where a panic report is appended: `<data_dir>/grimalkin/panic.log`.
fn panic_log_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("grimalkin").join("panic.log"))
}

/// The text of one panic report (message, location, backtrace).
fn format_panic_report(unix_secs: u64, thread: &str, message: &str, location: &str, backtrace: &str) -> String {
    format!(
        "--- grimalkin {} panic at unix time {unix_secs} ---\nthread '{thread}' panicked at {location}:\n{message}\nbacktrace:\n{backtrace}\n",
        env!("CARGO_PKG_VERSION")
    )
}

/// Append `report` to the panic log at `path`, creating its directory.
fn append_panic_log(path: &std::path::Path, report: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(report.as_bytes())
}

/// Report panics and exit with status 101. Without this, a panic inside a
/// winit callback (an `extern "C"` frame called from AppKit / Win32) cannot
/// unwind, so the runtime aborts the process and the OS shows a crash dialog
/// that carries no panic message. Installed from `main` only, so `cargo test`
/// keeps the default hook.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let message = match info.payload().downcast_ref::<&str>() {
            Some(s) => (*s).to_string(),
            None => match info.payload().downcast_ref::<String>() {
                Some(s) => s.clone(),
                None => "Box<dyn Any>".to_string(),
            },
        };
        let location = info.location().map_or_else(|| "unknown location".to_string(), |l| l.to_string());
        let thread = std::thread::current().name().unwrap_or("<unnamed>").to_string();
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let report = format_panic_report(secs, &thread, &message, &location, &backtrace);
        // Never panic from inside the hook: a closed stderr pipe makes
        // `eprintln!` panic, and a panic in the hook aborts the process
        // (exit 134 and an OS crash report), the very thing the hook prevents.
        use std::io::Write;
        let mut err = std::io::stderr();
        let _ = writeln!(err, "{report}");
        match panic_log_path() {
            Some(path) => match append_panic_log(&path, &report) {
                Ok(()) => {
                    let _ = writeln!(err, "grimalkin: panic report written to {}", path.display());
                }
                Err(e) => {
                    let _ = writeln!(err, "grimalkin: cannot write {}: {e}", path.display());
                }
            },
            None => {
                let _ = writeln!(err, "grimalkin: no data directory; panic report not saved");
            }
        }
        std::process::exit(101);
    }));
}

/// Tell the user about a fatal start-up failure when there is no terminal to
/// print to. Best effort; silent where there is no native dialog helper.
fn fatal_dialog(msg: &str) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
        let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let (text, title) = (wide(msg), wide("grimalkin"));
        // SAFETY: both buffers are NUL-terminated and outlive the call.
        unsafe {
            MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
        }
    }
    #[cfg(target_os = "macos")]
    {
        // The message is an argument, not part of the script text.
        let _ = std::process::Command::new("osascript")
            .args(["-e", "on run argv", "-e", "display alert \"grimalkin\" message (item 1 of argv) as critical", "-e", "end run"])
            .arg(msg)
            .status();
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = msg;
}

fn main() {
    install_panic_hook();
    // Test hook (see tests/panic_hook.rs): panic after N milliseconds.
    if let Some(ms) = std::env::var("GRIMALKIN_DEBUG_PANIC").ok().and_then(|v| v.parse::<u64>().ok()) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        panic!("GRIMALKIN_DEBUG_PANIC");
    }
    console::attach();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,grimalkin=info")).init();

    let opts = match parse_args(std::env::args_os().skip(1)) {
        Parsed::Run(o) => o,
        Parsed::Info(s) => {
            print!("{s}");
            console::finish();
            return;
        }
        Parsed::Error(e) => {
            eprintln!("grimalkin: {e}\n\n{USAGE}");
            console::exit(2);
        }
    };
    if let Some(src) = &opts.install_skin {
        console::exit(skin::winamp::install::run(src, opts.force));
    }
    let mut config = Config::load(opts.config_path.as_deref());
    if let Some(t) = &opts.theme {
        config.theme = t.clone();
    }
    if let Some(s) = &opts.skin {
        config.skin = s.clone();
    }

    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(l) => l,
        Err(e) => {
            log::error!("cannot create event loop: {e}");
            std::process::exit(1);
        }
    };
    let proxy = event_loop.create_proxy();
    let mut app = App::new(config, opts, proxy);
    if let Err(e) = event_loop.run_app(&mut app) {
        log::error!("event loop error: {e}");
        std::process::exit(1);
    }
    if let Some(err) = app.fatal_error() {
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), "grimalkin: {err}");
        // A GUI launch (Dock, Explorer, a shortcut) has no stderr the user
        // sees: say it in a dialog as well.
        use std::io::IsTerminal;
        if !std::io::stderr().is_terminal() {
            fatal_dialog(err);
        }
        console::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn panic_report_is_appended_to_the_log() {
        let dir = std::env::temp_dir().join(format!("grimalkin-panic-test-{}", std::process::id()));
        let path = dir.join("nested").join("panic.log");
        let r1 = format_panic_report(1, "main", "attempt to add with overflow", "skin/hit.rs:64:18", "bt1");
        let r2 = format_panic_report(2, "main", "second", "x.rs:1:1", "bt2");
        append_panic_log(&path, &r1).unwrap();
        append_panic_log(&path, &r2).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("attempt to add with overflow") && text.contains("skin/hit.rs:64:18") && text.contains("bt1"));
        assert!(text.contains("second") && text.find("bt1") < text.find("bt2"));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(panic_log_path().is_none_or(|p| p.ends_with("grimalkin/panic.log")));
    }

    #[test]
    fn parses_command_rest() {
        match parse_args(args(&["--config", "/x.toml", "-e", "sh", "-c", "echo hi"])) {
            Parsed::Run(o) => {
                assert_eq!(o.config_path, Some(PathBuf::from("/x.toml")));
                assert_eq!(o.command, Some(args(&["sh", "-c", "echo hi"])));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_skin_flag() {
        match parse_args(args(&["--skin", "debug", "-e", "sh"])) {
            Parsed::Run(o) => assert_eq!(o.skin.as_deref(), Some("debug")),
            _ => panic!(),
        }
        match parse_args(args(&["--skin=none"])) {
            Parsed::Run(o) => assert_eq!(o.skin.as_deref(), Some("none")),
            _ => panic!(),
        }
        assert!(matches!(parse_args(args(&["--skin"])), Parsed::Error(_)));
    }

    #[test]
    fn parses_skin_install_and_list_flags() {
        match parse_args(args(&["--install-skin", "a.wsz", "--force"])) {
            Parsed::Run(o) => {
                assert_eq!(o.install_skin.as_deref(), Some("a.wsz"));
                assert!(o.force);
            }
            _ => panic!(),
        }
        match parse_args(args(&["--install-skin=https://x/y.wsz"])) {
            Parsed::Run(o) => assert_eq!(o.install_skin.as_deref(), Some("https://x/y.wsz")),
            _ => panic!(),
        }
        assert!(matches!(parse_args(args(&["--install-skin"])), Parsed::Error(_)));
        match parse_args(args(&["--list-skins"])) {
            Parsed::Info(s) => assert!(s.lines().any(|l| l == "llama")),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_workdir_and_errors() {
        match parse_args(args(&["--working-directory=/tmp"])) {
            Parsed::Run(o) => assert_eq!(o.working_directory, Some(PathBuf::from("/tmp"))),
            _ => panic!(),
        }
        assert!(matches!(parse_args(args(&["--bogus"])), Parsed::Error(_)));
        assert!(matches!(parse_args(args(&["-e"])), Parsed::Error(_)));
        assert!(matches!(parse_args(args(&["--help"])), Parsed::Info(_)));
    }

    #[test]
    fn non_unicode_arguments_do_not_panic() {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let bad = OsString::from_vec(b"a\xffb".to_vec());
            match parse_args([OsString::from("--working-directory"), bad.clone()]) {
                Parsed::Run(o) => assert_eq!(o.working_directory, Some(PathBuf::from(bad.clone()))),
                _ => panic!(),
            }
            let mut inline = b"--working-directory=".to_vec();
            inline.extend_from_slice(b"a\xffb");
            match parse_args([OsString::from_vec(inline)]) {
                Parsed::Run(o) => assert_eq!(o.working_directory, Some(PathBuf::from(bad.clone()))),
                _ => panic!(),
            }
            assert!(matches!(parse_args([OsString::from("--theme"), bad.clone()]), Parsed::Error(_)));
            assert!(matches!(parse_args([bad]), Parsed::Error(_)));
        }
    }

    #[test]
    fn parses_theme_flags() {
        match parse_args(args(&["--theme", "nord"])) {
            Parsed::Run(o) => assert_eq!(o.theme.as_deref(), Some("nord")),
            _ => panic!(),
        }
        match parse_args(args(&["--theme=dracula", "-e", "ls"])) {
            Parsed::Run(o) => assert_eq!(o.theme.as_deref(), Some("dracula")),
            _ => panic!(),
        }
        assert!(matches!(parse_args(args(&["--theme"])), Parsed::Error(_)));
        match parse_args(args(&["--list-themes"])) {
            Parsed::Info(s) => assert!(s.lines().any(|l| l == "nord")),
            _ => panic!(),
        }
    }
}
