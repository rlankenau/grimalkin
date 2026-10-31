//! Reflow on column-width change (primary screen only).

use crate::*;
use std::time::Instant;

fn feed(t: &mut Terminal, s: &str) {
    t.advance(s.as_bytes());
}

/// Every logical line (scrollback then visible), soft-wrapped rows joined,
/// trailing blank lines dropped.
fn logical(t: &Terminal) -> Vec<String> {
    let g = t.main_grid();
    let mut rows: Vec<&Row> = (0..g.scrollback_len()).filter_map(|i| g.scrollback_row(i)).collect();
    rows.extend(g.lines().iter());
    let mut out = Vec::new();
    let mut cur = String::new();
    for r in rows {
        for c in &r.cells {
            if c.is_spacer() || c.flags.contains(Flags::WRAP_PAD) {
                continue;
            }
            c.push_text(&mut cur);
        }
        if !r.wrapped {
            out.push(cur.trim_end_matches(' ').to_string());
            cur.clear();
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out
}

fn snap(t: &Terminal) -> Vec<String> {
    t.snapshot_lines()
}

const LONG: &str = "the quick brown fox jumps over the lazy dog 0123456789";

#[test]
fn narrow_then_widen_restores_text() {
    let mut t = Terminal::new(60, 6, 1000);
    for i in 0..12 {
        feed(&mut t, &format!("{i}: {LONG}\r\n"));
    }
    feed(&mut t, "prompt$ ");
    let before = logical(&t);
    t.resize(13, 6);
    assert_eq!(logical(&t), before);
    assert!(t.main_grid().scrollback_len() > 12);
    t.resize(7, 9);
    assert_eq!(logical(&t), before);
    t.resize(60, 6);
    assert_eq!(logical(&t), before);
    t.resize(120, 6);
    assert_eq!(logical(&t), before);
}

#[test]
fn widening_rejoins_text_cut_while_narrow() {
    let mut t = Terminal::new(10, 4, 100);
    feed(&mut t, "0123456789ABCDEF");
    assert_eq!(snap(&t)[..2], ["0123456789", "ABCDEF"]);
    t.resize(5, 4);
    assert_eq!(snap(&t)[..4], ["01234", "56789", "ABCDE", "F"]);
    t.resize(16, 4);
    assert_eq!(snap(&t)[0], "0123456789ABCDEF");
    assert_eq!(snap(&t)[1], "");
}

#[test]
fn wide_chars_are_never_split_and_leave_a_pad() {
    let mut t = Terminal::new(8, 6, 100);
    feed(&mut t, "ab\u{65e5}\u{672c}\u{8a9e}");
    t.resize(3, 6);
    // a b | pad ; 日 | pad ; 本 | pad ; 語
    assert_eq!(snap(&t)[..4], ["ab", "\u{65e5}", "\u{672c}", "\u{8a9e}"]);
    for r in 0..3 {
        let row = t.grid().row(r);
        assert!(row.wrapped, "row {r}");
        let last = &row.cells[2];
        assert!(last.flags.contains(Flags::WRAP_PAD), "row {r} pad");
        assert_eq!(last.width, CellWidth::Single);
    }
    assert!(!t.grid().row(3).wrapped);
    assert!(t.grid().cell(1, 0).is_wide() && t.grid().cell(1, 1).is_spacer());
    t.resize(8, 6);
    assert_eq!(snap(&t)[0], "ab\u{65e5}\u{672c}\u{8a9e}");
    assert!(!t.grid().row(0).wrapped);
    // Exactly-fitting wide char: no pad.
    t.resize(2, 6);
    assert!(t.grid().cell(1, 0).is_wide());
    assert_eq!(logical(&t), ["ab\u{65e5}\u{672c}\u{8a9e}"]);
    // One column: wide chars cannot exist, nothing panics, narrow ones survive.
    t.resize(1, 6);
    assert_eq!(logical(&t)[0].chars().take(2).collect::<String>(), "ab");
    t.resize(8, 6);
    assert_eq!(snap(&t)[0].chars().take(2).collect::<String>(), "ab");
}

#[test]
fn cursor_stays_on_its_character() {
    let mut t = Terminal::new(40, 5, 100);
    feed(&mut t, "hello world foo bar baz\r\nsecond line here");
    // Put the cursor on the 'w' of "world".
    feed(&mut t, "\x1b[1;7H");
    assert_eq!(t.grid().cell(0, 6).ch, 'w');
    for (cols, rows) in [(5, 5), (4, 7), (9, 5), (3, 5), (40, 5), (11, 3), (80, 5)] {
        t.resize(cols, rows);
        let c = t.cursor();
        assert_eq!(t.grid().cell(c.row, c.col).ch, 'w', "cols {cols}");
        assert_eq!(t.grid().cell(c.row, c.col + 1).ch, 'o');
    }
    assert_eq!(t.cursor(), Cursor { col: 6, row: 0 });
}

#[test]
fn cursor_after_text_and_pending_wrap_follow() {
    let mut t = Terminal::new(10, 3, 100);
    feed(&mut t, "0123456789"); // pending wrap at col 9
    t.resize(4, 3);
    feed(&mut t, "X");
    t.resize(20, 3);
    assert_eq!(snap(&t)[0], "0123456789X");
    assert_eq!(t.cursor(), Cursor { col: 11, row: 0 });
    // Cursor out in the blank area past the text keeps its column.
    feed(&mut t, "\x1b[1;15H");
    t.resize(8, 3);
    feed(&mut t, "Y");
    t.resize(30, 3);
    assert_eq!(snap(&t)[0], "0123456789X   Y");
}

#[test]
fn cursor_absolute_row_never_decreases() {
    let mut t = Terminal::new(20, 5, 1000);
    for i in 0..30 {
        feed(&mut t, &format!("line {i} {LONG}\r\n"));
    }
    let mut last = t.cursor_abs_row();
    for (c, r) in [(7, 5), (50, 5), (9, 8), (120, 4), (5, 5), (20, 5)] {
        t.resize(c, r);
        let now = t.cursor_abs_row();
        assert!(now >= last, "{now} < {last} at {c}x{r}");
        assert!(t.oldest_abs_row() <= now);
        last = now;
    }
}

#[test]
fn scrollback_limit_is_respected() {
    let mut t = Terminal::new(10, 3, 5);
    for i in 0..40 {
        feed(&mut t, &format!("{i:0>10}\r\n"));
    }
    assert_eq!(t.main_grid().scrollback_len(), 5);
    t.resize(3, 3);
    assert_eq!(t.main_grid().scrollback_len(), 5);
    // The newest lines survive (oldest dropped).
    let l = logical(&t);
    assert_eq!(l.last().map(String::as_str), Some("0000000039"));
    t.resize(30, 3);
    assert!(t.main_grid().scrollback_len() <= 5);
    assert!(logical(&t).contains(&"0000000039".to_string()));
    // Zero scrollback.
    let mut t = Terminal::new(10, 3, 0);
    feed(&mut t, "abcdefghij\r\nklmnopqrst\r\nuvwxyz");
    t.resize(4, 3);
    assert_eq!(t.main_grid().scrollback_len(), 0);
    let c = t.cursor();
    assert_eq!(t.grid().cell(c.row, c.col.saturating_sub(1)).ch, 'z');
}

#[test]
fn alt_screen_is_not_reflowed() {
    let mut t = Terminal::new(10, 4, 100);
    feed(&mut t, "0123456789ABC\r\n");
    feed(&mut t, "\x1b[?1049h");
    feed(&mut t, "\x1b[Habcdefghij\x1b[2;1Hxyz");
    t.drain_events();
    t.resize(5, 4);
    assert_eq!(snap(&t)[..2], ["abcde", "xyz"], "alt truncates");
    assert!(t.drain_events().contains(&TermEvent::Reflowed), "main was reflowed");
    feed(&mut t, "\x1b[?1049l");
    assert_eq!(snap(&t)[..3], ["01234", "56789", "ABC"]);
    // Cursor restored by 1049l lands back after the "ABC" line.
    assert_eq!(t.cursor().row, 3);
    t.resize(13, 4);
    assert_eq!(snap(&t)[0], "0123456789ABC");
}

#[test]
fn empty_lines_are_preserved() {
    let mut t = Terminal::new(12, 8, 100);
    feed(&mut t, "a\r\n\r\n\r\nb\r\n\r\nc");
    let before = logical(&t);
    assert_eq!(before, ["a", "", "", "b", "", "c"]);
    t.resize(3, 8);
    assert_eq!(logical(&t), before);
    t.resize(30, 8);
    assert_eq!(logical(&t), before);
    assert_eq!(snap(&t)[..6], ["a", "", "", "b", "", "c"]);
}

#[test]
fn lines_exactly_at_width_have_no_stray_wrap() {
    let mut t = Terminal::new(8, 5, 100);
    feed(&mut t, "abcde\r\nfghij\r\nk");
    t.resize(5, 5);
    assert_eq!(snap(&t)[..3], ["abcde", "fghij", "k"]);
    assert!(t.grid().lines().iter().all(|r| !r.wrapped));
    t.resize(12, 5);
    assert_eq!(snap(&t)[..3], ["abcde", "fghij", "k"]);
    // Trailing blanks never cause a wrap, spaces inside the line survive.
    let mut t = Terminal::new(10, 4, 100);
    feed(&mut t, "ab   cd\x1b[3;1Hxy");
    t.resize(4, 4);
    assert_eq!(logical(&t), ["ab   cd", "", "xy"]);
    t.resize(10, 4);
    assert_eq!(snap(&t)[0], "ab   cd");
    assert!(t.grid().lines().iter().all(|r| !r.wrapped));
}

#[test]
fn reflowed_event_only_for_column_changes() {
    let mut t = Terminal::new(10, 4, 100);
    t.drain_events();
    t.resize(10, 6);
    assert!(t.drain_events().is_empty());
    t.resize(12, 6);
    assert_eq!(t.drain_events(), vec![TermEvent::Reflowed]);
}

#[test]
fn styles_marks_and_links_survive() {
    let mut t = Terminal::new(10, 4, 100);
    feed(&mut t, "\x1b]133;A\x07\x1b[31;1mRED RED RED RED\x1b[0m plain");
    t.resize(4, 4);
    assert!(t.cursor().row + 1 >= 3);
    let marks = t.line_marks();
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].1.kind, MarkKind::PromptStart);
    let c = t.grid().cell(0, 0);
    assert_eq!((c.ch, c.fg, c.bold()), ('R', Color::Indexed(1), true));
    t.resize(40, 4);
    assert_eq!(snap(&t)[0], "RED RED RED RED plain");
    assert_eq!(t.grid().cell(0, 20).fg, Color::Default);
}

