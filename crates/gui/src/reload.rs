//! Config hot-reload: watch the config file (and the user theme and skin directories)
//! and call back, debounced, when something changed.
//!
//! The callback runs on a helper thread; `app.rs` uses it to post a
//! `UserEvent::ConfigChanged` through the `EventLoopProxy`. Editors usually
//! save by writing a temp file and renaming it over the original, so the
//! *directory* is watched and events are filtered by path.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher};

use crate::config::Config;

/// Quiet period after the last file event before reloading.
const DEBOUNCE: Duration = Duration::from_millis(150);

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
    match (p.parent().and_then(|d| d.canonicalize().ok()), p.file_name()) {
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
    let mut watcher = match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(ev) = res else { return };
        if matches!(ev.kind, EventKind::Access(_)) {
            return;
        }
        let hit = ev
            .paths
            .iter()
            .any(|p| {
                *p == relevant_file
                    || relevant_themes.as_ref().is_some_and(|t| p.starts_with(t))
                    || relevant_skins.as_ref().is_some_and(|t| p.starts_with(t))
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
            while rx.recv().is_ok() {
                loop {
                    match rx.recv_timeout(DEBOUNCE) {
                        Ok(()) => continue,
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
                on_change();
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
    use std::time::Instant;

    #[test]
    fn debounced_callback_fires_once_per_burst() {
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
        // Unrelated files do not trigger; a burst of writes triggers once.
        std::fs::write(dir.join("other.txt"), "x").unwrap();
        for i in 0..5 {
            std::fs::write(&file, format!("scrollback_lines = {i}")).unwrap();
        }
        let t0 = Instant::now();
        while hits.load(Ordering::SeqCst) == 0 && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
