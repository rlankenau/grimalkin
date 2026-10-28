//! Turning a transmission command plus payload into RGBA pixels: base64,
//! file/temp-file media, zlib, raw RGB/RGBA and PNG.

use crate::cmd::Cmd;
use crate::{b64, MAX_DIMENSION};
use flate2::read::ZlibDecoder;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// A protocol error: POSIX-style code plus a short printable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GErr {
    pub code: &'static str,
    pub msg: String,
}

pub(crate) fn err(code: &'static str, msg: impl Into<String>) -> GErr {
    GErr { code, msg: msg.into() }
}

/// Decoded pixels, always RGBA8.
pub(crate) struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The single, identical error for every failure to read an image file, so a
/// sender learns nothing about files it cannot read itself.
fn read_failure() -> GErr {
    err("EBADF", "Failed to read image file")
}

/// Decode a command's payload into pixels. `memory_limit` bounds any single
/// allocation we are willing to make on behalf of the client.
pub(crate) fn load(cmd: &Cmd, payload: &[u8], memory_limit: usize) -> Result<Decoded, GErr> {
    if cmd.format == 8 || cmd.format == 16 {
        return Err(err("EINVAL", "Indexed color formats are not supported"));
    }
    if !matches!(cmd.format, 24 | 32 | 100) {
        return Err(err("EINVAL", "Unsupported image format"));
    }
    if let Some(o) = cmd.compression {
        if o != b'z' {
            return Err(err("EINVAL", "Unsupported compression"));
        }
    }
    let raw: Vec<u8> = match cmd.medium {
        b'd' => b64::decode(payload).ok_or_else(|| err("EINVAL", "Invalid base64 data"))?,
        b'f' | b't' => read_file(cmd, payload, memory_limit)?,
        b's' => return Err(err("EINVAL", "Shared memory transmission is not supported")),
        _ => return Err(err("EINVAL", "Unknown transmission medium")),
    };
    if raw.is_empty() {
        return Err(err("ENODATA", "No image data"));
    }

    let bpp: u64 = match cmd.format {
        24 => 3,
        _ => 4,
    };
    let raw_format = cmd.format != 100;
    if raw_format {
        if cmd.width == 0 || cmd.height == 0 {
            return Err(err("EINVAL", "Zero width or height"));
        }
        if cmd.width > MAX_DIMENSION || cmd.height > MAX_DIMENSION {
            return Err(err("EFBIG", "Image too large"));
        }
    }
    let expected: u64 = cmd.width as u64 * cmd.height as u64 * bpp;
    if raw_format && cmd.width as u64 * cmd.height as u64 * 4 > memory_limit as u64 {
        return Err(err("ENOSPC", "Image exceeds the storage quota"));
    }

    // Decompression.
    let data = if cmd.compression == Some(b'z') {
        let limit: u64 = if raw_format {
            expected
        } else if cmd.size > 0 && cmd.medium == b'd' {
            cmd.size as u64
        } else {
            memory_limit as u64
        };
        let mut out = Vec::new();
        let mut dec = ZlibDecoder::new(&raw[..]).take(limit.saturating_add(1));
        dec.read_to_end(&mut out)
            .map_err(|_| err("EINVAL", "Failed to inflate image data"))?;
        if out.len() as u64 > limit {
            return Err(err("EFBIG", "Too much image data"));
        }
        out
    } else {
        raw
    };

    if raw_format {
        let got = data.len() as u64;
        if got < expected {
            return Err(err("ENODATA", format!("Insufficient image data: {got} < {expected}")));
        }
        if got > expected {
            return Err(err("EFBIG", format!("Too much image data: {got} > {expected}")));
        }
        let rgba = if bpp == 4 {
            data
        } else {
            let mut v = Vec::with_capacity(data.len() / 3 * 4);
            for px in data.as_chunks::<3>().0 {
                v.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            v
        };
        return Ok(Decoded { width: cmd.width, height: cmd.height, rgba });
    }

    decode_png(&data, memory_limit)
}

fn decode_png(data: &[u8], memory_limit: usize) -> Result<Decoded, GErr> {
    let bad = || err("EINVAL", "Failed to decode PNG data");
    let reader = image::ImageReader::with_format(Cursor::new(data), image::ImageFormat::Png);
    let (w, h) = reader.into_dimensions().map_err(|_| bad())?;
    if w == 0 || h == 0 {
        return Err(bad());
    }
    if w > MAX_DIMENSION || h > MAX_DIMENSION {
        return Err(err("EFBIG", "Image too large"));
    }
    if (w as u64) * (h as u64) * 4 > memory_limit as u64 {
        return Err(err("ENOSPC", "Image exceeds the storage quota"));
    }
    let img = image::load_from_memory_with_format(data, image::ImageFormat::Png)
        .map_err(|_| bad())?
        .into_rgba8();
    let (width, height) = img.dimensions();
    Ok(Decoded { width, height, rgba: img.into_raw() })
}

// ---------------------------------------------------------------------------
// File media
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn is_sensitive(p: &Path) -> bool {
    if p.starts_with("/dev/shm") {
        return false;
    }
    ["/proc", "/sys", "/dev"].iter().any(|d| p.starts_with(d))
}

#[cfg(not(unix))]
fn is_sensitive(_p: &Path) -> bool {
    false
}

fn temp_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut push = |p: PathBuf| {
        if let Ok(c) = std::fs::canonicalize(&p) {
            roots.push(c);
        }
    };
    push(std::env::temp_dir());
    #[cfg(unix)]
    {
        push(PathBuf::from("/tmp"));
        push(PathBuf::from("/dev/shm"));
    }
    roots
}

