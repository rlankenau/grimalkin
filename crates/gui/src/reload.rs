//! Config hot-reload: watch the config file (and the user theme and skin directories)
//! and call back, debounced, when something changed.
//!
//! The callback runs on a helper thread; `app.rs` uses it to post a
//! `UserEvent::ConfigChanged` through the `EventLoopProxy`. Editors usually
//! save by writing a temp file and renaming it over the original, so the
//! *directory* is watched and events are filtered by path.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};

use crate::config::Config;

/// Quiet period after the last file event before reloading.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Pure debounce state machine: coalesces events until `window` has passed
/// with no new event, then reports one firing. Takes timestamps as inputs so
/// it can be tested without a filesystem or sleeping.
#[derive(Debug)]
struct Debouncer {
    window: Duration,
    last_event: Option<Instant>,
}

impl Debouncer {
    fn new(window: Duration) -> Self {
        Self {
            window,
            last_event: None,
        }
    }

    /// Record an event at `now`, extending the quiet period.
    fn on_event(&mut self, now: Instant) {
        self.last_event = Some(now);
    }

    /// When the pending burst (if any) becomes due.
    fn deadline(&self) -> Option<Instant> {
        self.last_event.map(|t| t + self.window)
    }

    /// True exactly once per burst, once the quiet period has elapsed.
    fn due(&mut self, now: Instant) -> bool {
        match self.deadline() {
            Some(d) if now >= d => {
                self.last_event = None;
                true
            }
            _ => false,
        }
    }
}

/// Is `p` the config file or inside the themes/skins directories?
fn is_relevant(p: &Path, file: &Path, themes: Option<&Path>, skins: Option<&Path>) -> bool {
    p == file || themes.is_some_and(|t| p.starts_with(t)) || skins.is_some_and(|t| p.starts_with(t))
}

/// Keeps the watcher (and so the debounce thread) alive; dropping it stops both.
pub struct ConfigWatcher {
    _watcher: notify::RecommendedWatcher,
}

/// What a running window needs to re-read its configuration.
pub struct ReloadCtx {
    /// `--config PATH`, or `None` for the default location.
    pub path: Option<PathBuf>,
    /// `--theme NAME`, which keeps winning over the file.
    pub theme_override: Option<String>,
    /// `--skin NAME`, which keeps winning over the file.
    pub skin_override: Option<String>,
    pub _watcher: Option<ConfigWatcher>,
}

fn canon(p: &Path) -> PathBuf {
    // A file that does not exist yet cannot be canonicalized: resolve its
    // parent (FSEvents reports resolved paths, e.g. /private/var on macOS).
    if let Ok(c) = p.canonicalize() {
        return c;
    }
    match (
        p.parent().and_then(|d| d.canonicalize().ok()),
        p.file_name(),
    ) {
        (Some(d), Some(n)) => d.join(n),
        _ => p.to_path_buf(),
    }
}

