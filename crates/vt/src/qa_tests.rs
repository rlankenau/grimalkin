//! Regression tests for the adversarial-QA findings (F5-F8, F18, F23, F24,
//! F30, F34-F36, F39, F42): work bounds, memory bounds and parser agreement.

use crate::term::{MAX_HYPERLINKS, MAX_HYPERLINK_BYTES, MAX_HYPERLINK_URI};
use crate::*;
use std::time::{Duration, Instant};

fn feed(t: &mut Terminal, s: &str) {
    t.advance(s.as_bytes());
}

/// Fail if `f` takes longer than `limit` (generous: debug builds, CI).
fn within(limit: Duration, f: impl FnOnce()) {
    let start = Instant::now();
    f();
    assert!(start.elapsed() < limit, "took {:?}", start.elapsed());
}

// ------------------------------------------------------------------ F5

#[test]
fn rep_is_clamped_to_a_screenful_per_sequence() {
    let mut t = Terminal::new(80, 24, 10_000);
    feed(&mut t, "a\x1b[65535b");
    // 1920 cells = 24 rows: at most ~24 lines scrolled, not 819.
    assert!(t.grid().scrolled_total() <= 25, "{}", t.grid().scrolled_total());
}

#[test]
fn rep_flood_has_bounded_total_work() {
    for ch in ["a", "\u{4e2d}"] {
        let mut t = Terminal::new(80, 24, 10_000);
        let data = format!("{ch}\x1b[65535b").repeat(100_000);
        within(Duration::from_secs(10), || t.advance(data.as_bytes()));
        // Output cells are bounded by the credit earned from the input (8 per
        // byte plus the bucket), not 65535 per sequence (~82M rows).
        let cells = t.grid().scrolled_total() * 80;
        assert!(cells < 20 * data.len() as u64 + (4 << 20), "{cells}");
    }
}

// ------------------------------------------------------------------ F6

#[test]
fn cht_cbt_stop_when_the_column_stops_changing() {
    let mut t = Terminal::new(80, 24, 100);
    feed(&mut t, "\x1b[65535I");
    assert_eq!(t.cursor().col, 79);
    feed(&mut t, "\x1b[65535Z");
    assert_eq!(t.cursor().col, 0);
    feed(&mut t, "\x1b[3I");
    assert_eq!(t.cursor().col, 24);
    feed(&mut t, "\x1b[2Z");
    assert_eq!(t.cursor().col, 8);
}

#[test]
fn cht_cbt_flood_is_fast() {
    let mut t = Terminal::new(80, 24, 100);
    let data = "\x1b[65535I\x1b[65535Z".repeat(50_000);
    within(Duration::from_secs(10), || t.advance(data.as_bytes()));
    // With every tab stop cleared the column is stuck from the first step.
    feed(&mut t, "\x1b[3g");
    within(Duration::from_secs(10), || t.advance(data.as_bytes()));
}

// ------------------------------------------------------------------ F7

#[test]
fn whole_screen_operations_are_bounded_per_input_byte() {
    let floods = [
        "\x1b[65535S",
        "\x1b[65535T",
        "\x1b[65535L",
        "\x1b[65535M",
        "\x1b#8",
        "\x1b[2J",
        "\x1b[?1049h\x1b[?1049l",
    ];
    for unit in floods {
        let mut t = Terminal::new(500, 200, 10_000);
        let data = unit.repeat(250_000 / unit.len());
        within(Duration::from_secs(10), || t.advance(data.as_bytes()));
    }
}

#[test]
fn single_whole_screen_operations_still_work() {
    let mut t = Terminal::new(10, 4, 100);
    feed(&mut t, "1\r\n2\r\n3\r\n4\x1b[2S");
    assert_eq!(t.snapshot_lines(), vec!["3", "4", "", ""]);
    feed(&mut t, "\x1b[2T");
    assert_eq!(t.snapshot_lines(), vec!["", "", "3", "4"]);
    feed(&mut t, "\x1b#8");
    assert_eq!(t.snapshot_lines()[3], "EEEEEEEEEE");
    feed(&mut t, "\x1b[2J");
    assert_eq!(t.render_snapshot().trim(), "");
    feed(&mut t, "\x1b[?1049hX\x1b[?1049l");
    assert_eq!(t.screen(), Screen::Main);
}

// ------------------------------------------------------------------ F8

