//! Tolerant base64 decoder (standard and URL-safe alphabets, padding optional,
//! ASCII whitespace ignored). Kitty clients are not always strict about
//! padding, so we accept what a lenient terminal would accept.

fn val(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    }
}

/// Decode `input`. Returns `None` on an invalid character or impossible length.
pub fn decode(input: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() / 4 * 3 + 3);
    let mut acc: u32 = 0;
    let mut nbits: u32 = 0;
    let mut seen_pad = false;
    let mut count: usize = 0;
    for &c in input {
        if c.is_ascii_whitespace() {
            continue;
        }
        if c == b'=' {
            seen_pad = true;
            continue;
        }
        if seen_pad {
            // data after padding
            return None;
        }
        let v = val(c)?;
        count += 1;
        acc = (acc << 6) | v as u32;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((acc >> nbits) as u8);
            acc &= (1 << nbits) - 1;
        }
    }
    // A single leftover sextet cannot encode a byte.
    if count % 4 == 1 {
        return None;
    }
    Some(out)
}

#[cfg(test)]
pub fn encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for ch in data.chunks(3) {
        let b = [ch[0], *ch.get(1).unwrap_or(&0), *ch.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if ch.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if ch.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all_lengths() {
        for len in 0..40usize {
            let data: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(decode(encode(&data).as_bytes()).unwrap(), data);
        }
    }

    #[test]
    fn unpadded_and_whitespace() {
        assert_eq!(decode(b"TWFu").unwrap(), b"Man");
        assert_eq!(decode(b"TWE").unwrap(), b"Ma");
        assert_eq!(decode(b"TW E=\n").unwrap(), b"Ma");
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"TW!u").is_none());
        assert!(decode(b"T").is_none());
        assert!(decode(b"TQ==TQ").is_none());
    }
}
