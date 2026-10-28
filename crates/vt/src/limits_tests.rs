//! Hostile-input limits: oversized string sequences and combining runs.

use crate::*;
use std::time::{Duration, Instant};

fn term() -> Terminal {
    Terminal::new(40, 5, 100)
}

fn big(prefix: &[u8], n: usize, suffix: &[u8]) -> Vec<u8> {
    let mut v = prefix.to_vec();
    v.resize(v.len() + n, b'A');
    v.extend_from_slice(suffix);
    v
}

fn feed_chunked(t: &mut Terminal, data: &[u8], chunk: usize) {
    for c in data.chunks(chunk) {
        t.advance(c);
        assert!(t.string_bytes_buffered() <= MAX_STRING_BODY);
    }
}

fn has_clip_set(t: &mut Terminal) -> bool {
    t.drain_events()
        .iter()
        .any(|e| matches!(e, TermEvent::ClipboardSet(_)))
}

#[test]
fn huge_osc52_is_dropped_quickly_and_bounded() {
    for terminator in [&b"\x07"[..], &b"\x1b\\"[..]] {
        let mut t = term();
        let data = big(b"\x1b]52;c;", 10 * 1024 * 1024, terminator);
        let start = Instant::now();
        feed_chunked(&mut t, &data, 64 * 1024);
        assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
        assert!(!has_clip_set(&mut t));
        assert_eq!(t.string_bytes_buffered(), 0);
        // Nothing leaked as text and the parser is back in ground state.
        t.advance(b"ok");
        assert_eq!(t.snapshot_lines()[0], "ok");
    }
}

#[test]
fn huge_osc52_single_chunk() {
    let mut t = term();
    let data = big(b"\x1b]52;c;", 10 * 1024 * 1024, b"\x07hi");
    let start = Instant::now();
    t.advance(&data);
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(!has_clip_set(&mut t));
    assert_eq!(t.snapshot_lines()[0], "hi");
}

#[test]
fn osc52_within_limit_still_works_and_over_clip_limit_rejected() {
    let mut t = term();
    let ok = big(b"\x1b]52;c;", 400_000, b"\x07"); // 'AAAA..' decodes fine
    t.advance(&ok);
    assert!(has_clip_set(&mut t));
    // Under the string cap (2 MiB) but over the clipboard cap (1 MiB).
    let over = big(b"\x1b]52;c;", 1_600_000, b"\x07");
    t.advance(&over);
    assert!(!has_clip_set(&mut t));
    t.advance(b"x");
    assert_eq!(t.snapshot_lines()[0], "x");
}

#[test]
fn huge_apc_ending_with_st_is_discarded_cleanly() {
    let mut t = term();
    let mut data = b"\x1b_G".to_vec();
    data.resize(data.len() + 10 * 1024 * 1024, b'B');
    data.extend_from_slice(b"\x1b\\after");
    let start = Instant::now();
    feed_chunked(&mut t, &data, 4096);
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(t
        .drain_events()
        .iter()
        .all(|e| !matches!(e, TermEvent::GraphicsCommand(_))));
    assert_eq!(t.snapshot_lines()[0], "after");
    assert_eq!(t.string_bytes_buffered(), 0);
    assert!(t.string_buffer_capacity() <= 64 * 1024);
}

#[test]
fn small_apc_g_still_delivered() {
    let mut t = term();
    t.advance(b"\x1b_Ga=q,i=1;AAAA\x1b\\z");
    let ev = t.drain_events();
    assert!(ev
        .iter()
        .any(|e| matches!(e, TermEvent::GraphicsCommand(p) if p == b"a=q,i=1;AAAA")));
    assert_eq!(t.snapshot_lines()[0], "z");
}

#[test]
fn huge_dcs_sos_pm_are_discarded() {
    for intro in [&b"\x1bP"[..], &b"\x1bX"[..], &b"\x1b^"[..], &b"\x1b]0;"[..]] {
        let mut t = term();
        let data = big(intro, 5 * 1024 * 1024, b"\x1b\\ok");
        feed_chunked(&mut t, &data, 100_000);
        assert_eq!(t.snapshot_lines()[0], "ok", "intro {intro:?}");
        assert!(t
            .drain_events()
            .iter()
            .all(|e| !matches!(e, TermEvent::Title(_))));
    }
}

#[test]
fn overflowing_string_aborted_by_can_or_new_escape() {
    let mut t = term();
    t.advance(&big(b"\x1b]52;c;", 3 * 1024 * 1024, b"\x18ok"));
    assert_eq!(t.snapshot_lines()[0], "ok");

    let mut t = term();
    // An ESC that is not part of ST aborts the string and starts a CSI.
    t.advance(&big(b"\x1bP", 3 * 1024 * 1024, b"\x1b[2;3Hxy"));
    assert_eq!(t.snapshot_lines()[1], "  xy");

    // The same thing split across chunks right at the ESC.
    let mut t = term();
    t.advance(&big(b"\x1b]0;", 3 * 1024 * 1024, b"\x1b"));
    t.advance(b"[2;3Hxy");
    assert_eq!(t.snapshot_lines()[1], "  xy");
}

#[test]
fn unterminated_oversized_string_swallows_until_terminator() {
    let mut t = term();
    t.advance(&big(b"\x1b]52;c;", 3 * 1024 * 1024, b""));
    t.advance(b"still inside the string");
    assert_eq!(t.snapshot_lines()[0], "");
    t.advance(b"\x07back");
    assert_eq!(t.snapshot_lines()[0], "back");
}

#[test]
fn normal_osc_under_limit_unaffected() {
    let mut t = term();
    t.advance(b"\x1b]0;hello world\x07");
    assert!(t
        .drain_events()
        .iter()
        .any(|e| matches!(e, TermEvent::Title(s) if s == "hello world")));
    // Large but legal title still works.
    let title = big(b"\x1b]2;", 200_000, b"\x1b\\");
    t.advance(&title);
    assert!(t
        .drain_events()
        .iter()
        .any(|e| matches!(e, TermEvent::Title(s) if s.len() == 200_000)));
}

#[test]
fn combining_marks_capped_per_cell() {
    let mut t = term();
    let mut s = String::from("a");
    for _ in 0..200 {
        s.push('\u{301}');
    }
    s.push('b');
    t.advance(s.as_bytes());
    let c = t.grid().cell(0, 0);
    assert_eq!(c.ch, 'a');
    assert_eq!(c.combining.len(), MAX_COMBINING);
    assert_eq!(t.grid().cell(0, 1).ch, 'b');
    assert!(t.grid().cell(0, 2).ch == ' ');
}
