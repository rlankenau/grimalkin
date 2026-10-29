//! Read access to the files of a WinAMP skin: a `.wsz` (ZIP) or an unpacked
//! directory. Entry names are matched case-insensitively and may sit in
//! subfolders; nothing is ever extracted to disk by name.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

/// Largest single entry we are willing to inflate (zip-bomb guard).
pub const MAX_ENTRY: u64 = 8 * 1024 * 1024;
/// Largest archive file, and the budget of inflated bytes over all reads.
pub const MAX_ARCHIVE: u64 = 64 * 1024 * 1024;
/// Archives with more entries than this are not skins.
const MAX_ENTRIES: usize = 20_000;
/// How deep an unpacked skin directory is searched.
const MAX_DIR_DEPTH: usize = 4;

enum Store {
    Zip(zip::ZipArchive<Cursor<Vec<u8>>>),
    Dir(Vec<PathBuf>),
}

/// An opened skin container.
pub struct Archive {
    store: Store,
    /// Normalised (lower case, `/` separated) entry paths, in archive order.
    names: Vec<String>,
    budget: u64,
}

/// Lower-case, forward-slash version of an entry name.
fn normalise(raw: &[u8]) -> String {
    let s = match std::str::from_utf8(raw) {
        Ok(s) => s.to_string(),
        // Old tools wrote the OEM/Latin-1 code page.
        Err(_) => raw.iter().map(|&b| b as char).collect(),
    };
    s.replace('\\', "/").to_lowercase()
}

/// Reject entry names that could escape a directory if anyone ever extracted
/// them: NUL, a leading `/` or `\\`, a drive prefix (`C:`), or a `..` component.
fn check_entry_name(raw: &[u8]) -> Result<(), String> {
    let bad = raw.contains(&0)
        || matches!(raw.first(), Some(b'/' | b'\\'))
        || (raw.len() >= 2 && raw[0].is_ascii_alphabetic() && raw[1] == b':')
        || raw.split(|&b| b == b'/' || b == b'\\').any(|c| c == b"..");
    if bad {
        return Err(format!(
            "unsafe entry name {:?} in archive",
            String::from_utf8_lossy(raw)
        ));
    }
    Ok(())
}

fn hidden(path: &str) -> bool {
    path.ends_with('/') || path.split('/').any(|c| c == "__macosx")
}