#[test]
fn full_hyperlink_table_does_not_gc_for_every_new_link() {
    let mut t = Terminal::new(200, 40, 10_000);
    let mut fill = String::new();
    for i in 0..MAX_HYPERLINKS {
        fill.push_str(&format!("\x1b]8;;http://x/{i}\x07x"));
    }
    t.advance(fill.as_bytes());
    assert_eq!(t.hyperlinks.len(), MAX_HYPERLINKS);
    let mut more = String::new();
    for i in 0..50_000 {
        more.push_str(&format!("\x1b]8;;http://probe/{i}\x07y"));
    }
    within(Duration::from_secs(20), || t.advance(more.as_bytes()));
    assert!(t.gc_runs < 100, "{} GC runs for 50000 links", t.gc_runs);
}

// ------------------------------------------------------------------ F24

#[test]
fn oversized_hyperlink_uris_are_ignored() {
    let mut t = Terminal::new(40, 3, 100);
    let big = "a".repeat(MAX_HYPERLINK_URI + 1);
    feed(&mut t, &format!("\x1b]8;;http://{big}\x1b\\x"));
    assert_eq!(t.grid().cell(0, 0).hyperlink_id, None);
    assert_eq!(t.hyperlinks.len(), 0);
    let ok = "a".repeat(MAX_HYPERLINK_URI - 10);
    feed(&mut t, &format!("\x1b]8;;http://{ok}\x1b\\y"));
    assert!(t.grid().cell(0, 1).hyperlink_id.is_some());
}

#[test]
fn unused_hyperlinks_do_not_accumulate() {
    let mut t = Terminal::new(40, 3, 100);
    let uri = "a".repeat(MAX_HYPERLINK_URI - 20);
    for i in 0..2000 {
        // Distinct (even with id=) and identical URIs, nothing printed.
        feed(&mut t, &format!("\x1b]8;id={i};http://{uri}{i}\x1b\\"));
        feed(&mut t, &format!("\x1b]8;;http://{uri}\x1b\\"));
    }
    assert_eq!(t.hyperlinks.len(), 1);
    assert!(t.link_bytes <= MAX_HYPERLINK_URI);
}

#[test]
fn hyperlink_table_bytes_are_capped() {
    let mut t = Terminal::new(200, 10, 100_000);
    let uri = "a".repeat(MAX_HYPERLINK_URI - 20);
    // Each link is printed under, so all stay referenced.
    for i in 0..(MAX_HYPERLINK_BYTES / (MAX_HYPERLINK_URI - 10)) * 2 {
        feed(&mut t, &format!("\x1b]8;;http://{uri}{i}\x1b\\x"));
    }
    assert!(t.link_bytes <= MAX_HYPERLINK_BYTES, "{}", t.link_bytes);
    assert!(t.hyperlinks.len() < MAX_HYPERLINKS);
}

#[test]
fn ris_drops_the_hyperlink_table() {
    let mut t = Terminal::new(40, 3, 100);
    feed(&mut t, "\x1b]8;;http://a\x07x\x1b]8;;\x07\x1bc");
    assert_eq!(t.hyperlinks.len(), 0);
    assert_eq!(t.link_bytes, 0);
}

// ------------------------------------------------------------------ F18

#[test]
fn osc52_reply_never_echoes_attacker_text() {
    for sel in ["rm -rf ~/ #", "\x7f", "\u{202e}x", "cccccccccc", "z", "c\x7f"] {
        let mut t = Terminal::new(40, 3, 100);
        feed(&mut t, &format!("\x1b]52;{sel};?\x07"));
        assert_eq!(t.drain_events(), vec![TermEvent::ClipboardQuery]);
        t.respond_clipboard("");
        assert_eq!(t.drain_responses(), b"\x1b]52;c;\x1b\\", "{sel:?}");
    }
    let mut t = Terminal::new(40, 3, 100);
    feed(&mut t, "\x1b]52;qs7;?\x1b\\");
    t.respond_clipboard("");
    assert_eq!(t.drain_responses(), b"\x1b]52;qs7;\x1b\\");
}

// ------------------------------------------------------------------ F23

