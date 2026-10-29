//! Turning a transmission command plus payload into RGBA pixels: base64,
//! file/temp-file media, zlib, raw RGB/RGBA and PNG.

use crate::cmd::Cmd;
use crate::{b64, MAX_DIMENSION};
use flate2::read::ZlibDecoder;
use image::codecs::png::PngDecoder;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A protocol error: POSIX-style code plus a short printable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GErr {
    pub code: &'static str,
    pub msg: String,
}

pub(crate) fn err(code: &'static str, msg: impl Into<String>) -> GErr {
    GErr { code, msg: msg.into() }
}

/// Decoded pixels, always RGBA8. The buffer is allocated once, at its final
/// size and in its final type, so storing it never copies it again.
pub(crate) struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

/// The single, identical error for every failure to read an image file, so a
/// sender learns nothing about files it cannot read itself.
fn read_failure() -> GErr {
    err("EBADF", "Failed to read image file")
}

/// Largest ratio between deflate output and input (RFC 1951: 1032:1).
const MAX_INFLATE_RATIO: usize = 1032;

/// A zeroed RGBA buffer of `bytes` bytes, allocated exactly once.
fn rgba_buffer(bytes: usize) -> Arc<[u8]> {
    std::iter::repeat_n(0u8, bytes).collect()
}

/// Mutable access to a buffer from [`rgba_buffer`] (always uniquely owned).
fn buf_mut(a: &mut Arc<[u8]>) -> &mut [u8] {
    Arc::get_mut(a).expect("freshly created Arc is unique")
}

/// Read until `buf` is full or the stream ends cleanly; returns the count.
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// Decode a command's payload into pixels. `memory_limit` bounds any single
/// allocation we are willing to make on behalf of the client.
///
/// For file media (`t=f`, `t=t`) every failure, whatever its cause, is the
/// same `EBADF`: otherwise the size/validity errors would let a program that
/// can only write to the terminal (an ssh session, say) probe the existence,
/// size and type of local files it cannot read itself.
///
/// `make_room(n)` is called, for raw formats only, once the data has been
/// proven to be exactly the declared size and immediately before the final
/// `n`-byte buffer is allocated: the store evicts then, so that the old images
/// and the new one are not resident together, yet a malformed transmission
/// never evicts anything.
pub(crate) fn load(
    cmd: &Cmd,
    payload: &[u8],
    memory_limit: usize,
    make_room: &mut dyn FnMut(usize),
) -> Result<Decoded, GErr> {
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
    let result = decode(cmd, raw, memory_limit, make_room);
    if matches!(cmd.medium, b'f' | b't') {
        return result.map_err(|_| read_failure());
    }
    result
}

fn decode(cmd: &Cmd, raw: Vec<u8>, memory_limit: usize, make_room: &mut dyn FnMut(usize)) -> Result<Decoded, GErr> {
    if raw.is_empty() {
        return Err(err("ENODATA", "No image data"));
    }
    if cmd.format == 100 {
        let data = if cmd.compression == Some(b'z') {
            let limit: u64 = if cmd.size > 0 && cmd.medium == b'd' { cmd.size as u64 } else { memory_limit as u64 };
            inflate_all(&raw, limit)?
        } else {
            raw
        };
        return decode_png(&data, memory_limit);
    }

    // Raw RGB / RGBA. Sizes are only computed once both dimensions are known
    // to be small, so none of the arithmetic below can overflow.
    let bpp: usize = if cmd.format == 24 { 3 } else { 4 };
    if cmd.width == 0 || cmd.height == 0 {
        return Err(err("EINVAL", "Zero width or height"));
    }
    if cmd.width > MAX_DIMENSION || cmd.height > MAX_DIMENSION {
        return Err(err("EFBIG", "Image too large"));
    }
    let pixels = cmd.width as u64 * cmd.height as u64;
    if pixels * 4 > memory_limit as u64 {
        return Err(err("ENOSPC", "Image exceeds the storage quota"));
    }
    let expected = pixels * bpp as u64;
    let rgba = if cmd.compression == Some(b'z') {
        // First pass: prove the stream inflates to exactly the right size,
        // using no memory to speak of. Then make room and inflate for real.
        check_inflated_len(ZlibDecoder::new(&raw[..]), expected)?;
        make_room(pixels as usize * 4);
        fill_rgba(&mut ZlibDecoder::new(&raw[..]), pixels as usize, bpp)?
    } else {
        let got = raw.len() as u64;
        if got < expected {
            return Err(err("ENODATA", format!("Insufficient image data: {got} < {expected}")));
        }
        if got > expected {
            return Err(err("EFBIG", format!("Too much image data: {got} > {expected}")));
        }
        make_room(pixels as usize * 4);
        fill_rgba(&mut &raw[..], pixels as usize, bpp)?
    };
    Ok(Decoded { width: cmd.width, height: cmd.height, rgba })
}

/// Run a decoder to its end, checking it yields exactly `expected` bytes.
fn check_inflated_len(mut dec: impl Read, expected: u64) -> Result<(), GErr> {
    let mut buf = [0u8; 32 * 1024];
    let mut total = 0u64;
    loop {
        match dec.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                total += n as u64;
                if total > expected {
                    return Err(err("EFBIG", "Too much image data"));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err(err("EINVAL", "Failed to inflate image data")),
        }
    }
    if total < expected {
        return Err(err("ENODATA", format!("Insufficient image data: {total} < {expected}")));
    }
    Ok(())
}

