//! Parsing of the control-data section (`k=v,k=v`) of a graphics command.

/// A parsed graphics command (control data only; the payload is kept apart).
#[derive(Debug, Clone)]
pub(crate) struct Cmd {
    pub action: Option<u8>,
    pub quiet: u8,
    pub has_quiet: bool,
    pub format: u32,
    pub medium: u8,
    /// `s`
    pub width: u32,
    /// `v`
    pub height: u32,
    /// `S`
    pub size: u32,
    /// `O`
    pub offset: u32,
    /// `i`
    pub id: u32,
    /// `I`
    pub number: u32,
    /// `p`
    pub pid: u32,
    pub compression: Option<u8>,
    pub more: bool,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// `X`
    pub xoff: u32,
    /// `Y`
    pub yoff: u32,
    pub cols: u32,
    pub rows: u32,
    /// `C`
    pub cursor_policy: u32,
    /// `U`
    pub virt: u32,
    pub z: i32,
    pub delete: Option<u8>,
    pub feature: Option<u8>,
    /// `P`
    pub parent_image: u32,
    /// `Q`
    pub parent_placement: u32,
    /// True if any key (other than `i`/`I`) that only makes sense on the
    /// first chunk of a transmission was present.
    pub has_transmit_keys: bool,
    /// First parse problem, if any.
    pub bad: Option<String>,
}

impl Default for Cmd {
    fn default() -> Self {
        Cmd {
            action: None,
            quiet: 0,
            has_quiet: false,
            format: 32,
            medium: b'd',
            width: 0,
            height: 0,
            size: 0,
            offset: 0,
            id: 0,
            number: 0,
            pid: 0,
            compression: None,
            more: false,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            xoff: 0,
            yoff: 0,
            cols: 0,
            rows: 0,
            cursor_policy: 0,
            virt: 0,
            z: 0,
            delete: None,
            feature: None,
            parent_image: 0,
            parent_placement: 0,
            has_transmit_keys: false,
            bad: None,
        }
    }
}

impl Cmd {
    /// The effective action (`t` when absent).
    pub fn action_or_default(&self) -> u8 {
        self.action.unwrap_or(b't')
    }
}

/// Split a raw APC payload into `(control data, base64 payload)`.
pub(crate) fn split_payload(raw: &[u8]) -> (&[u8], &[u8]) {
    match raw.iter().position(|&b| b == b';') {
        Some(i) => (&raw[..i], &raw[i + 1..]),
        None => (raw, &[]),
    }
}

