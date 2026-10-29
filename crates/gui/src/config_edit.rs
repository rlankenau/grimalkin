//! Editing the user's `grimalkin.toml` in place.
//!
//! The Preferences UI and the menus change one setting at a time. Rewriting
//! the whole file from the parsed [`Config`](crate::config::Config) would
//! throw away the user's comments and layout, so this works on a
//! format-preserving document (`toml_edit`): only the touched keys change.
//!
//! [`ConfigEditor::save`] writes a temp file next to the config and renames it
//! over the original. The reload watcher (`reload.rs`) sees one burst of
//! events and re-applies the config exactly once, and a crash mid-write never
//! leaves a truncated config behind.

use std::hash::{BuildHasher, Hasher};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table, Value};

use crate::config::Config;

/// Content of a freshly created config file: the documented example.
const EXAMPLE: &str = include_str!("../../../docs/grimalkin.example.toml");

/// Comments that stand above `key` but are not about it, kept when the key
/// is removed: everything above the last blank line, or, for the first key
/// of the file, the whole block (a file header usually sits right on top of
/// the first setting). They move to the next entry of the table.
fn salvage_comments(table: &mut dyn toml_edit::TableLike, key: &str, first_in_root: bool) {
    let Some(prefix) = table.get_key_value(key).and_then(|(k, _)| k.leaf_decor().prefix().and_then(|p| p.as_str()).map(str::to_string)) else {
        return;
    };
    let kept = if first_in_root {
        prefix.as_str()
    } else {
        prefix.rfind("\n\n").or_else(|| prefix.rfind("\n\r\n")).map_or("", |i| &prefix[..=i + 1])
    };
    if !kept.contains('#') {
        return;
    }
    let Some(next) = table.iter().map(|(k, _)| k.to_string()).skip_while(|k| k != key).nth(1) else { return };
    let prepend = |old: Option<&str>| format!("{kept}{}", old.unwrap_or(""));
    if let Some(tbl) = table.get_mut(&next).and_then(Item::as_table_mut) {
        let p = prepend(tbl.decor().prefix().and_then(|p| p.as_str()));
        tbl.decor_mut().set_prefix(p);
    } else if let Some((mut k, _)) = table.get_key_value_mut(&next) {
        let p = prepend(k.leaf_decor().prefix().and_then(|p| p.as_str()));
        k.leaf_decor_mut().set_prefix(p);
    }
}

pub struct ConfigEditor {
    path: PathBuf,
    doc: DocumentMut,
    /// The file used Windows line endings; the parser normalises them, so
    /// they are put back when saving.
    crlf: bool,
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

impl ConfigEditor {
    /// Open the config at the default location, creating it from the
    /// embedded example if it does not exist yet.
    #[allow(dead_code)] // the app and Preferences use `open_at` (explicit path)
    pub fn open() -> io::Result<Self> {
        let path = Config::default_path().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no config directory"))?;
        Self::open_at(&path)
    }

