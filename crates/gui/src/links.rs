//! Hyperlink detection under the mouse: OSC 8 links (cells carrying a
//! `hyperlink_id`) and plain-text `http(s)://` URLs, plus opening them.
//!
//! Everything works on the visible viewport (rows are followed across soft
//! wraps, so a URL that wraps onto the next row is one link).

use grimalkin_vt::{CellWidth, Terminal};

use crate::selection::CellPos;

/// A link under the pointer: the cell segments to underline and the target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkSpan {
    /// `(visible row, first col, last col)` inclusive, in reading order.
    pub segments: Vec<(usize, usize, usize)>,
    pub uri: String,
}

/// Link at `pos`, if any. OSC 8 links win over detected URLs.
pub fn link_at(term: &Terminal, pos: CellPos) -> Option<LinkSpan> {
    if pos.row >= term.rows() || pos.col >= term.cols() {
        return None;
    }
    osc8_link(term, pos).or_else(|| url_link(term, pos))
}

fn osc8_link(term: &Terminal, pos: CellPos) -> Option<LinkSpan> {
    let id = term.visible_row(pos.row).cells.get(pos.col)?.hyperlink_id?;
    let uri = term.hyperlink_uri(id)?.to_string();
    let rows = term.rows();
    // Maximal run of cells carrying `id` around `col` in visible row `r`.
    let run_at = |r: usize, col: usize| {
        let cells = &term.visible_row(r).cells;
        let (mut s, mut e) = (col, col);
        while s > 0 && cells[s - 1].hyperlink_id == Some(id) {
            s -= 1;
        }
        while e + 1 < cells.len() && cells[e + 1].hyperlink_id == Some(id) {
            e += 1;
        }
        (s, e)
    };
    let has_id =
        |r: usize, col: usize| term.visible_row(r).cells.get(col).is_some_and(|c| c.hyperlink_id == Some(id));

    let (s, e) = run_at(pos.row, pos.col);
    let mut segments = vec![(pos.row, s, e)];
    // Follow the link backwards over soft wraps.
    let (mut r, mut start) = (pos.row, s);
    while start == 0 && r > 0 && term.visible_row(r - 1).wrapped {
        let last = term.visible_row(r - 1).cells.len().saturating_sub(1);
        if !has_id(r - 1, last) {
            break;
        }
        r -= 1;
        let (a, b) = run_at(r, last);
        segments.insert(0, (r, a, b));
        start = a;
    }
    // And forwards.
    let (mut r, mut end) = (pos.row, e);
    while r + 1 < rows
        && term.visible_row(r).wrapped
        && end + 1 == term.visible_row(r).cells.len()
        && has_id(r + 1, 0)
    {
        r += 1;
        let (a, b) = run_at(r, 0);
        segments.push((r, a, b));
        end = b;
    }
    Some(LinkSpan { segments, uri })
}

fn url_link(term: &Terminal, pos: CellPos) -> Option<LinkSpan> {
    let rows = term.rows();
    let mut first = pos.row;
    while first > 0 && term.visible_row(first - 1).wrapped {
        first -= 1;
    }
    let mut last = pos.row;
    while last + 1 < rows && term.visible_row(last).wrapped {
        last += 1;
    }
    // Flatten to chars with their cell coordinates.
    let mut chars: Vec<char> = Vec::new();
    let mut at: Vec<(usize, usize)> = Vec::new();
    let mut idx = None;
    for r in first..=last {
        let row = term.visible_row(r);
        let next = (r < last).then(|| term.visible_row(r + 1));
        let pad = crate::selection::wide_wrap_pad(row, next);
        for (c, cell) in row.cells.iter().enumerate() {
            if cell.width == CellWidth::WideSpacer || (pad && c + 1 == row.cells.len()) {
                if r == pos.row && c == pos.col {
                    idx = Some(chars.len().saturating_sub(1));
                }
                continue;
            }
            if r == pos.row && c == pos.col {
                idx = Some(chars.len());
            }
            chars.push(if cell.ch == '\0' { ' ' } else { cell.ch });
            at.push((r, c));
        }
    }
    let (s, e) = find_url(&chars, idx?)?;
    let uri: String = chars[s..e].iter().collect();
    let mut segments: Vec<(usize, usize, usize)> = Vec::new();
    for &(r, c) in &at[s..e] {
        match segments.last_mut() {
            Some(seg) if seg.0 == r => seg.2 = c,
            _ => segments.push((r, c, c)),
        }
    }
    // Cover the right half of a trailing wide char.
    if let (Some(&(r, c)), Some(seg)) = (at.get(e - 1), segments.last_mut()) {
        if term.visible_row(r).cells.get(c).is_some_and(|x| x.width == CellWidth::Wide) {
            seg.2 = c + 1;
        }
    }
    Some(LinkSpan { segments, uri })
}

