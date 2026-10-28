//! Adversarial-review regression tests: invariant fuzzing and targeted bugs.
use crate::*;

fn check_invariants(t: &Terminal) {
    let (cols, rows) = t.size();
    let cur = t.cursor();
    assert!(cur.col < cols && cur.row < rows, "cursor {:?} outside {}x{}", cur, cols, rows);
    let g = t.grid();
    assert_eq!(g.rows(), rows);
    assert_eq!(g.cols(), cols);
    for r in 0..rows {
        assert_eq!(g.row(r).cells.len(), cols);
        check_row(&g.row(r).cells);
    }
    for i in 0..g.scrollback_len() {
        let row = g.scrollback_row(i).unwrap();
        assert_eq!(row.cells.len(), cols, "scrollback row {} width", i);
        check_row(&row.cells);
    }
    assert!(t.display_offset() <= g.scrollback_len() || t.screen() == Screen::Alt);
    // Must not panic or underflow.
    let _ = t.oldest_abs_row();
    let _ = t.line_marks();
    let n = t.visible_rows().count();
    assert_eq!(n, rows);
    for r in 0..rows {
        let _ = t.abs_row_of_visible_row(r);
        assert_eq!(t.visible_row(r).cells.len(), cols);
    }
    assert!(t.cursor_abs_row() >= cur.row as u64);
}

fn check_row(cells: &[Cell]) {
    for (i, c) in cells.iter().enumerate() {
        match c.width {
            CellWidth::Wide => {
                assert!(i + 1 < cells.len(), "wide at end");
                assert_eq!(cells[i + 1].width, CellWidth::WideSpacer, "wide without spacer");
            }
            CellWidth::WideSpacer => {
                assert!(i > 0 && cells[i - 1].width == CellWidth::Wide, "orphan spacer");
            }
            CellWidth::Single => {}
        }
    }
}

#[test]
fn invariant_fuzz_with_wide_chars_resizes_and_scrolling() {
    let pieces: &[&str] = &[
        "a", "bc", "\u{65e5}", "\u{672c}", "e\u{301}", "\u{1f600}", "\u{2764}\u{fe0f}", "\u{1f468}\u{200d}\u{1f469}",
        "\u{1f1fa}\u{1f1f8}", "\r", "\n", "\t", "\x08", "\x1b[H", "\x1b[2J", "\x1b[K", "\x1b[1K", "\x1b[3J",
        "\x1b[3;7r", "\x1b[r", "\x1b[2L", "\x1b[3M", "\x1b[4@", "\x1b[3P", "\x1b[2X", "\x1b[5S", "\x1b[2T",
        "\x1b[?1049h", "\x1b[?1049l", "\x1b[?47h", "\x1b[?47l", "\x1b[?7l", "\x1b[?7h", "\x1b[4h", "\x1b[4l",
        "\x1b[?6h", "\x1b[?6l", "\x1b7", "\x1b8", "\x1bM", "\x1bD", "\x1bE", "\x1bc", "\x1b#8", "\x1b[5b",
        "\x1b[9999C", "\x1b[9999;9999H", "\x1b[1;1H", "\x1b]133;A\x07", "\x1b]8;;http://x\x07", "\x1b]8;;\x07",
        "\x1b[?2026h", "\x1b[?2026l", "\x1b(0", "\x1b(B", "\x0e", "\x0f", "\x1b[1;31m", "\x1b[0m",
        "\x1b[48;2;1;2;3m", "\x1b[>1u", "\x1b[<u", "\x1b[?u", "\x1b[14t", "\x1b_Gi=1;AAAA\x1b\\",
        "\x1b[s", "\x1b[u", "\x1b[Z", "\x1b[3I", "\x1b[g", "\x1b[3g", "\x1bH",
    ];
    let mut x: u64 = 0x9e3779b97f4a7c15;
    let mut rnd = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for seed_round in 0..6 {
        let mut t = Terminal::new(2 + seed_round * 3, 3 + seed_round, 20);
        for step in 0..20_000 {
            let r = rnd();
            match r % 40 {
                0 => {
                    let c = 1 + (rnd() % 14) as usize;
                    let rw = 1 + (rnd() % 9) as usize;
                    t.resize(c, rw);
                }
                1 => {
                    let d = (rnd() % 15) as i32 - 7;
                    t.scroll_display(d);
                }
                2 => {
                    t.take_damage();
                }
                3 => {
                    t.move_cursor_rel((rnd() % 40) as i32 - 20, (rnd() % 12) as i32 - 6);
                }
                _ => {
                    let p = pieces[(rnd() % pieces.len() as u64) as usize];
                    t.advance(p.as_bytes());
                }
            }
            if step % 7 == 0 {
                t.drain_events();
                t.drain_responses();
            }
            check_invariants(&t);
        }
    }
}