#[test]
fn control_byte_between_esc_and_osc_introducer_does_not_bypass_the_cap() {
    for pre in [&b"\x1b\x01]"[..], b"\x1b\x00]", b"\x1b\x1f]", b"\x1b\x7f]", b"\x1b\x9f]", b"\x1b\x1b\x01]"] {
        let mut t = Terminal::new(40, 3, 100);
        let mut data = pre.to_vec();
        data.extend_from_slice(b"0;");
        data.resize(data.len() + 3 * 1024 * 1024, b'a');
        for chunk in data.chunks(64 * 1024) {
            t.advance(chunk);
            assert!(t.string_bytes_buffered() <= MAX_STRING_BODY);
        }
        t.advance(b"\x07ok");
        assert!(t.title().is_empty(), "{pre:?}");
        assert!(!t.drain_events().iter().any(|e| matches!(e, TermEvent::Title(_))));
        assert_eq!(t.snapshot_lines()[0], "ok");
    }
}

#[test]
fn osc_after_control_byte_still_works_under_the_cap() {
    let mut t = Terminal::new(40, 3, 100);
    feed(&mut t, "\x1b\x01]2;hi\x07");
    assert_eq!(t.title(), "hi");
}

// ------------------------------------------------------------------ F30

#[test]
fn osc133_marks_are_bounded_per_row() {
    let mut t = Terminal::new(40, 3, 100);
    feed(&mut t, &"\x1b]133;C\x07\x1b]133;B\x07\x1b]133;D;1\x07".repeat(10_000));
    feed(&mut t, "\x1b]133;D;7\x07");
    let marks = t.line_marks();
    assert!(marks.len() <= 4, "{}", marks.len());
    let d = marks.iter().find(|(_, m)| m.kind == MarkKind::CommandEnd).unwrap();
    assert_eq!(d.1.exit_code, Some(7), "latest mark wins");
}

// ------------------------------------------------------------------ F34

#[test]
fn utf8_split_across_calls_matches_a_single_call() {
    let inputs: [&[u8]; 6] = [
        "a\u{43f}\n\u{43c}b".as_bytes(),
        "a\u{43f}\r\u{43c}b".as_bytes(),
        "a\u{43f}\t\u{43c}b".as_bytes(),
        "a\u{43f}\x1b[31m\u{43c}red".as_bytes(),
        "x\u{4e2d}\n\u{4e2d}\u{1f600}\ny".as_bytes(),
        "\u{43f}\u{43f}\u{43f}\u{43f}\n\u{43f}\u{43f}\u{43f}\u{43f}\nz".as_bytes(),
    ];
    for input in inputs {
        let mut whole = Terminal::new(20, 6, 100);
        whole.advance(input);
        for split in 1..input.len() {
            let mut a = Terminal::new(20, 6, 100);
            a.advance(&input[..split]);
            a.advance(&input[split..]);
            assert_eq!(a.render_snapshot(), whole.render_snapshot(), "split {split} of {input:?}");
            assert_eq!(a.cursor(), whole.cursor(), "split {split} of {input:?}");
            // And byte-at-a-time.
        }
        let mut b = Terminal::new(20, 6, 100);
        for byte in input {
            b.advance(std::slice::from_ref(byte));
        }
        assert_eq!(b.render_snapshot(), whole.render_snapshot(), "{input:?}");
    }
}

#[test]
fn issue_repro_lf_after_split_two_byte_char_is_kept() {
    let mut t = Terminal::new(10, 3, 10);
    t.advance(b"a\xd0");
    t.advance(b"\xbf\n\xd0\xbcb");
    assert_eq!(t.snapshot_lines()[0], "a\u{43f}");
    assert_eq!(t.snapshot_lines()[1].trim(), "\u{43c}b");
}

#[test]
fn truncated_utf8_before_ascii_becomes_one_replacement_char() {
    let mut t = Terminal::new(10, 3, 10);
    t.advance(b"a\xe2\x82");
    t.advance(b"b");
    assert_eq!(t.snapshot_lines()[0], "a\u{fffd}b");
}

// ------------------------------------------------------------------ F35

#[test]
fn dcs_ended_by_c1_st_does_not_swallow_later_output() {
    let mut t = Terminal::new(20, 5, 100);
    t.advance(b"\x1bPq\x9c");
    let line = "visible line of text\r\n".repeat(150_000);
    t.advance(line.as_bytes());
    t.advance(b"TAIL-MARKER");
    assert!(t.render_snapshot().contains("TAIL-MARKER"));
    // A 0x9c inside a UTF-8 payload ends the (hooked) DCS in vte as well.
    let mut t = Terminal::new(20, 5, 100);
    t.advance(b"\x1bPq\xe2\x9c\x93 ");
    t.advance(line.as_bytes());
    t.advance(b"TAIL-MARKER");
    assert!(t.render_snapshot().contains("TAIL-MARKER"));
}

