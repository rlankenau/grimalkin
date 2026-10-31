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

/// Notices from the app that a change waits for a restart start with this.
pub const RESTART_PREFIX: &str = "Restart grimalkin";

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
    /// Optional name for the skin being installed (overrides the derived one).
    pub install_name: String,
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

    /// Show (or, with `None`, clear) the "needs a restart" line. Returns
    /// whether the status line changed.
    pub fn restart_notice(&mut self, text: Option<&str>) -> bool {
        let shown = self.status.as_ref().is_some_and(|s| s.text.starts_with(RESTART_PREFIX));
        match text {
            Some(t) if self.status.as_ref().is_none_or(|s| s.text != t) => {
                self.note(t.to_string());
                true
            }
            None if shown => {
                self.status = None;
                true
            }
            _ => false,
        }
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
                self.status = Some(Status { text: format!("Config file has an error: {}", model::short_error(&e)), error: true });
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
    /// Returns whether the shown values changed after the controls were
    /// drawn (a reload), so the caller must draw another frame.
    pub fn tick(&mut self, now: Instant, settled: bool) -> bool {
        let mut changed = self.poll_install();
        if self.queue.due(now, settled) {
            self.flush(now);
        }
        if self.reload_pending && self.queue.is_empty() && settled {
            self.reload();
            changed = true;
        }
        changed
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
        let name = self.text.install_name.trim().to_string();
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
            let r = skin::winamp::install::install_from(&src, &dir, force, Some(&name));
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

    fn poll_install(&mut self) -> bool {
        let done = match self.install.lock() {
            Ok(mut g) if matches!(*g, Install::Done(_)) => std::mem::replace(&mut *g, Install::Idle),
            _ => return false,
        };
        if let Install::Done(r) = done {
            match r {
                Ok(name) => {
                    self.note(format!("Installed WinAMP skin \"{name}\""));
                    self.text.install.clear();
                    self.text.install_name.clear();
                    self.skins = model::skin_options(skin::user_skin_dir().as_deref(), &self.cfg.skin);
                    self.last_installed = Some(format!("winamp:{name}"));
                }
                Err(e) => self.error(format!("Install failed: {e}")),
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Duration;

    struct Fixture {
        dir: PathBuf,
        file: PathBuf,
        prefs: Prefs,
        applied: Rc<Cell<u32>>,
        picked: Rc<std::cell::RefCell<Vec<MenuAction>>>,
    }

    fn fixture(tag: &str, text: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!("grimalkin-prefs-state-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("grimalkin.toml");
        std::fs::write(&file, text).unwrap();
        let applied = Rc::new(Cell::new(0));
        let picked = Rc::new(std::cell::RefCell::new(Vec::new()));
        let (a, p) = (applied.clone(), picked.clone());
        let prefs = Prefs::new(Some(file.clone()), Box::new(move |m| p.borrow_mut().push(m)), Box::new(move || a.set(a.get() + 1)), Arc::new(|| {}));
        Fixture { dir, file, prefs, applied, picked }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn drag_writes_are_throttled_and_the_release_writes_the_rest() {
        let mut f = fixture("drag", "[font]\nsize = 12.0 # mine\n");
        let t0 = Instant::now();
        f.prefs.edit(Edit::float("font.size", 14.0));
        f.prefs.tick(t0, false);
        assert_eq!(f.applied.get(), 1, "the first write of a drag goes out at once");
        for (i, size) in [15.0, 16.0, 17.0].into_iter().enumerate() {
            f.prefs.edit(Edit::float("font.size", size));
            f.prefs.tick(t0 + Duration::from_millis(10 * (i as u64 + 1)), false);
        }
        assert_eq!(f.applied.get(), 1, "held back while the pointer is down");
        assert!(f.prefs.queue.due_at().is_some(), "a repaint is scheduled for the held-back write");
        f.prefs.tick(t0 + Duration::from_millis(120), false);
        assert_eq!(f.applied.get(), 2, "one write per throttle period");
        f.prefs.edit(Edit::float("font.size", 18.0));
        f.prefs.tick(t0 + Duration::from_millis(130), true);
        assert_eq!(f.applied.get(), 3, "release writes the final value at once");
        let text = std::fs::read_to_string(&f.file).unwrap();
        assert!(text.contains("size = 18.0 # mine"), "{text}");
        assert!(f.prefs.queue.is_empty());
    }

    #[test]
    fn external_changes_refresh_the_controls_after_the_frame() {
        let mut f = fixture("reload", "theme = \"default\"\n");
        assert_eq!(f.prefs.cfg.theme, "default");
        std::fs::write(&f.file, "theme = \"nord\"\n").unwrap();
        assert!(!f.prefs.tick(Instant::now(), true), "nothing to refresh yet");
        f.prefs.config_changed();
        let t = Instant::now();
        assert!(!f.prefs.tick(t, false) && f.prefs.cfg.theme == "default", "not while a drag is in progress");
        assert!(f.prefs.tick(t, true), "the caller is told to draw another frame");
        assert_eq!(f.prefs.cfg.theme, "nord");
    }

    #[test]
    fn a_broken_file_keeps_the_last_values_and_shows_a_short_error() {
        let mut f = fixture("broken", "[font]\nsize = 20.0\n");
        std::fs::write(&f.file, "[font]\nsize = 20.0\n[cursor]\nshape = \"banana\"\n").unwrap();
        f.prefs.config_changed();
        f.prefs.tick(Instant::now(), true);
        assert_eq!(f.prefs.cfg.font.size, 20.0);
        let s = f.prefs.status.as_ref().unwrap();
        assert!(s.error && s.text.contains("banana") && !s.text.contains('\n'), "{}", s.text);
        assert!(!s.text.contains(f.dir.to_str().unwrap()), "no path in the line: {}", s.text);
        // Fixing the file clears the message.
        std::fs::write(&f.file, "[font]\nsize = 21.0\n").unwrap();
        f.prefs.config_changed();
        f.prefs.tick(Instant::now(), true);
        assert!(f.prefs.status.is_none());
        assert_eq!(f.prefs.cfg.font.size, 21.0);
    }

    #[test]
    fn restart_notice_is_shown_and_cleared() {
        let mut f = fixture("restart", "");
        assert!(!f.prefs.restart_notice(None), "nothing to clear");
        assert!(f.prefs.restart_notice(Some("Restart grimalkin to switch.")));
        assert!(!f.prefs.restart_notice(Some("Restart grimalkin to switch.")), "unchanged");
        assert_eq!(f.prefs.status.as_ref().unwrap().text, "Restart grimalkin to switch.");
        assert!(f.prefs.restart_notice(None));
        assert!(f.prefs.status.is_none());
        // Other messages are not touched by a clear.
        f.prefs.note("Installed a skin".into());
        assert!(!f.prefs.restart_notice(None));
        assert!(f.prefs.status.is_some());
    }

    #[test]
    fn theme_and_skin_picks_go_through_the_app() {
        let mut f = fixture("picks", "");
        f.prefs.pick_theme("nord");
        f.prefs.pick_skin("llama");
        assert_eq!(*f.picked.borrow(), vec![MenuAction::SetTheme("nord".into()), MenuAction::SetSkin("llama".into())]);
        assert_eq!(f.prefs.cfg.theme, "nord");
        assert!(f.prefs.queue.is_empty(), "the app writes the file, not the window");
    }

    #[test]
    fn reset_section_rewrites_the_file_and_tells_the_app() {
        let mut f = fixture("reset", "# keep\nscrollback_lines = 7\n[font]\nsize = 20.0\n");
        f.prefs.reset_section(Section::Font);
        assert_eq!(f.applied.get(), 1);
        assert_eq!(f.prefs.cfg.font.size, Config::default().font.size);
        assert_eq!(f.prefs.cfg.scrollback_lines, 7);
        assert!(std::fs::read_to_string(&f.file).unwrap().starts_with("# keep\n"));
    }
}
