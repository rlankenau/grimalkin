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
mod theme;

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

pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Parsed {
    let mut opts = LaunchOptions::default();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (a.clone(), None),
        };
        match flag.as_str() {
            "-h" | "--help" => return Parsed::Info(USAGE.to_string()),
            "-V" | "--version" => return Parsed::Info(format!("grimalkin {}\n", env!("CARGO_PKG_VERSION"))),
            "-e" | "--command" => {
                let mut cmd: Vec<String> = inline.into_iter().collect();
                cmd.extend(it.by_ref());
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
                Some(s) => opts.skin = Some(s),
                None => return Parsed::Error("--skin requires a name".into()),
            },
            "--working-directory" => match inline.or_else(|| it.next()) {
                Some(p) => opts.working_directory = Some(PathBuf::from(p)),
                None => return Parsed::Error("--working-directory requires a path".into()),
            },
            "--theme" => match inline.or_else(|| it.next()) {
                Some(n) => opts.theme = Some(n),
                None => return Parsed::Error("--theme requires a name".into()),
            },
            "--install-skin" => match inline.or_else(|| it.next()) {
                Some(s) => opts.install_skin = Some(s),
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

fn main() {
    console::attach();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,grimalkin=info")).init();

    let opts = match parse_args(std::env::args().skip(1)) {
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
        eprintln!("grimalkin: {err}");
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