/// Inflate a whole zlib stream, refusing more than `limit` output bytes.
fn inflate_all(raw: &[u8], limit: u64) -> Result<Vec<u8>, GErr> {
    // The output cannot exceed ratio * input, so reserving that (virtual,
    // untouched) capacity up front avoids the doubling-and-copying growth.
    let cap = raw
        .len()
        .saturating_mul(MAX_INFLATE_RATIO)
        .saturating_add(64)
        .min(usize::try_from(limit.saturating_add(1)).unwrap_or(usize::MAX));
    let mut out = Vec::with_capacity(cap);
    let mut dec = ZlibDecoder::new(raw).take(limit.saturating_add(1));
    dec.read_to_end(&mut out).map_err(|_| err("EINVAL", "Failed to inflate image data"))?;
    if out.len() as u64 > limit {
        return Err(err("EFBIG", "Too much image data"));
    }
    Ok(out)
}

/// Stream `pixels` pixels of `bpp` (3 or 4) bytes each from `src` straight
/// into a single RGBA allocation.
fn fill_rgba(src: &mut impl Read, pixels: usize, bpp: usize) -> Result<Arc<[u8]>, GErr> {
    let total = pixels * 4;
    let mut arc = rgba_buffer(total);
    let dst = buf_mut(&mut arc);
    let inflate = |_| err("EINVAL", "Failed to inflate image data");
    let insufficient =
        |got: usize| err("ENODATA", format!("Insufficient image data: {} < {}", got, pixels * bpp));
    if bpp == 4 {
        let n = read_full(src, dst).map_err(inflate)?;
        if n < total {
            return Err(insufficient(n));
        }
    } else {
        const CHUNK_PX: usize = 16 * 1024;
        let mut buf = vec![0u8; CHUNK_PX * 3];
        let mut done = 0usize;
        while done < pixels {
            let px = CHUNK_PX.min(pixels - done);
            let n = read_full(src, &mut buf[..px * 3]).map_err(inflate)?;
            if n < px * 3 {
                return Err(insufficient(done * 3 + n));
            }
            for (d, s) in dst[done * 4..(done + px) * 4].chunks_exact_mut(4).zip(buf.chunks_exact(3)) {
                d.copy_from_slice(&[s[0], s[1], s[2], 255]);
            }
            done += px;
        }
    }
    Ok(arc)
}

/// Expand an `N`-bytes-per-pixel 8-bit image into a fresh RGBA buffer.
fn expand<const N: usize>(src: &[u8], total: usize, f: impl Fn(&[u8]) -> [u8; 4]) -> Arc<[u8]> {
    let mut arc = rgba_buffer(total);
    for (d, s) in buf_mut(&mut arc).chunks_exact_mut(4).zip(src.chunks_exact(N)) {
        d.copy_from_slice(&f(s));
    }
    arc
}

fn decode_png(data: &[u8], memory_limit: usize) -> Result<Decoded, GErr> {
    use image::{DynamicImage, ImageDecoder};
    let bad = || err("EINVAL", "Failed to decode PNG data");
    let mut dec = PngDecoder::new(Cursor::new(data)).map_err(|_| bad())?;
    let (w, h) = dec.dimensions();
    if w == 0 || h == 0 {
        return Err(bad());
    }
    if w > MAX_DIMENSION || h > MAX_DIMENSION {
        return Err(err("EFBIG", "Image too large"));
    }
    if (w as u64) * (h as u64) * 4 > memory_limit as u64 {
        return Err(err("ENOSPC", "Image exceeds the storage quota"));
    }
    let mut limits = image::Limits::default();
    limits.max_alloc = Some((memory_limit as u64).saturating_mul(2).min(1 << 30));
    dec.set_limits(limits).map_err(|_| err("ENOSPC", "Image exceeds the storage quota"))?;
    let total = w as usize * h as usize * 4;

    if dec.color_type() == image::ColorType::Rgba8 {
        // Decode directly into the buffer that will be stored.
        let mut arc = rgba_buffer(total);
        dec.read_image(buf_mut(&mut arc)).map_err(|_| bad())?;
        return Ok(Decoded { width: w, height: h, rgba: arc });
    }
    let img = DynamicImage::from_decoder(dec).map_err(|_| bad())?;
    // Expand the common 8-bit layouts straight into the final buffer; only
    // the exotic ones go through an intermediate RGBA image.
    let rgba: Arc<[u8]> = match img {
        DynamicImage::ImageLuma8(b) => expand::<1>(b.as_raw(), total, |s| [s[0], s[0], s[0], 255]),
        DynamicImage::ImageLumaA8(b) => expand::<2>(b.as_raw(), total, |s| [s[0], s[0], s[0], s[1]]),
        DynamicImage::ImageRgb8(b) => expand::<3>(b.as_raw(), total, |s| [s[0], s[1], s[2], 255]),
        other => Arc::from(other.into_rgba8().into_raw()),
    };
    Ok(Decoded { width: w, height: h, rgba })
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