#[test]
fn row_growth_with_column_change_pulls_history() {
    let mut t = Terminal::new(10, 3, 100);
    for i in 0..8 {
        feed(&mut t, &format!("L{i}\r\n"));
    }
    feed(&mut t, "end");
    t.resize(20, 6);
    assert_eq!(snap(&t)[3..], ["L6", "L7", "end"][..]);
    assert_eq!(t.cursor().row, 5);
}

/// Wrapped, wide, styled content; random resizes; the logical text never changes.
#[test]
fn random_resizes_preserve_logical_text() {
    let mut t = Terminal::new(30, 8, 100_000);
    let words = ["alpha", "b\u{65e5}\u{672c}c", "e\u{301}x", "\u{1f600}", "0123456789", "z", "wide\u{8a9e}"];
    let mut x: u64 = 0x2545f4914f6cdd1d;
    let mut rnd = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for i in 0..300 {
        let mut line = String::new();
        for _ in 0..(rnd() % 14) {
            line.push_str(words[(rnd() % words.len() as u64) as usize]);
            if rnd() % 3 == 0 {
                line.push(' ');
            }
        }
        feed(&mut t, &format!("{line}{}", if i % 17 == 0 { "\r\n\r\n" } else { "\r\n" }));
    }
    let before = logical(&t);
    for _ in 0..60 {
        t.resize(2 + (rnd() % 100) as usize, 3 + (rnd() % 20) as usize);
        let c = t.cursor();
        assert!(c.row < t.rows() && c.col < t.cols());
        assert_eq!(logical(&t), before, "after resize to {:?}", t.size());
    }
}

