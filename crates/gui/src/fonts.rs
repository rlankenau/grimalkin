//! Fast font-system construction.
//!
//! `FontSystem::new()` parses every installed font face (443 on a stock
//! Windows 10), which costs ~3 s on a cold disk cache. Instead we keep an
//! index of the installed faces (`fonts.cache` in the user cache dir, keyed
//! by the font directories' mtimes) and at startup load only the files that
//! provide the configured families. The complete system database (needed for
//! emoji / CJK fallback) is built on a background thread and swapped in the
//! first time a glyph misses the configured families.
//!
//! First run (or after fonts are installed) there is no valid index, so the
//! full database is loaded synchronously and the index is written.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::UNIX_EPOCH;

use cosmic_text::fontdb::{self, Database};
use cosmic_text::FontSystem;

const MAGIC: &str = "grimalkin-fonts-v1";

/// Font directories whose modification times invalidate the index.
fn font_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        let root = std::env::var_os("windir").or_else(|| std::env::var_os("SystemRoot"));
        v.push(root.map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("Fonts"));
        if let Some(l) = dirs::data_local_dir() {
            v.push(l.join("Microsoft").join("Windows").join("Fonts"));
        }
    } else if cfg!(target_os = "macos") {
        v.push("/System/Library/Fonts".into());
        v.push("/Library/Fonts".into());
        v.push("/Network/Library/Fonts".into());
        if let Some(h) = dirs::home_dir() {
            v.push(h.join("Library/Fonts"));
        }
    } else {
        v.push("/usr/share/fonts".into());
        v.push("/usr/local/share/fonts".into());
        if let Some(h) = dirs::home_dir() {
            v.push(h.join(".fonts"));
        }
        if let Some(d) = dirs::data_dir() {
            v.push(d.join("fonts"));
        }
    }
    v
}

/// `(path, mtime in ns)` for each font dir and its immediate subdirs.
fn dir_stamps() -> Vec<(String, u128)> {
    let mtime = |p: &Path| -> Option<u128> {
        let m = std::fs::metadata(p).ok()?.modified().ok()?;
        Some(m.duration_since(UNIX_EPOCH).ok()?.as_nanos())
    };
    let mut out = Vec::new();
    for d in font_dirs() {
        let Some(t) = mtime(&d) else { continue };
        out.push((d.to_string_lossy().into_owned(), t));
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    if let Some(t) = mtime(&e.path()) {
                        out.push((e.path().to_string_lossy().into_owned(), t));
                    }
                }
            }
        }
    }
    out.sort();
    out
}

fn cache_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("grimalkin").join("fonts.cache"))
}

/// One cached file: path and the family names of the faces inside.
struct Index {
    files: Vec<(PathBuf, Vec<String>)>,
}

fn read_index(stamps: &[(String, u128)]) -> Option<Index> {
    let text = std::fs::read_to_string(cache_path()?).ok()?;
    let mut lines = text.lines();
    if lines.next()? != MAGIC {
        return None;
    }
    let mut seen = Vec::new();
    let mut files = Vec::new();
    for l in lines {
        let mut f = l.split('\t');
        match f.next()? {
            "dir" => seen.push((f.next()?.to_string(), f.next()?.parse::<u128>().ok()?)),
            "file" => {
                let p = PathBuf::from(f.next()?);
                files.push((p, f.next()?.split('|').map(str::to_string).collect()));
            }
            _ => return None,
        }
    }
    seen.sort();
    (seen == stamps).then_some(Index { files })
}

fn write_index(stamps: &[(String, u128)], db: &Database) {
    let Some(path) = cache_path() else { return };
    let mut by_file: std::collections::BTreeMap<PathBuf, BTreeSet<String>> = Default::default();
    for face in db.faces() {
        let p = match &face.source {
            fontdb::Source::File(p) => p.clone(),
            fontdb::Source::SharedFile(p, _) => p.clone(),
            fontdb::Source::Binary(_) => continue,
        };
        let names = by_file.entry(p).or_default();
        for (n, _) in &face.families {
            if !n.contains(['\t', '|', '\n']) {
                names.insert(n.clone());
            }
        }
    }
    let mut s = format!("{MAGIC}\n");
    for (d, t) in stamps {
        if !d.contains(['\t', '\n']) {
            s.push_str(&format!("dir\t{d}\t{t}\n"));
        }
    }
    for (p, names) in &by_file {
        let ps = p.to_string_lossy();
        if ps.contains(['\t', '\n']) || names.is_empty() {
            continue;
        }
        let joined: Vec<&str> = names.iter().map(String::as_str).collect();
        s.push_str(&format!("file\t{ps}\t{}\n", joined.join("|")));
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, s).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

fn finish_db(db: &mut Database) {
    // Same generic-family defaults `FontSystem::new` applies.
    db.set_monospace_family("Noto Sans Mono");
    db.set_sans_serif_family("Open Sans");
    db.set_serif_family("DejaVu Serif");
}

fn full_font_system(locale: String) -> FontSystem {
    let mut db = Database::new();
    db.load_system_fonts();
    finish_db(&mut db);
    FontSystem::new_with_locale_and_db(locale, db)
}

/// A font system plus, when it only holds the configured families, a handle
/// to the background load of the complete one.
pub struct Fonts {
    pub fs: FontSystem,
    pending: Option<JoinHandle<FontSystem>>,
}

impl Fonts {
    pub fn load(families: &[String]) -> Fonts {
        let t0 = std::time::Instant::now();
        let stamps = dir_stamps();
        if let Some(index) = read_index(&stamps) {
            let mut db = Database::new();
            let mut loaded = 0;
            for (path, names) in &index.files {
                if names.iter().any(|n| families.iter().any(|w| w.eq_ignore_ascii_case(n))) && db.load_font_file(path).is_ok() {
                    loaded += 1;
                }
            }
            if loaded > 0 {
                finish_db(&mut db);
                let fs = FontSystem::new_with_locale_and_db(locale_of_default(), db);
                let locale = fs.locale().to_string();
                log::info!("fonts: {loaded} configured font files from index in {:?}", t0.elapsed());
                let pending = std::thread::Builder::new()
                    .name("font-loader".into())
                    .spawn(move || full_font_system(locale))
                    .ok();
                return Fonts { fs, pending };
            }
        }
        let mut db = Database::new();
        db.load_system_fonts();
        write_index(&stamps, &db);
        finish_db(&mut db);
        let fs = FontSystem::new_with_locale_and_db(locale_of_default(), db);
        log::info!("fonts: full system scan in {:?}", t0.elapsed());
        Fonts { fs, pending: None }
    }

    /// True while only the configured families are loaded.
    pub fn is_partial(&self) -> bool {
        self.pending.is_some()
    }

    /// Replace the partial font system with the complete one (waits for the
    /// background load if it is still running).
    pub fn upgrade(&mut self) {
        let Some(h) = self.pending.take() else { return };
        match h.join() {
            Ok(full) => self.fs = full,
            Err(_) => {
                log::warn!("font loader thread panicked; loading synchronously");
                self.fs = full_font_system(self.fs.locale().to_string());
            }
        }
    }
}

fn locale_of_default() -> String {
    sys_locale::get_locale().unwrap_or_else(|| "en-US".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_then_full() {
        let fam = vec!["Menlo".to_string(), "DejaVu Sans Mono".to_string(), "Consolas".to_string()];
        let _ = Fonts::load(&fam); // writes the index if missing
        let mut f = Fonts::load(&fam);
        let partial_faces = f.fs.db().len();
        f.upgrade();
        assert!(!f.is_partial());
        assert!(f.fs.db().len() >= partial_faces);
    }
}
