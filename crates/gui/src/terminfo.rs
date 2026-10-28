//! Make `TERM=xterm-kitty` safe to advertise.
//!
//! grimalkin tells children `TERM=xterm-kitty` so kitty-aware tools enable
//! the graphics and keyboard protocols. ncurses programs (vim, htop, less,
//! tmux) only work if they can also *find* that terminfo entry; stock macOS
//! and many minimal Linux installs do not ship it, and vim then falls back to
//! a dumb terminal. kitty itself solves this by pointing `TERMINFO` at its
//! bundled database. We do the same when a kitty install provides one, and
//! otherwise fall back to `TERM=xterm-256color` instead of advertising a
//! terminal nobody can look up.

#[cfg(unix)]
use std::path::{Path, PathBuf};

/// Environment overrides for the child process (applied on top of the
/// defaults in `grimalkin-pty`).
pub fn child_env() -> Vec<(String, String)> {
    #[cfg(unix)]
    {
        unix_env()
    }
    #[cfg(not(unix))]
    {
        Vec::new()
    }
}

#[cfg(unix)]
fn unix_env() -> Vec<(String, String)> {
    let search = system_dirs();
    if has_entry(&search, "xterm-kitty") {
        return Vec::new();
    }
    for dir in bundled_dirs() {
        if has_entry(std::slice::from_ref(&dir), "xterm-kitty") {
            log::info!("using bundled terminfo from {}", dir.display());
            return vec![("TERMINFO".into(), dir.to_string_lossy().into_owned())];
        }
    }
    log::warn!("no xterm-kitty terminfo entry found; advertising TERM=xterm-256color instead");
    vec![("TERM".into(), "xterm-256color".into())]
}

/// Directories ncurses would search for an entry, in order.
#[cfg(unix)]
fn system_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    let env_dir = |name: &str| std::env::var_os(name).filter(|s| !s.is_empty());
    if let Some(t) = env_dir("TERMINFO") {
        v.push(t.into());
    }
    if let Some(h) = dirs::home_dir() {
        v.push(h.join(".terminfo"));
    }
    if let Some(t) = env_dir("TERMINFO_DIRS") {
        v.extend(std::env::split_paths(&t).filter(|p| !p.as_os_str().is_empty()));
    }
    for d in [
        "/etc/terminfo",
        "/lib/terminfo",
        "/usr/lib/terminfo",
        "/usr/share/terminfo",
        "/opt/homebrew/share/terminfo",
        "/usr/local/share/terminfo",
    ] {
        v.push(d.into());
    }
    v
}

/// Places a kitty (or grimalkin) install may keep a terminfo database.
#[cfg(unix)]
fn bundled_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        v.push(dir.join("terminfo"));
        v.push(dir.join("../share/grimalkin/terminfo"));
        v.push(dir.join("../share/terminfo"));
    }
    let mut apps = vec![PathBuf::from("/Applications/kitty.app")];
    if let Some(h) = dirs::home_dir() {
        apps.push(h.join("Applications/kitty.app"));
        v.push(h.join(".local/kitty.app/lib/kitty/terminfo"));
    }
    for app in apps {
        v.push(app.join("Contents/Resources/kitty/terminfo"));
        v.push(app.join("Contents/Resources/terminfo"));
    }
    v.push("/usr/lib/kitty/terminfo".into());
    v.push("/usr/share/kitty/terminfo".into());
    v.push("/opt/homebrew/lib/kitty/terminfo".into());
    v
}

/// True if any directory in `dirs` holds a compiled entry for `name`, in
/// either the `x/name` (Linux) or `78/name` (macOS, hex) layout.
#[cfg(unix)]
fn has_entry(dirs: &[PathBuf], name: &str) -> bool {
    let Some(first) = name.chars().next() else { return false };
    let hex = format!("{:x}", first as u32);
    dirs.iter().any(|d| {
        d.join(first.to_string()).join(name).is_file() || d.join(&hex).join(name).is_file()
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("grimalkin-ti-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn finds_both_layouts() {
        let a = scratch("a");
        let b = scratch("b");
        std::fs::create_dir_all(a.join("x")).unwrap();
        std::fs::write(a.join("x/xterm-kitty"), b"x").unwrap();
        std::fs::create_dir_all(b.join("78")).unwrap();
        std::fs::write(b.join("78/xterm-kitty"), b"x").unwrap();
        assert!(has_entry(std::slice::from_ref(&a), "xterm-kitty"));
        assert!(has_entry(std::slice::from_ref(&b), "xterm-kitty"));
        assert!(!has_entry(&[a.clone(), b.clone()], "xterm-foo"));
        assert!(!has_entry(&[], "xterm-kitty"));
        let _ = std::fs::remove_dir_all(a);
        let _ = std::fs::remove_dir_all(b);
    }
}