    /// Like [`open`](Self::open) for an explicit path (`--config PATH`).
    pub fn open_at(path: &Path) -> io::Result<Self> {
        // Edit the file a symlink points at (dotfile repos) instead of
        // replacing the link with a regular file on save.
        let path = match fs::canonicalize(path) {
            Ok(p) => p,
            Err(_) => {
                // A dangling symlink would be replaced by a regular file on
                // save (and the example config created in the wrong place).
                if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{} is a symbolic link to a file that does not exist", path.display()),
                    ));
                }
                path.to_path_buf()
            }
        };
        let text = match crate::fsutil::read_regular_string(&path, crate::fsutil::MAX_CONFIG_BYTES) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if let Some(dir) = path.parent() {
                    fs::create_dir_all(dir)?;
                }
                let ed = Self::from_text(path, EXAMPLE)?;
                ed.save()?;
                return Ok(ed);
            }
            Err(e) => return Err(e),
        };
        Self::from_text(path, &text)
    }

    fn from_text(path: PathBuf, text: &str) -> io::Result<Self> {
        let doc = text.parse::<DocumentMut>().map_err(|e| invalid(format!("{}: {e}", path.display())))?;
        Ok(ConfigEditor { path, doc, crlf: text.contains("\r\n") })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The value at a dotted path such as `"font.size"`, if set.
    pub fn get(&self, path: &str) -> Option<&Value> {
        let mut item = self.doc.as_item();
        for key in path.split('.') {
            item = item.as_table_like()?.get(key)?;
        }
        item.as_value()
    }

    /// Set the value at a dotted path, creating missing tables (`[font]`).
    /// An existing value keeps its trailing comment. Fails if a path
    /// component exists but is not a table.
    pub fn set(&mut self, path: &str, value: impl Into<Value>) -> io::Result<()> {
        let mut value: Value = value.into();
        let keys: Vec<&str> = path.split('.').collect();
        let (last, parents) = keys.split_last().filter(|(l, _)| !l.is_empty()).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, format!("bad config path {path:?}"))
        })?;
        let mut table: &mut dyn toml_edit::TableLike = self.doc.as_table_mut();
        for key in parents {
            let item = table.entry(key).or_insert_with(|| Item::Table(Table::new()));
            table = item
                .as_table_like_mut()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("{key:?} in {path:?} is not a table")))?;
        }
        // Replace in place so the key keeps its leading comments, and the
        // value its trailing one.
        match table.get_mut(last) {
            Some(old) => {
                if let Some(old_value) = old.as_value() {
                    *value.decor_mut() = old_value.decor().clone();
                }
                *old = Item::Value(value);
            }
            None => {
                table.insert(last, Item::Value(value));
            }
        }
        Ok(())
    }

    /// Remove the key at a dotted path. Returns whether it existed.
    pub fn unset(&mut self, path: &str) -> bool {
        let keys: Vec<&str> = path.split('.').collect();
        let Some((last, parents)) = keys.split_last() else { return false };
        let mut table: &mut dyn toml_edit::TableLike = self.doc.as_table_mut();
        for key in parents {
            match table.get_mut(key).and_then(Item::as_table_like_mut) {
                Some(t) => table = t,
                None => return false,
            }
        }
        let first_in_root = parents.is_empty() && table.iter().next().is_some_and(|(k, _)| k == *last);
        salvage_comments(table, last, first_in_root);
        table.remove(last).is_some()
    }

    /// Write the document: temp file in the same directory, flushed to disk,
    /// then renamed over the original (atomic on the same filesystem,
    /// replacing on Windows too). The temp file is created exclusively under
    /// an unpredictable name, so a pre-planted file or symlink of any name is
    /// never written through. A read-only config is refused, not replaced.
    pub fn save(&self) -> io::Result<()> {
        use std::io::Write;
        if fs::metadata(&self.path).is_ok_and(|m| m.permissions().readonly()) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is read-only", self.path.display()),
            ));
        }
        let dir = self.path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut text = self.doc.to_string();
        if self.crlf {
            text = text.replace("\r\n", "\n").replace('\n', "\r\n");
        }
        let (tmp, mut file) = create_temp(dir, &name)?;
        let result = file
            .write_all(text.as_bytes())
            .and_then(|()| file.sync_all())
            .and_then(|()| {
                drop(file);
                // Keep the permissions of an existing config.
                if let Ok(meta) = fs::metadata(&self.path) {
                    let _ = fs::set_permissions(&tmp, meta.permissions());
                }
                fs::rename(&tmp, &self.path)
            });
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
}

