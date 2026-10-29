//! Bounded reads of files the user (or a downloaded skin) controls.
//!
//! `std::fs::read` on a path that turns out to be `/dev/zero` allocates until
//! the process dies, and opening a FIFO blocks the calling (UI) thread
//! forever. These helpers refuse anything but a regular file and cap the size.

use std::io::{self, Read};
use std::path::Path;

/// Largest config / theme file read.
pub const MAX_CONFIG_BYTES: u64 = 1 << 20;
/// Largest single skin asset (image, skin.toml) read from a skin directory.
pub const MAX_SKIN_FILE_BYTES: u64 = 32 << 20;

fn too_big(path: &Path, max: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{}: larger than {} KiB", path.display(), max >> 10),
    )
}

/// Read the regular file at `path` (symlinks followed), at most `max` bytes.
/// Directories, devices, FIFOs and sockets are refused without being opened
/// where the platform allows it, so nothing blocks.
pub fn read_regular(path: &Path, max: u64) -> io::Result<Vec<u8>> {
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{}: not a regular file", path.display()),
        ));
    }
    if meta.len() > max {
        return Err(too_big(path, max));
    }
    let f = std::fs::File::open(path)?;
    // The path may have been swapped between the check and the open.
    if !f.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{}: not a regular file", path.display()),
        ));
    }
    let mut out = Vec::new();
    f.take(max + 1).read_to_end(&mut out)?;
    if out.len() as u64 > max {
        return Err(too_big(path, max));
    }
    Ok(out)
}

/// [`read_regular`] as UTF-8 text.
pub fn read_regular_string(path: &Path, max: u64) -> io::Result<String> {
    String::from_utf8(read_regular(path, max)?)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, format!("{}: not valid UTF-8", path.display())))
}

/// True when `name` (from an untrusted skin.toml) stays inside the skin
/// directory: relative, no `..`, no drive or root.
pub fn is_contained_relative(name: &str) -> bool {
    use std::path::Component;
    !name.is_empty()
        && !name.contains('\0')
        && Path::new(name).components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        && !name.starts_with(['/', '\\'])
        && !name.contains(':')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("grimalkin-fsutil-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn reads_small_regular_files_and_refuses_big_ones() {
        let d = tmp("reg");
        std::fs::write(d.join("a"), b"hello").unwrap();
        assert_eq!(read_regular(&d.join("a"), 10).unwrap(), b"hello");
        assert!(read_regular(&d.join("a"), 4).is_err());
        assert_eq!(read_regular(&d.join("nope"), 4).unwrap_err().kind(), io::ErrorKind::NotFound);
        assert!(read_regular(&d, 100).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn devices_and_fifos_are_refused_without_blocking() {
        // F27: /dev/zero exhausted memory; a FIFO blocked the UI thread.
        assert!(read_regular(Path::new("/dev/zero"), 1 << 20).is_err());
        let d = tmp("fifo");
        let fifo = d.join("f.toml");
        let ok = std::process::Command::new("mkfifo").arg(&fifo).status().map(|s| s.success()).unwrap_or(false);
        if ok {
            assert!(read_regular(&fifo, 1 << 20).is_err());
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn asset_names_must_stay_inside_the_skin_directory() {
        for ok in ["frame.png", "img/frame.png", "./a.png"] {
            assert!(is_contained_relative(ok), "{ok}");
        }
        for bad in ["/dev/zero", "../x.png", "a/../../x", "", "C:\\x.png", "\\\\srv\\x", "a\0b"] {
            assert!(!is_contained_relative(bad), "{bad:?}");
        }
    }
}
