//! Preferences state and behaviour, free of egui and winit types: the config
//! as last read, pending edits, the lists the controls offer, and the
//! background jobs (font scan, skin install).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::model::{self, Edit, EditQueue, Section, SkinOption, ThemeEntry};
use crate::config::Config;
use crate::menu::MenuAction;
use crate::{skin, theme};

/// A line under the controls: what just happened, or what went wrong.
pub struct Status {
    pub text: String,
    pub error: bool,
}

/// Progress of the WinAMP skin install (it may download, so it runs on a thread).
pub enum Install {
    Idle,
    Running,
    Done(Result<String, String>),
}

/// The text boxes, which keep what the user types until it is committed.
#[derive(Default)]
pub struct TextBuffers {
    pub family: String,
    pub shell: String,
    pub title: String,
    pub install: String,
}

pub struct Prefs {
    pub cfg: Config,
    pub section: Section,
    pub queue: EditQueue,
    pub themes: Vec<ThemeEntry>,
    pub skins: Vec<SkinOption>,
    /// The chosen theme sets its own (font, cursor), which then win over the file.
    pub theme_overrides: (bool, bool),
    pub text: TextBuffers,
    /// Overwrite an already installed skin of the same name.
    pub overwrite_skin: bool,
    /// Installed monospace families once the background scan has finished.
    pub fonts: Arc<Mutex<Option<Vec<String>>>>,
    pub install: Arc<Mutex<Install>>,
    /// Config value of the skin installed last (offered as "Use it").
    pub last_installed: Option<String>,
    pub status: Option<Status>,
    /// Set by the UI each frame: a text box has the keyboard.
    pub editing_text: bool,
    path: Option<PathBuf>,
    reload_pending: bool,
    dispatch: Box<dyn Fn(MenuAction)>,
    /// Tells the app the file was just written, so it applies it at once
    /// instead of waiting for the file watcher's quiet period.
    applied: Box<dyn Fn()>,
    repaint: Arc<dyn Fn() + Send + Sync>,
}

impl Prefs {
    /// `path` is the config file being edited; `dispatch` runs an app action
    /// (theme and skin picks go through the app so a `--theme` launch
    /// override is dropped); `repaint` wakes the window from other threads.
    pub fn new(path: Option<PathBuf>, dispatch: Box<dyn Fn(MenuAction)>, applied: Box<dyn Fn()>, repaint: Arc<dyn Fn() + Send + Sync>) -> Prefs {
        let fonts = Arc::new(Mutex::new(None));
        {
            let fonts = fonts.clone();
            let repaint = repaint.clone();
            let _ = std::thread::Builder::new().name("prefs-fonts".into()).spawn(move || {
                let list = crate::fonts::monospace_families();
                if let Ok(mut g) = fonts.lock() {
                    *g = Some(list);
                }
                repaint();
            });
        }
        let mut p = Prefs {
            cfg: Config::default(),
            section: Section::Appearance,
            queue: EditQueue::default(),
            themes: Vec::new(),
            skins: Vec::new(),
            theme_overrides: (false, false),
            text: TextBuffers::default(),
            overwrite_skin: false,
            fonts,
            install: Arc::new(Mutex::new(Install::Idle)),
            last_installed: None,
            status: None,
            editing_text: false,
            path,
            reload_pending: false,
            dispatch,
            applied,
            repaint,
        };
        p.reload();
        p
    }