#[test]
fn c1_st_before_the_dcs_is_hooked_is_ignored_like_vte_does() {
    let mut t = Terminal::new(20, 3, 10);
    t.advance(b"\x1bP\x9chidden\x1b\\visible");
    assert_eq!(t.snapshot_lines()[0], "visible");
}

// ------------------------------------------------------------------ F36

#[test]
fn ris_clears_scrollback() {
    let mut t = Terminal::new(10, 4, 100);
    for i in 0..50 {
        feed(&mut t, &format!("line{i}\r\n"));
    }
    assert!(t.scrollback_len() > 0);
    let before = t.reset_count();
    feed(&mut t, "\x1bc");
    assert_eq!(t.scrollback_len(), 0);
    assert_eq!(t.reset_count(), before + 1);
    assert_eq!(t.render_snapshot().trim(), "");
    // Nothing scrolled-back is reachable any more.
    assert!(!t.scroll_display(10));
}

#[test]
fn ris_clears_the_alternate_screen_too() {
    let mut t = Terminal::new(10, 3, 10);
    feed(&mut t, "\x1b[?1049hALT\x1b[?1049l\x1bc\x1b[?1047h");
    assert_eq!(t.render_snapshot().trim(), "");
}

// ------------------------------------------------------------------ F39

#[test]
fn osc52_write_with_malformed_base64_is_ignored() {
    for bad in ["A", "====", "AAA", "QQ==QQ==", "QQ", "aGk", "a Gk=", "aG k=", "-_-_", "!!!!"] {
        let mut t = Terminal::new(10, 3, 10);
        feed(&mut t, &format!("\x1b]52;c;{bad}\x07"));
        assert!(t.drain_events().is_empty(), "{bad:?}");
    }
    let mut t = Terminal::new(10, 3, 10);
    feed(&mut t, "\x1b]52;c;aGk=\x07");
    assert_eq!(t.drain_events(), vec![TermEvent::ClipboardSet("hi".into())]);
    // Only an explicitly empty payload clears.
    feed(&mut t, "\x1b]52;c;\x07");
    assert_eq!(t.drain_events(), vec![TermEvent::ClipboardSet(String::new())]);
}

// ------------------------------------------------------------------ F42

#[test]
fn can_and_sub_cancel_an_osc() {
    for cancel in ["\x18", "\x1a"] {
        let mut t = Terminal::new(20, 3, 10);
        feed(&mut t, &format!("abc\x1b]2;pwned{cancel}def"));
        assert!(t.title().is_empty());
        assert_eq!(t.snapshot_lines()[0], "abcdef");
        assert!(t.drain_events().is_empty());

        feed(&mut t, &format!("\x1b]52;c;aGk={cancel}"));
        assert!(t.drain_events().is_empty());

        feed(&mut t, &format!("\x1b]4;1;rgb:ff/00/00{cancel}"));
        assert_eq!(t.colors().palette[1], default_palette_rgb(1));

        // The parser is back to ground and later OSCs work.
        feed(&mut t, "\x1b]2;ok\x07");
        assert_eq!(t.title(), "ok");
    }
}

// ------------------------------------------------------------------ F33

#[test]
fn cells_stay_compact() {
    // Scrollback is millions of cells; this was 56 bytes.
    assert!(std::mem::size_of::<Cell>() <= 40, "{}", std::mem::size_of::<Cell>());
}

#[test]
fn column_resize_moves_a_wide_char_that_no_longer_fits() {
    let mut t = Terminal::new(6, 2, 10);
    feed(&mut t, "ab\u{4e2d}\u{4e2d}");
    // Cells: a b W S W S; shrinking to 5 cuts the second wide char.
    t.resize(5, 2);
    assert_eq!(t.grid().cell(0, 4).width, CellWidth::Single);
    assert_eq!(t.snapshot_lines()[..2], ["ab\u{4e2d}", "\u{4e2d}"]);
    t.resize(8, 2);
    assert_eq!(t.snapshot_lines()[0], "ab\u{4e2d}\u{4e2d}");
    assert_eq!(t.grid().row(0).cells.len(), 8);
}