/// Whether `canonical` may be deleted/read as a `t=t` temp file.
fn is_valid_temp(canonical: &Path) -> bool {
    canonical.to_string_lossy().contains("tty-graphics-protocol")
        && temp_roots().iter().any(|r| canonical.starts_with(r))
}

fn read_file(cmd: &Cmd, payload: &[u8], memory_limit: usize) -> Result<Vec<u8>, GErr> {
    let path_bytes = b64::decode(payload).ok_or_else(read_failure)?;
    let path_str = String::from_utf8(path_bytes).map_err(|_| read_failure())?;
    if path_str.is_empty() || path_str.contains('\0') {
        return Err(read_failure());
    }
    // Resolve symlinks (and fail on loops / missing files) *before* opening.
    let canonical = std::fs::canonicalize(&path_str).map_err(|_| read_failure())?;
    if is_sensitive(&canonical) {
        return Err(read_failure());
    }
    let temp = cmd.medium == b't';
    if temp && !is_valid_temp(&canonical) {
        return Err(read_failure());
    }
    let meta = std::fs::metadata(&canonical).map_err(|_| read_failure())?;
    if !meta.is_file() {
        return Err(read_failure());
    }
    let result = read_regular(&canonical, cmd, memory_limit);
    if temp {
        let _ = std::fs::remove_file(&canonical);
    }
    result
}

fn read_regular(path: &Path, cmd: &Cmd, memory_limit: usize) -> Result<Vec<u8>, GErr> {
    let mut f = std::fs::File::open(path).map_err(|_| read_failure())?;
    // Re-check on the opened handle (guards against swap-after-check).
    if !f.metadata().map_err(|_| read_failure())?.is_file() {
        return Err(read_failure());
    }
    if cmd.offset > 0 {
        f.seek(SeekFrom::Start(cmd.offset as u64)).map_err(|_| read_failure())?;
    }
    let mut out = Vec::new();
    if cmd.size > 0 {
        let want = cmd.size as u64;
        if want > memory_limit as u64 {
            return Err(read_failure());
        }
        f.take(want).read_to_end(&mut out).map_err(|_| read_failure())?;
        if (out.len() as u64) < want {
            return Err(read_failure());
        }
    } else {
        f.take((memory_limit as u64).saturating_add(1)).read_to_end(&mut out).map_err(|_| read_failure())?;
        if out.len() > memory_limit {
            return Err(read_failure());
        }
    }
    Ok(out)
}