/// Longest stretch of characters inspected on each side of the pointer when
/// looking for a plain-text URL.
const MAX_URL_SPAN: usize = 2048;

/// Find the `http://` / `https://` URL containing `idx`; returns `[start, end)`.
pub fn find_url(chars: &[char], idx: usize) -> Option<(usize, usize)> {
    if idx >= chars.len() || is_url_stop(chars[idx]) {
        return None;
    }
    // Scan back to the start of the non-delimiter run, then look for a scheme
    // anywhere inside it that begins at or before `idx`.
    // Bounded: the scan looks at most MAX_URL_SPAN chars either side of
    // `idx`, so a line-sized run of URL characters cannot make this (called
    // every frame while the link modifier is held) cost more than a fixed
    // amount per call.
    let mut run_start = idx;
    while run_start > 0 && idx - run_start < MAX_URL_SPAN && !is_url_stop(chars[run_start - 1]) {
        run_start -= 1;
    }
    let mut run_end = idx + 1;
    while run_end < chars.len() && run_end - idx <= MAX_URL_SPAN && !is_url_stop(chars[run_end]) {
        run_end += 1;
    }
    let mut best: Option<(usize, usize)> = None;
    let mut i = run_start;
    while i < run_end {
        let scheme = [("http://", 7), ("https://", 8)]
            .iter()
            .find(|(s, _)| matches_at(chars, i, s, run_end))
            .map(|&(_, n)| n);
        if let Some(n) = scheme {
            if i + n < run_end && i <= idx {
                let end = trim_url_end(chars, i, run_end);
                if end > i + n && idx < end {
                    best = Some((i, end));
                }
            }
        }
        i += 1;
    }
    best
}

fn matches_at(chars: &[char], i: usize, s: &str, limit: usize) -> bool {
    let n = s.chars().count();
    i + n <= limit && chars[i..i + n].iter().zip(s.chars()).all(|(a, b)| a.eq_ignore_ascii_case(&b))
}

fn is_url_stop(c: char) -> bool {
    c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"' | '`' | '\u{0}') || c == '\u{a0}'
}

/// Drop trailing punctuation that is almost never part of the URL, keeping
/// closing brackets that balance an opening one inside the URL.
fn trim_url_end(chars: &[char], start: usize, mut end: usize) -> usize {
    // Bracket counts are taken once and adjusted as characters are dropped,
    // so trimming is linear however many closing brackets trail the URL.
    let mut count = [0usize; 6]; // ( ) [ ] { }
    let slot = |c: char| match c {
        '(' => Some(0),
        ')' => Some(1),
        '[' => Some(2),
        ']' => Some(3),
        '{' => Some(4),
        '}' => Some(5),
        _ => None,
    };
    for &c in &chars[start..end] {
        if let Some(i) = slot(c) {
            count[i] += 1;
        }
    }
    while end > start {
        let c = chars[end - 1];
        let drop = match c {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' => true,
            ')' | ']' | '}' => {
                let close = slot(c).unwrap_or(1);
                count[close] > count[close - 1]
            }
            _ => false,
        };
        if !drop {
            break;
        }
        if let Some(i) = slot(c) {
            count[i] -= 1;
        }
        end -= 1;
    }
    end
}

/// Only these schemes are opened; anything else (file:, javascript:, custom
/// app schemes) could run local code and is refused.
pub fn is_openable(uri: &str) -> bool {
    let Some((scheme, _)) = uri.split_once(':') else { return false };
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "ftp" | "ftps" | "mailto"
    ) && !uri.chars().any(|c| c.is_control() || c.is_whitespace() || crate::textsafe::is_bidi_or_separator(c))
}

/// `uri` for a log line: escaped and cut to a sane length, so a hostile
/// (up to 2 MiB, control-laden) OSC 8 target cannot inject log lines.
fn loggable(uri: &str) -> String {
    let cut: String = uri.chars().take(200).collect();
    let mut s = format!("{cut:?}");
    if uri.chars().nth(200).is_some() {
        s.push_str("...");
    }
    s
}