impl Archive {
    /// Open the ZIP in `bytes`.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Archive, String> {
        if bytes.len() as u64 > MAX_ARCHIVE {
            return Err(format!("archive is larger than {} MiB", MAX_ARCHIVE >> 20));
        }
        let zip = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|e| format!("not a valid zip archive: {e}"))?;
        if zip.len() > MAX_ENTRIES {
            return Err("archive has too many entries".into());
        }
        for raw in zip.file_names_raw() {
            check_entry_name(raw)?;
        }
        let names = zip.file_names_raw().map(normalise).collect();
        Ok(Archive {
            store: Store::Zip(zip),
            names,
            budget: MAX_ARCHIVE,
        })
    }

    /// Open a `.wsz` file or an unpacked skin directory.
    pub fn open(path: &Path) -> Result<Archive, String> {
        let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if meta.is_dir() {
            return Archive::from_dir(path);
        }
        if meta.len() > MAX_ARCHIVE {
            return Err(format!(
                "{}: larger than {} MiB",
                path.display(),
                MAX_ARCHIVE >> 20
            ));
        }
        let bytes = crate::fsutil::read_regular(path, MAX_ARCHIVE).map_err(|e| format!("{}: {e}", path.display()))?;
        Archive::from_bytes(bytes).map_err(|e| format!("{}: {e}", path.display()))
    }

    fn from_dir(root: &Path) -> Result<Archive, String> {
        let mut files: Vec<PathBuf> = Vec::new();
        let mut stack = vec![(root.to_path_buf(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut entries: Vec<_> = rd.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                // `file_type` does not follow symlinks: links are skipped.
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_file() {
                    files.push(e.path());
                } else if ft.is_dir() && depth < MAX_DIR_DEPTH {
                    stack.push((e.path(), depth + 1));
                }
                if files.len() > MAX_ENTRIES {
                    return Err("directory has too many files".into());
                }
            }
        }
        let names = files
            .iter()
            .map(|p| {
                normalise(
                    p.strip_prefix(root)
                        .unwrap_or(p)
                        .to_string_lossy()
                        .as_bytes(),
                )
            })
            .collect();
        Ok(Archive {
            store: Store::Dir(files),
            names,
            budget: MAX_ARCHIVE,
        })
    }

    /// Index of the entry `[folder/]base.ext` for the first extension in
    /// `exts` that has a match. The shallowest folder wins; among entries
    /// that differ only by case the last in archive order wins (Winamp
    /// extracted onto a case-insensitive disk, so later entries overwrote
    /// earlier ones).
    pub fn find(&self, base: &str, exts: &[&str]) -> Option<usize> {
        for ext in exts {
            let want = format!("{base}.{ext}");
            let best = self
                .names
                .iter()
                .enumerate()
                .filter(|(_, n)| !hidden(n))
                .filter(|(_, n)| n.rsplit('/').next() == Some(want.as_str()))
                .max_by_key(|(i, n)| (std::cmp::Reverse(n.matches('/').count()), *i));
            if let Some((i, _)) = best {
                return Some(i);
            }
        }
        None
    }

    /// Read entry `i`, refusing anything over [`MAX_ENTRY`] and keeping a
    /// running budget over the whole archive.
    pub fn read(&mut self, i: usize) -> Result<Vec<u8>, String> {
        let limit = MAX_ENTRY.min(self.budget);
        let name = self.names[i].clone();
        let too_big = || format!("{name}: too large (limit {} MiB)", MAX_ENTRY >> 20);
        let mut out = Vec::new();
        match &mut self.store {
            Store::Zip(z) => {
                let f = z.by_index(i).map_err(|e| format!("{name}: {e}"))?;
                if f.size() > limit {
                    return Err(too_big());
                }
                f.take(limit + 1)
                    .read_to_end(&mut out)
                    .map_err(|e| format!("{name}: {e}"))?;
            }
            Store::Dir(files) => {
                out = crate::fsutil::read_regular(&files[i], limit).map_err(|e| format!("{name}: {e}"))?;
            }
        }
        if out.len() as u64 > limit {
            return Err(too_big());
        }
        self.budget -= out.len() as u64;
        Ok(out)
    }

    /// Find and read `[folder/]base.ext`.
    pub fn read_named(&mut self, base: &str, exts: &[&str]) -> Result<Option<Vec<u8>>, String> {
        match self.find(base, exts) {
            Some(i) => self.read(i).map(Some),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::io::Write;

    /// A ZIP (stored, no compression) of `(name, bytes)` entries.
    pub fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::zip_of;
    use super::*;

    #[test]
    fn lookup_is_case_insensitive_and_subfolder_tolerant() {
        let z = zip_of(&[
            ("skin/Pledit.bmp", b"a"),
            ("PLEDIT.BMP", b"b"),
            ("__MACOSX/._pledit.bmp", b"c"),
            ("sub/dir/pledit.bmp", b"d"),
        ]);
        let mut a = Archive::from_bytes(z).unwrap();
        // Shallowest wins.
        assert_eq!(a.read_named("pledit", &["bmp"]).unwrap().unwrap(), b"b");
        assert!(a.read_named("pledit", &["png"]).unwrap().is_none());
        assert!(a.find("nothing", &["bmp"]).is_none());
    }

    #[test]
    fn last_entry_wins_among_equals_and_backslashes_work() {
        let z = zip_of(&[
            ("Pledit.txt", b"first"),
            ("pledit.TXT", b"second"),
            ("a\\viscolor.txt", b"v"),
        ]);
        let mut a = Archive::from_bytes(z).unwrap();
        assert_eq!(
            a.read_named("pledit", &["txt"]).unwrap().unwrap(),
            b"second"
        );
        assert_eq!(a.read_named("viscolor", &["txt"]).unwrap().unwrap(), b"v");
    }

    #[test]
    fn oversized_entries_and_non_zips_are_refused() {
        let big = vec![0u8; MAX_ENTRY as usize + 1];
        let mut a = Archive::from_bytes(zip_of(&[("pledit.bmp", &big)])).unwrap();
        assert!(a
            .read_named("pledit", &["bmp"])
            .unwrap_err()
            .contains("too large"));
        assert!(Archive::from_bytes(b"not a zip at all".to_vec()).is_err());
    }

    #[test]
    fn unsafe_entry_names_are_rejected() {
        for bad in [
            "../x", "a/../x", "..\\x", "/abs", "\\abs", "C:evil", "c:/evil",
        ] {
            let z = zip_of(&[("pledit.bmp", b"a"), (bad, b"x")]);
            let e = Archive::from_bytes(z).err().unwrap_or_default();
            assert!(e.contains("unsafe entry name"), "{bad}: {e:?}");
        }
        let ok = zip_of(&[
            ("pledit.bmp", b"a"),
            ("sub/dir/..x.txt", b"x"),
            ("a b/c.txt", b"y"),
        ]);
        assert!(Archive::from_bytes(ok).is_ok());
    }

    #[test]
    fn unpacked_directories_work() {
        let d = std::env::temp_dir().join(format!("grimalkin-wa-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("Sub")).unwrap();
        std::fs::write(d.join("Sub").join("PLEDIT.BMP"), b"x").unwrap();
        let mut a = Archive::open(&d).unwrap();
        assert_eq!(a.read_named("pledit", &["bmp"]).unwrap().unwrap(), b"x");
        let _ = std::fs::remove_dir_all(&d);
    }
}