fn feed(t: &mut Terminal, s: &str) {
    t.advance(s.as_bytes());
}

#[test]
fn decaln_clears_pending_wrap() {
    let mut t = Terminal::new(5, 2, 10);
    feed(&mut t, "abcde"); // pending wrap at the last column
    feed(&mut t, "\x1b#8");
    feed(&mut t, "x");
    assert_eq!(t.grid().cell(0, 0).ch, 'x');
    assert_eq!(t.cursor().row, 0);
    assert_eq!(t.cursor().col, 1);
}

#[test]
fn move_cursor_rel_extreme_values_do_not_panic() {
    let mut t = Terminal::new(5, 3, 10);
    t.move_cursor_rel(i32::MIN, i32::MIN);
    assert_eq!(t.cursor(), Cursor { col: 0, row: 0 });
    t.move_cursor_rel(2, 1);
    assert_eq!(t.cursor(), Cursor { col: 2, row: 1 });
}

#[test]
fn move_cursor_rel_dirties_old_and_new_rows() {
    let mut t = Terminal::new(5, 4, 10);
    feed(&mut t, "x");
    t.take_damage();
    t.move_cursor_rel(0, 2);
    match t.take_damage() {
        Damage::Rows(r) => assert_eq!(r, vec![0, 2]),
        other => panic!("expected cursor rows damaged, got {:?}", other),
    }
}

#[test]
fn view_stays_anchored_when_scrollback_is_full() {
    let mut t = Terminal::new(4, 2, 3);
    for i in 0..10 {
        feed(&mut t, &format!("{}\r\n", i));
    }
    // scrollback holds the 3 lines before the screen; scroll to see them.
    t.scroll_display(1);
    let before = t.visible_row(0).text();
    let abs_before = t.abs_row_of_visible_row(0);
    feed(&mut t, "x\r\n");
    // The same content must still be at the top of the view, or the view must
    // have been clamped at the oldest line.
    let after = t.visible_row(0).text();
    let abs_after = t.abs_row_of_visible_row(0);
    if t.display_offset() < t.scrollback_len() {
        assert_eq!(after, before);
        assert_eq!(abs_after, abs_before);
    }
}

#[test]
fn take_damage_ends_stuck_synchronized_output() {
    let mut t = Terminal::new(5, 2, 10);
    feed(&mut t, "\x1b[?2026hhi");
    assert_eq!(t.take_damage(), Damage::None);
    t.backdate_sync_for_test(std::time::Duration::from_secs(5));
    assert_eq!(t.take_damage(), Damage::Full);
    assert!(!t.synchronized_output());
}

#[test]
fn hyperlink_table_is_garbage_collected_when_full() {
    let mut t = Terminal::new(40, 2, 10);
    // One link that stays on screen, then enough unreferenced ones to fill the table.
    feed(&mut t, "\x1b]8;;http://keep\x07K\x1b]8;;\x07");
    for i in 0..(crate::term::MAX_HYPERLINKS + 50) {
        feed(&mut t, &format!("\x1b]8;;http://x/{}\x07", i));
    }
    // The last link is still active for new text and resolves correctly.
    feed(&mut t, "Z");
    let z = t.grid().cell(0, 1).hyperlink_id.expect("pen link survives gc");
    assert!(t.hyperlink_uri(z).unwrap().starts_with("http://x/"));
    let k = t.grid().cell(0, 0).hyperlink_id.expect("on-screen link survives gc");
    assert_eq!(t.hyperlink_uri(k), Some("http://keep"));
}

#[test]
fn decstbm_clamps_oversized_bottom_margin() {
    let mut t = Terminal::new(5, 5, 10);
    feed(&mut t, "\x1b[2;99r");
    feed(&mut t, "\x1b[5;1Hx\n");
    // Region is rows 2..=5, so the LF at the bottom scrolled only that region.
    assert_eq!(t.grid().cell(3, 0).ch, 'x');
    feed(&mut t, "\x1b[r\x1b[1;1H\x1bM");
    assert_eq!(t.cursor().row, 0);
}

#[test]
fn move_cursor_rel_huge_column_delta_terminates_quickly() {
    let mut t = Terminal::new(1, 3, 10);
    t.move_cursor_rel(i32::MAX, 0);
    assert_eq!(t.cursor().col, 0);
    let mut t = Terminal::new(7, 3, 10);
    t.move_cursor_rel(10, 0); // 10 = 1 wrap + 3 columns
    assert_eq!(t.cursor(), Cursor { col: 3, row: 1 });
    t.move_cursor_rel(i32::MAX, i32::MAX);
    assert!(t.cursor().col < 7 && t.cursor().row < 3);
}