/// Open `uri` with the system handler.
pub fn open(uri: &str) {
    if !is_openable(uri) {
        log::warn!("refusing to open link with unsupported scheme or characters: {}", loggable(uri));
        return;
    }
    if let Err(e) = open::that_detached(uri) {
        log::warn!("cannot open {}: {e}", loggable(uri));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn url_in(s: &str, at: usize) -> Option<String> {
        let c = chars(s);
        find_url(&c, at).map(|(a, b)| c[a..b].iter().collect())
    }

    #[test]
    fn finds_plain_urls() {
        let s = "see https://example.com/a?b=c#d for more";
        assert_eq!(url_in(s, 6).as_deref(), Some("https://example.com/a?b=c#d"));
        assert_eq!(url_in(s, 30).as_deref(), Some("https://example.com/a?b=c#d"));
        assert_eq!(url_in(s, 1), None);
        assert_eq!(url_in(s, 3), None);
        assert_eq!(url_in(s, 36), None);
    }

    #[test]
    fn trims_trailing_punctuation() {
        assert_eq!(url_in("(http://a.org/x).", 3).as_deref(), Some("http://a.org/x"));
        assert_eq!(url_in("http://a.org/x, y", 3).as_deref(), Some("http://a.org/x"));
        assert_eq!(
            url_in("http://en.wikipedia.org/wiki/Rust_(language)", 3).as_deref(),
            Some("http://en.wikipedia.org/wiki/Rust_(language)")
        );
    }

    #[test]
    fn rejects_bare_scheme_and_other_schemes() {
        assert_eq!(url_in("http:// x", 2), None);
        assert_eq!(url_in("ftp://example.com", 5), None);
        assert_eq!(url_in("<https://a.b/c>", 5).as_deref(), Some("https://a.b/c"));
    }

    #[test]
    fn pathological_line_is_bounded() {
        // F13: 'http://' x300 then ')' x7900 was cubic (2.6 s at 200x50).
        let mut s = "http://".repeat(300);
        s.push_str(&")".repeat(7900));
        let c = chars(&s);
        let t = std::time::Instant::now();
        for at in [c.len() - 1, 2000, 7, 0] {
            let _ = find_url(&c, at);
        }
        assert!(t.elapsed().as_millis() < 500, "{:?}", t.elapsed());
        // Ordinary URLs inside the window are unaffected.
        assert_eq!(url_in("x http://a.b/c)).", 8).as_deref(), Some("http://a.b/c"));
    }

    #[test]
    fn rejected_uris_are_logged_escaped_and_truncated() {
        let l = loggable(&format!("evil\n\x1b[2J{}", "a".repeat(5000)));
        assert!(!l.contains('\n') && !l.contains('\x1b') && l.len() < 300, "{l}");
    }

    #[test]
    fn scheme_gate() {
        assert!(is_openable("https://x.y"));
        assert!(is_openable("mailto:a@b.c"));
        assert!(!is_openable("file:///etc/passwd"));
        assert!(!is_openable("javascript:alert(1)"));
        assert!(!is_openable("/usr/bin/true"));
        assert!(!is_openable("https://x\u{7}y"));
        // F45: spaces and bidi marks make a link read differently than it acts.
        assert!(!is_openable("https://x.y/a b"));
        assert!(!is_openable("https://x.y/\u{202e}gpj.exe"));
        assert!(is_openable("mailto:a@b.c?subject=hi"));
    }

    #[test]
    fn url_on_terminal_with_wrap() {
        let mut t = Terminal::new(12, 4, 10usize);
        t.advance(b"go https://example.com/path ok");
        // Wraps: "go https://e" / "xample.com/p" / "ath ok"
        let l = link_at(&t, CellPos { col: 5, row: 0 }).expect("link");
        assert_eq!(l.uri, "https://example.com/path");
        assert_eq!(l.segments, vec![(0, 3, 11), (1, 0, 11), (2, 0, 2)]);
        let l2 = link_at(&t, CellPos { col: 1, row: 2 }).expect("link on wrapped row");
        assert_eq!(l2.uri, l.uri);
        assert!(link_at(&t, CellPos { col: 0, row: 0 }).is_none());
        assert!(link_at(&t, CellPos { col: 4, row: 2 }).is_none());
    }

    #[test]
    fn wide_char_wrap_does_not_split_url() {
        // F38: the pad cell before a wrapped wide char used to read as a space.
        let mut t = Terminal::new(10, 4, 10usize);
        t.advance("http://a/\u{4e16}bc".as_bytes());
        let l = link_at(&t, CellPos { col: 2, row: 0 }).expect("link");
        assert_eq!(l.uri, "http://a/\u{4e16}bc");
    }

    #[test]
    fn osc8_link() {
        let mut t = Terminal::new(30, 3, 10usize);
        t.advance(b"a \x1b]8;;https://x.test/p\x1b\\click me\x1b]8;;\x1b\\ b");
        let l = link_at(&t, CellPos { col: 4, row: 0 }).expect("link");
        assert_eq!(l.uri, "https://x.test/p");
        assert_eq!(l.segments, vec![(0, 2, 9)]);
        assert!(link_at(&t, CellPos { col: 0, row: 0 }).is_none());
        assert!(link_at(&t, CellPos { col: 11, row: 0 }).is_none());
    }
}