/// Exclusively create `.<name>.<unique>.tmp` in `dir`.
fn create_temp(dir: &Path, name: &str) -> io::Result<(PathBuf, fs::File)> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let mut last = None;
    for _ in 0..32 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        // Not secret, but not guessable in advance either; `create_new` is
        // what makes a collision (or a planted file) harmless.
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(nanos);
        h.write_u64(SEQ.fetch_add(1, Ordering::Relaxed));
        h.write_u32(std::process::id());
        let tmp = dir.join(format!(".{name}.{:016x}.tmp", h.finish()));
        match fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(f) => return Ok((tmp, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("cannot create a temporary file")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("grimalkin-cfgedit-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[cfg(unix)]
    #[test]
    fn save_never_writes_through_a_planted_symlink_or_replaces_read_only_files() {
        // F43: the temp name was predictable and non-exclusive.
        let d = tmpdir("tmpsym");
        let victim = d.join("victim.txt");
        fs::write(&victim, "precious").unwrap();
        let f = d.join("c.toml");
        fs::write(&f, "theme = \"a\"\n").unwrap();
        let pid = std::process::id();
        std::os::unix::fs::symlink(&victim, d.join(format!(".c.toml.{pid}.tmp"))).unwrap();
        let mut ed = ConfigEditor::open_at(&f).unwrap();
        ed.set("theme", "b").unwrap();
        ed.save().unwrap();
        assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
        assert!(fs::read_to_string(&f).unwrap().contains("\"b\""));

        // Read-only configs are refused, not silently replaced.
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&f, fs::Permissions::from_mode(0o444)).unwrap();
        let mut ed = ConfigEditor::open_at(&f).unwrap();
        ed.set("theme", "c").unwrap();
        assert_eq!(ed.save().unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert!(fs::read_to_string(&f).unwrap().contains("\"b\""));

        // A dangling symlink is refused, not replaced by a regular file.
        let link = d.join("dangling.toml");
        std::os::unix::fs::symlink(d.join("nowhere/x.toml"), &link).unwrap();
        assert!(ConfigEditor::open_at(&link).is_err());
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn set_preserves_comments_and_layout() {
        let d = tmpdir("keep");
        let f = d.join("grimalkin.toml");
        let src = "# my config\ntheme = \"default\" # trailing\n\n# fonts\n[font]\nsize = 13.0   # points\nfamily = \"Menlo\"\n";
        fs::write(&f, src).unwrap();
        let mut ed = ConfigEditor::open_at(&f).unwrap();
        ed.set("theme", "nord").unwrap();
        ed.set("font.size", 15.5).unwrap();
        ed.save().unwrap();
        let out = fs::read_to_string(&f).unwrap();
        assert_eq!(
            out,
            "# my config\ntheme = \"nord\" # trailing\n\n# fonts\n[font]\nsize = 15.5   # points\nfamily = \"Menlo\"\n"
        );
        // The result still parses as a real config.
        let cfg = Config::try_load(Some(&f)).unwrap();
        assert_eq!(cfg.theme, "nord");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn creates_nested_tables_and_unsets() {
        let d = tmpdir("nest");
        let f = d.join("grimalkin.toml");
        fs::write(&f, "theme = \"x\"\n").unwrap();
        let mut ed = ConfigEditor::open_at(&f).unwrap();
        ed.set("window.opacity", 0.9).unwrap();
        ed.set("colors.background", "#101010").unwrap();
        ed.set("window.padding", 4).unwrap();
        assert_eq!(ed.get("window.opacity").and_then(Value::as_float), Some(0.9));
        assert_eq!(ed.get("colors.background").and_then(Value::as_str), Some("#101010"));
        assert!(ed.get("window.missing").is_none());
        assert!(ed.get("nope.deeper").is_none());
        ed.save().unwrap();
        let text = fs::read_to_string(&f).unwrap();
        assert!(text.contains("[window]") && text.contains("[colors]"), "{text}");
        let cfg = Config::try_load(Some(&f)).unwrap();
        assert!((cfg.window.opacity - 0.9).abs() < 1e-6);

        let mut ed = ConfigEditor::open_at(&f).unwrap();
        assert!(ed.unset("window.opacity"));
        assert!(!ed.unset("window.opacity"));
        assert!(!ed.unset("nothing.here"));
        assert!(ed.get("window.opacity").is_none());
        // A scalar in the way is an error, not a clobber.
        assert!(ed.set("theme.sub", 1).is_err());
        assert!(ed.set("", 1).is_err());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_file_is_created_from_the_example() {
        let d = tmpdir("create");
        let f = d.join("sub").join("grimalkin.toml");
        let mut ed = ConfigEditor::open_at(&f).unwrap();
        assert!(f.is_file());
        assert!(fs::read_to_string(&f).unwrap().contains("Grimalkin example configuration"));
        // `theme` must be a top-level key in the example, not inside [window].
        assert_eq!(ed.get("theme").and_then(Value::as_str), Some("default"));
        ed.set("theme", "dracula").unwrap();
        ed.save().unwrap();
        let text = fs::read_to_string(&f).unwrap();
        assert!(text.contains("Grimalkin example configuration"), "comments survive");
        assert_eq!(Config::try_load(Some(&f)).unwrap().theme, "dracula");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn save_is_atomic_and_leaves_no_temp_files() {
        let d = tmpdir("atomic");
        let f = d.join("grimalkin.toml");
        fs::write(&f, "theme = \"a\"\n").unwrap();
        let mut ed = ConfigEditor::open_at(&f).unwrap();
        ed.set("theme", "b").unwrap();
        ed.save().unwrap();
        ed.save().unwrap();
        let names: Vec<String> = fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["grimalkin.toml".to_string()]);

        // A failed write (target is a directory) cleans up its temp file.
        let bad = d.join("dir.toml");
        fs::create_dir(&bad).unwrap();
        let mut ed = ConfigEditor::from_text(bad.clone(), "").unwrap();
        ed.set("a", 1).unwrap();
        assert!(ed.save().is_err());
        assert!(!fs::read_dir(&d).unwrap().flatten().any(|e| e.file_name().to_string_lossy().ends_with(".tmp")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn invalid_toml_is_an_error() {
        let d = tmpdir("bad");
        let f = d.join("grimalkin.toml");
        fs::write(&f, "theme = = 1").unwrap();
        let err = ConfigEditor::open_at(&f).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let _ = fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_config_is_edited_through_the_link() {
        let d = tmpdir("link");
        let real = d.join("real.toml");
        let link = d.join("grimalkin.toml");
        fs::write(&real, "theme = \"a\"\n").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mut ed = ConfigEditor::open_at(&link).unwrap();
        ed.set("theme", "b").unwrap();
        ed.save().unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert!(fs::read_to_string(&real).unwrap().contains("\"b\""));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn inline_tables_crlf_and_value_types_survive() {
        let dir = tmpdir("inline");
        let f = dir.join("grimalkin.toml");
        fs::write(&f, "# c\r\nfont = { size = 12.0, family = \"Menlo\" } # inline\r\ntheme = \"nord\"\r\n").unwrap();
        let mut ed = ConfigEditor::open_at(&f).unwrap();
        ed.set("font.size", 15.5).unwrap();
        ed.set("theme", "default").unwrap();
        ed.set("font.family", toml_edit::Value::from_iter(["A", "B"])).unwrap();
        assert!(ed.set("theme.nested", 1).is_err(), "a string is not a table");
        assert!(ed.set("", 1).is_err());
        assert!(!ed.unset("window.opacity"), "removing a missing key is not an error");
        ed.save().unwrap();
        let text = fs::read_to_string(&f).unwrap();
        assert!(text.contains("# inline") && text.starts_with("# c\r\n"), "{text:?}");
        let c = Config::try_load(Some(&f)).unwrap();
        assert_eq!((c.font.size, c.theme.as_str()), (15.5, "default"));
        assert_eq!(c.font.family, vec!["A".to_string(), "B".to_string()]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn removing_a_key_keeps_the_file_header_and_detached_comments() {
        let dir = tmpdir("salvage");
        let f = dir.join("grimalkin.toml");
        let text = "# my config header\ntheme = \"nord\" # why\nskin = \"llama\"\n\n# about fonts\n[font]\n# the size\nsize = 14.0\n\n# loose note\n\n# line height\nline_height = 1.2\nfamily = \"Menlo\"\n";
        fs::write(&f, text).unwrap();
        let mut ed = ConfigEditor::open_at(&f).unwrap();
        assert!(ed.unset("theme"));
        assert!(ed.unset("skin"));
        assert!(ed.unset("font.size"));
        assert!(ed.unset("font.line_height"));
        ed.save().unwrap();
        let out = fs::read_to_string(&f).unwrap();
        assert_eq!(out, "# my config header\n\n# about fonts\n[font]\n\n# loose note\n\nfamily = \"Menlo\"\n", "{out}");
        assert!(Config::try_load(Some(&f)).is_ok());
        // Removing the only key leaves the header on the table.
        let mut ed = ConfigEditor::open_at(&f).unwrap();
        assert!(ed.unset("font.family"));
        assert!(ed.get("font").is_none() || ed.get("font.family").is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
