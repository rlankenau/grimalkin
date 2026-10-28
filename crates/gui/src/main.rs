//! Grimalkin: a GPU-accelerated terminal emulator.
//!
//! Module map (each is meant to be extended independently):
//! * `config`        TOML configuration
//! * `app`           window/event loop glue, pty + terminal ownership
//! * `renderer`      wgpu renderer (grid instances, cursor); `glyphs` fonts/atlas;
//!   `boxdraw` procedural box/block characters
//! * `input`         keyboard/wheel encoding, mouse + kitty keyboard hooks
//! * `selection`     text selection; `links` hyperlink detection; `bell` audible bell
//! * `theme`         color themes (TOML + kitty .conf); `reload` config hot-reload
//! * `graphics_glue` kitty graphics protocol glue (stub)

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod bell;
mod boxdraw;
mod config;
mod fonts;
mod glyphs;
mod graphics_glue;
mod images;
mod input;
mod links;
mod reload;
mod renderer;
mod selection;
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
            "--working-directory" => match inline.or_else(|| it.next()) {
                Some(p) => opts.working_directory = Some(PathBuf::from(p)),
                None => return Parsed::Error("--working-directory requires a path".into()),
            },
            "--theme" => match inline.or_else(|| it.next()) {
                Some(n) => opts.theme = Some(n),
                None => return Parsed::Error("--theme requires a name".into()),
            },
            "--list-themes" => return Parsed::Info(theme::list_text(theme::user_dir().as_deref())),
            other => return Parsed::Error(format!("unknown argument {other:?}")),
        }
    }
    Parsed::Run(opts)
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,grimalkin=info")).init();

    let opts = match parse_args(std::env::args().skip(1)) {
        Parsed::Run(o) => o,
        Parsed::Info(s) => {
            print!("{s}");
            return;
        }
        Parsed::Error(e) => {
            eprintln!("grimalkin: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    let mut config = Config::load(opts.config_path.as_deref());
    if let Some(t) = &opts.theme {
        config.theme = t.clone();
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
        std::process::exit(1);
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
