//! Small helpers: base64, colour spec parsing, file-URI parsing.

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(B64[(n >> 18 & 63) as usize] as char);
        out.push(B64[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

pub fn base64_decode(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for &c in data {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b'\r' | b'\n' | b' ' => continue,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Parse an X11-style colour spec: `rgb:R/G/B` (1-4 hex digits per channel)
/// or `#RGB`, `#RRGGBB`, `#RRRGGGBBB`, `#RRRRGGGGBBBB`.
pub fn parse_color_spec(s: &[u8]) -> Option<(u8, u8, u8)> {
    let s = std::str::from_utf8(s).ok()?;
    fn scale(h: &str) -> Option<u8> {
        if h.is_empty() || h.len() > 4 {
            return None;
        }
        let v = u32::from_str_radix(h, 16).ok()?;
        let max = (1u32 << (4 * h.len())) - 1;
        Some(((v * 255 + max / 2) / max) as u8)
    }
    if let Some(rest) = s.strip_prefix("rgb:") {
        let mut it = rest.split('/');
        let r = scale(it.next()?)?;
        let g = scale(it.next()?)?;
        let b = scale(it.next()?)?;
        if it.next().is_some() {
            return None;
        }
        Some((r, g, b))
    } else if let Some(hex) = s.strip_prefix('#') {
        if hex.is_empty() || hex.len() % 3 != 0 || hex.len() > 12 || !hex.is_ascii() {
            return None;
        }
        let n = hex.len() / 3;
        Some((
            scale(&hex[0..n])?,
            scale(&hex[n..2 * n])?,
            scale(&hex[2 * n..])?,
        ))
    } else {
        None
    }
}

/// Format as xterm does for colour query replies: `rgb:RRRR/GGGG/BBBB`.
pub fn format_color_spec(c: (u8, u8, u8)) -> String {
    format!(
        "rgb:{0:02x}{0:02x}/{1:02x}{1:02x}/{2:02x}{2:02x}",
        c.0, c.1, c.2
    )
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            if let Some(hex) = s.get(i + 1..i + 3) {
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Turn an OSC 7 value (`file://host/path`) into a plain path. Values that
/// are not `file://` URIs are returned unchanged.
pub fn parse_cwd_uri(uri: &str) -> String {
    if let Some(rest) = uri.strip_prefix("file://") {
        let path = match rest.find('/') {
            Some(i) => &rest[i..],
            None => return percent_decode(rest),
        };
        let mut p = percent_decode(path);
        // /C:/Users/x -> C:/Users/x
        let b = p.as_bytes();
        if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
            p.remove(0);
        }
        p
    } else {
        uri.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip() {
        for s in ["", "f", "fo", "foo", "foob", "fooba", "foobar", "héllo wörld"] {
            let e = base64_encode(s.as_bytes());
            assert_eq!(base64_decode(e.as_bytes()).unwrap(), s.as_bytes());
        }
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert!(base64_decode(b"!!!").is_none());
    }

    #[test]
    fn color_specs() {
        assert_eq!(parse_color_spec(b"rgb:ff/00/80"), Some((255, 0, 128)));
        assert_eq!(parse_color_spec(b"rgb:ffff/0000/8080"), Some((255, 0, 128)));
        assert_eq!(parse_color_spec(b"#ff0080"), Some((255, 0, 128)));
        assert_eq!(parse_color_spec(b"#f08"), Some((255, 0, 136)));
        assert_eq!(parse_color_spec(b"rgb:f/f/f"), Some((255, 255, 255)));
        assert_eq!(parse_color_spec(b"bogus"), None);
        assert_eq!(parse_color_spec(b"rgb:1/2"), None);
        assert_eq!(format_color_spec((1, 2, 255)), "rgb:0101/0202/ffff");
    }

    #[test]
    fn cwd_uris() {
        assert_eq!(parse_cwd_uri("file://host/home/me/a%20b"), "/home/me/a b");
        assert_eq!(parse_cwd_uri("file:///C:/Users/me"), "C:/Users/me");
        assert_eq!(parse_cwd_uri("/plain"), "/plain");
        assert_eq!(parse_cwd_uri("file:///x%"), "/x%");
    }
}