fn timed_reflow(lines: usize, cols: usize) -> (u128, u128) {
    let mut t = Terminal::new(cols, 40, lines + 100);
    let text: String = (0..cols).map(|i| (b'a' + (i % 26) as u8) as char).collect();
    let mut data = String::new();
    for i in 0..lines {
        // Every 4th line is longer than a row so wraps are exercised.
        let n = if i % 4 == 0 { cols + cols / 2 } else { cols - 3 };
        data.push_str(&text.chars().cycle().take(n).collect::<String>());
        data.push_str("\r\n");
    }
    feed(&mut t, &data);
    let start = Instant::now();
    t.resize(cols / 2, 40);
    let narrow = start.elapsed().as_millis();
    let start = Instant::now();
    t.resize(cols, 40);
    let wide = start.elapsed().as_millis();
    (narrow, wide)
}

#[test]
fn reflow_of_ten_thousand_lines_is_fast() {
    let (narrow, wide) = timed_reflow(10_000, 200);
    eprintln!("reflow 10k x 200: narrow {narrow} ms, widen {wide} ms");
    // Release runs take a few tens of ms; the bound is loose for debug / CI.
    let limit = if cfg!(debug_assertions) { 5_000 } else { 400 };
    assert!(narrow < limit && wide < limit, "narrow {narrow} ms, widen {wide} ms");
}

/// `cargo test -p grimalkin-vt --release -- --ignored --nocapture reflow_bench`
#[test]
#[ignore]
fn reflow_bench() {
    for _ in 0..3 {
        let (narrow, wide) = timed_reflow(10_000, 200);
        eprintln!("reflow 10k x 200: narrow {narrow} ms, widen {wide} ms");
    }
}