/// Parse control data. Never fails; problems are recorded in `Cmd::bad` so
/// that the caller can still answer with whatever ids were understood.
pub(crate) fn parse(ctl: &[u8]) -> Cmd {
    let mut c = Cmd::default();
    for part in ctl.split(|&b| b == b',') {
        if part.is_empty() {
            continue;
        }
        let Some(eq) = part.iter().position(|&b| b == b'=') else {
            bad(&mut c, "Malformed key=value pair");
            continue;
        };
        let (k, v) = (&part[..eq], &part[eq + 1..]);
        if k.len() != 1 {
            // Unknown multi-char keys are ignored, like unknown keys.
            continue;
        }
        let key = k[0];
        match key {
            b'a' | b't' | b'o' | b'd' | b'e' => {
                if v.len() != 1 || !v[0].is_ascii_alphanumeric() {
                    bad(&mut c, "Expected a single character value");
                    continue;
                }
                let ch = v[0];
                match key {
                    b'a' => c.action = Some(ch),
                    b't' => {
                        c.medium = ch;
                        c.has_transmit_keys = true;
                    }
                    b'o' => {
                        c.compression = Some(ch);
                        c.has_transmit_keys = true;
                    }
                    b'd' => c.delete = Some(ch),
                    _ => c.feature = Some(ch),
                }
            }
            b'z' | b'H' | b'V' => match parse_i32(v) {
                Some(n) => {
                    if key == b'z' {
                        c.z = n;
                    }
                }
                None => bad(&mut c, "Invalid signed integer"),
            },
            b'q' | b'f' | b's' | b'v' | b'S' | b'O' | b'i' | b'I' | b'p' | b'm' | b'x' | b'y'
            | b'w' | b'h' | b'X' | b'Y' | b'c' | b'r' | b'C' | b'U' | b'P' | b'Q' | b'N' => {
                let Some(n) = parse_u32(v) else {
                    bad(&mut c, "Invalid unsigned integer");
                    continue;
                };
                match key {
                    b'q' => {
                        if n > 2 {
                            bad(&mut c, "Invalid quiet level");
                        } else {
                            c.quiet = n as u8;
                            c.has_quiet = true;
                        }
                    }
                    b'f' => {
                        c.format = n;
                        c.has_transmit_keys = true;
                    }
                    b's' => {
                        c.width = n;
                        c.has_transmit_keys = true;
                    }
                    b'v' => {
                        c.height = n;
                        c.has_transmit_keys = true;
                    }
                    b'S' => {
                        c.size = n;
                        c.has_transmit_keys = true;
                    }
                    b'O' => {
                        c.offset = n;
                        c.has_transmit_keys = true;
                    }
                    b'i' => c.id = n,
                    b'I' => c.number = n,
                    b'p' => c.pid = n,
                    b'm' => c.more = n == 1,
                    b'x' => c.x = n,
                    b'y' => c.y = n,
                    b'w' => c.w = n,
                    b'h' => c.h = n,
                    b'X' => c.xoff = n,
                    b'Y' => c.yoff = n,
                    b'c' => c.cols = n,
                    b'r' => c.rows = n,
                    b'C' => c.cursor_policy = n,
                    b'U' => c.virt = n,
                    b'P' => c.parent_image = n,
                    b'Q' => c.parent_placement = n,
                    _ => {}
                }
            }
            _ => {} // unknown keys are ignored
        }
    }
    c
}

fn bad(c: &mut Cmd, msg: &str) {
    if c.bad.is_none() {
        c.bad = Some(msg.to_string());
    }
}

fn parse_u32(v: &[u8]) -> Option<u32> {
    if v.is_empty() || v.len() > 10 || !v.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(v).ok()?.parse::<u64>().ok()?.try_into().ok()
}

fn parse_i32(v: &[u8]) -> Option<i32> {
    let (neg, digits) = match v.first() {
        Some(b'-') => (true, &v[1..]),
        _ => (false, v),
    };
    if digits.is_empty() || digits.len() > 11 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let n: i64 = std::str::from_utf8(digits).ok()?.parse().ok()?;
    let n = if neg { -n } else { n };
    i32::try_from(n).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_keys() {
        let c = parse(b"a=T,f=100,i=7,p=3,z=-5,c=10,r=2,C=1,q=2,m=1");
        assert_eq!(c.action, Some(b'T'));
        assert_eq!(c.format, 100);
        assert_eq!((c.id, c.pid, c.z), (7, 3, -5));
        assert_eq!((c.cols, c.rows, c.cursor_policy), (10, 2, 1));
        assert_eq!(c.quiet, 2);
        assert!(c.more && c.bad.is_none());
    }

    #[test]
    fn defaults_and_stray_commas() {
        let c = parse(b",,a=p,,");
        assert_eq!(c.format, 32);
        assert_eq!(c.medium, b'd');
        assert!(c.bad.is_none());
    }

    #[test]
    fn flags_bad_values() {
        assert!(parse(b"i=abc").bad.is_some());
        assert!(parse(b"i=4294967296").bad.is_some());
        assert!(parse(b"z=2147483648").bad.is_some());
        assert!(parse(b"a=xy").bad.is_some());
        assert!(parse(b"q=3").bad.is_some());
        assert!(parse(b"junk").bad.is_some());
        assert!(parse(b"z=-2147483648").bad.is_none());
    }

    #[test]
    fn split() {
        assert_eq!(split_payload(b"a=t;AAAA"), (&b"a=t"[..], &b"AAAA"[..]));
        assert_eq!(split_payload(b"a=t"), (&b"a=t"[..], &b""[..]));
    }
}