/// Watch the config file at `path` (default location when `None`) and the
/// user theme directory; `on_change` is called once per burst of changes.
pub fn watch(path: Option<&Path>, on_change: impl Fn() + Send + 'static) -> Option<ConfigWatcher> {
    let file = canon(&path.map(Path::to_path_buf).or_else(Config::default_path)?);
    let config_dir = Config::config_dir().map(|d| canon(&d));
    let themes = config_dir.as_ref().map(|d| d.join("themes"));

    let (tx, rx) = mpsc::channel::<()>();
    let relevant_file = file.clone();
    let relevant_themes = themes.clone();
    let relevant_skins = config_dir.as_ref().map(|d| d.join("skins"));
    let mut watcher =
        match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            if matches!(ev.kind, EventKind::Access(_)) {
                return;
            }
            let hit = ev.paths.iter().any(|p| {
                is_relevant(
                    p,
                    &relevant_file,
                    relevant_themes.as_deref(),
                    relevant_skins.as_deref(),
                )
            });
            if hit {
                let _ = tx.send(());
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                log::warn!("config hot-reload unavailable: {e}");
                return None;
            }
        };

    let mut watching = false;
    let mut add = |dir: &Path, mode: RecursiveMode| {
        if dir.is_dir() {
            match watcher.watch(dir, mode) {
                Ok(()) => watching = true,
                Err(e) => log::warn!("cannot watch {}: {e}", dir.display()),
            }
        }
    };
    if let Some(parent) = file.parent() {
        add(parent, RecursiveMode::NonRecursive);
    }
    // The whole config directory, recursively, so a `themes` folder created
    // after startup is picked up too (events are filtered by path above).
    if let Some(d) = &config_dir {
        add(d, RecursiveMode::Recursive);
    }
    if !watching {
        log::debug!("no config directory to watch for {}", file.display());
        return None;
    }

    std::thread::Builder::new()
        .name("config-watch".into())
        .spawn(move || {
            // Ends when the watcher (and with it the sender) is dropped.
            let mut deb = Debouncer::new(DEBOUNCE);
            loop {
                let got = match deb.deadline() {
                    None => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
                    Some(d) => rx.recv_timeout(d.saturating_duration_since(Instant::now())),
                };
                match got {
                    Ok(()) => deb.on_event(Instant::now()),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
                if deb.due(Instant::now()) {
                    on_change();
                }
            }
        })
        .ok()?;
    Some(ConfigWatcher { _watcher: watcher })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn burst_within_window_fires_once() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(ms(150));
        let mut fired = 0;
        for i in 0..5 {
            let now = t0 + ms(i * 20);
            d.on_event(now);
            fired += usize::from(d.due(now));
        }
        // Poll every 10ms for a second: exactly one firing, after the last event (+80ms) + window.
        for k in 0..100 {
            let now = t0 + ms(80 + k * 10);
            if d.due(now) {
                assert!(now >= t0 + ms(230));
                fired += 1;
            }
        }
        assert_eq!(fired, 1);
    }

    #[test]
    fn events_apart_fire_separately() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(ms(150));
        d.on_event(t0);
        assert!(d.due(t0 + ms(150)));
        d.on_event(t0 + ms(1000));
        assert!(!d.due(t0 + ms(1100)));
        assert!(d.due(t0 + ms(1150)));
        assert!(!d.due(t0 + ms(2000)));
    }

    #[test]
    fn idle_debouncer_never_fires() {
        let mut d = Debouncer::new(ms(150));
        assert_eq!(d.deadline(), None);
        assert!(!d.due(Instant::now() + ms(10_000)));
    }

    #[test]
    fn unrelated_paths_are_filtered() {
        let file = Path::new("/c/grimalkin.toml");
        let themes = Path::new("/c/themes");
        let skins = Path::new("/c/skins");
        let rel = |p: &str| is_relevant(Path::new(p), file, Some(themes), Some(skins));
        assert!(rel("/c/grimalkin.toml"));
        assert!(rel("/c/themes/dark.toml"));
        assert!(rel("/c/skins/x/skin.toml"));
        assert!(!rel("/c/other.txt"));
        assert!(!is_relevant(Path::new("/c/themes/a"), file, None, None));
    }

    /// Smoke test against the real watcher: only asserts that at least one
    /// callback arrives; exact counts depend on OS timing and are covered by
    /// the `Debouncer` tests above.
    #[test]
    fn watcher_fires_at_least_once() {
        let dir = std::env::temp_dir().join(format!("grimalkin-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("grimalkin.toml");
        std::fs::write(&file, "").unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        let w = watch(Some(&file), move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        assert!(w.is_some());
        std::fs::write(&file, "scrollback_lines = 1").unwrap();
        let t0 = Instant::now();
        while hits.load(Ordering::SeqCst) == 0 && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(hits.load(Ordering::SeqCst) >= 1, "no callback within 5s");
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