    pub fn config_path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }

    pub fn error(&mut self, text: String) {
        log::error!("preferences: {text}");
        self.status = Some(Status { text, error: true });
    }

    pub fn note(&mut self, text: String) {
        self.status = Some(Status { text, error: false });
    }

    /// Re-read the config file and the theme and skin lists.
    pub fn reload(&mut self) {
        self.reload_pending = false;
        match Config::try_load(self.path.as_deref()) {
            Ok(c) => {
                self.cfg = c;
                if self.status.as_ref().is_some_and(|s| s.error && s.text.starts_with("Config file")) {
                    self.status = None;
                }
            }
            Err(e) => {
                // Keep showing the last good values; the terminal does too.
                self.status = Some(Status { text: format!("Config file has an error: {e}"), error: true });
            }
        }
        let tdir = theme::user_dir();
        self.themes = model::theme_entries(tdir.as_deref());
        self.skins = model::skin_options(skin::user_skin_dir().as_deref(), &self.cfg.skin);
        self.theme_overrides = model::theme_overrides(&self.cfg.theme, tdir.as_deref());
        if !self.editing_text {
            self.text.family = self.cfg.font.family.join(", ");
            self.text.shell = self.cfg.shell.clone();
            self.text.title = self.cfg.window.title.clone();
        }
    }

    /// The config changed on disk (the app's reload fired).
    pub fn config_changed(&mut self) {
        self.reload_pending = true;
    }

    /// Queue an edit; it is written by the next [`tick`](Self::tick).
    pub fn edit(&mut self, e: Edit) {
        self.queue.push(e);
    }

    pub fn edits(&mut self, es: impl IntoIterator<Item = Edit>) {
        for e in es {
            self.queue.push(e);
        }
    }

    fn flush(&mut self, now: Instant) {
        let Some(path) = self.path.clone() else {
            self.queue = EditQueue::default();
            self.error("no config file location".into());
            return;
        };
        if let Err(e) = self.queue.flush(&path, now) {
            self.error(format!("Cannot write {}: {e}", path.display()));
        } else {
            (self.applied)();
            if self.status.as_ref().is_some_and(|s| s.error && s.text.starts_with("Cannot write")) {
                self.status = None;
            }
        }
    }

    /// Once per frame, after the controls ran. `settled` is false while a
    /// slider or other drag is in progress.
    pub fn tick(&mut self, now: Instant, settled: bool) {
        self.poll_install();
        if self.queue.due(now, settled) {
            self.flush(now);
        }
        if self.reload_pending && self.queue.is_empty() && settled {
            self.reload();
        }
    }

    /// Theme and skin picks go through the app.
    pub fn pick_theme(&mut self, name: &str) {
        self.cfg.theme = name.to_string();
        (self.dispatch)(MenuAction::SetTheme(name.to_string()));
    }

    pub fn pick_skin(&mut self, value: &str) {
        self.cfg.skin = value.to_string();
        (self.dispatch)(MenuAction::SetSkin(value.to_string()));
    }

    pub fn run_action(&self, a: MenuAction) {
        (self.dispatch)(a);
    }

    /// Remove every key of `section` from the config file.
    pub fn reset_section(&mut self, section: Section) {
        for k in section.keys() {
            self.queue.push(Edit::Unset((*k).to_string()));
        }
        self.flush(Instant::now());
        self.reload();
        self.note(format!("{} settings reset to their defaults", section.label()));
    }

    /// Install a WinAMP skin from a path or URL on a worker thread.
    pub fn start_install(&mut self) {
        let src = self.text.install.trim().to_string();
        if src.is_empty() {
            return;
        }
        let Some(dir) = skin::user_skin_dir() else {
            self.error("no config directory to install into".into());
            return;
        };
        {
            let Ok(mut g) = self.install.lock() else { return };
            if matches!(*g, Install::Running) {
                return;
            }
            *g = Install::Running;
        }
        self.status = None;
        let state = self.install.clone();
        let repaint = self.repaint.clone();
        let force = self.overwrite_skin;
        let spawned = std::thread::Builder::new().name("skin-install".into()).spawn(move || {
            let r = skin::winamp::install::install_from(&src, &dir, force);
            if let Ok(mut g) = state.lock() {
                *g = Install::Done(r);
            }
            repaint();
        });
        if spawned.is_err() {
            if let Ok(mut g) = self.install.lock() {
                *g = Install::Idle;
            }
            self.error("cannot start the install thread".into());
        }
    }

    pub fn installing(&self) -> bool {
        self.install.lock().is_ok_and(|g| matches!(*g, Install::Running))
    }

    fn poll_install(&mut self) {
        let done = match self.install.lock() {
            Ok(mut g) if matches!(*g, Install::Done(_)) => std::mem::replace(&mut *g, Install::Idle),
            _ => return,
        };
        if let Install::Done(r) = done {
            match r {
                Ok(name) => {
                    self.note(format!("Installed WinAMP skin \"{name}\""));
                    self.text.install.clear();
                    self.skins = model::skin_options(skin::user_skin_dir().as_deref(), &self.cfg.skin);
                    self.last_installed = Some(format!("winamp:{name}"));
                }
                Err(e) => self.error(format!("Install failed: {e}")),
            }
        }
    }
}
