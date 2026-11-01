use crate::*;

fn term(cols: usize, rows: usize) -> Terminal {
    Terminal::new(cols, rows, 100)
}

fn feed(t: &mut Terminal, s: &str) {
    t.advance(s.as_bytes());
}

fn snap(t: &Terminal) -> Vec<String> {
    t.snapshot_lines()
}

fn lines(t: &Terminal) -> String {
    t.render_snapshot()
}

fn cell(t: &Terminal, r: usize, c: usize) -> &Cell {
    t.grid().cell(r, c)
}

// ---------------------------------------------------------------- basics

#[test]
fn prints_text_and_snapshot() {
    let mut t = term(10, 3);
    feed(&mut t, "hello");
    assert_eq!(snap(&t), vec!["hello", "", ""]);
    assert_eq!(t.cursor(), Cursor { col: 5, row: 0 });
    assert_eq!(format!("{}", t), "hello\n\n");
}

#[test]
fn cr_lf_bs_tab_bel() {
    let mut t = term(20, 4);
    feed(&mut t, "ab\r\ncd\tx\x08y");
    assert_eq!(snap(&t)[0], "ab");
    assert_eq!(snap(&t)[1], "cd      y");
    feed(&mut t, "\x07");
    assert_eq!(t.drain_events(), vec![TermEvent::Bell]);
}

#[test]
fn autowrap_and_pending_wrap() {
    let mut t = term(5, 3);
    feed(&mut t, "abcde");
    // Cursor sits on the last column with a pending wrap.
    assert_eq!(t.cursor(), Cursor { col: 4, row: 0 });
    feed(&mut t, "f");
    assert_eq!(snap(&t), vec!["abcde", "f", ""]);
    assert!(t.grid().row(0).wrapped);
    assert!(!t.grid().row(1).wrapped);
}

#[test]
fn autowrap_off_overwrites_last_column() {
    let mut t = term(5, 2);
    feed(&mut t, "\x1b[?7labcdefg");
    assert_eq!(snap(&t), vec!["abcdg", ""]);
    feed(&mut t, "\x1b[?7h");
    assert!(t.modes().wrap);
}

#[test]
fn scrolls_when_full() {
    let mut t = term(4, 3);
    feed(&mut t, "1\r\n2\r\n3\r\n4");
    assert_eq!(snap(&t), vec!["2", "3", "4"]);
    assert_eq!(t.scrollback_len(), 1);
    assert_eq!(t.grid().scrollback_row(0).unwrap().text(), "1");
}

// ---------------------------------------------------------------- cursor CSI

#[test]
fn cursor_movement_csi() {
    let mut t = term(10, 6);
    feed(&mut t, "\x1b[3;4H");
    assert_eq!(t.cursor(), Cursor { col: 3, row: 2 });
    feed(&mut t, "\x1b[A");
    assert_eq!(t.cursor().row, 1);
    feed(&mut t, "\x1b[2B");
    assert_eq!(t.cursor().row, 3);
    feed(&mut t, "\x1b[3C");
    assert_eq!(t.cursor().col, 6);
    feed(&mut t, "\x1b[2D");
    assert_eq!(t.cursor().col, 4);
    feed(&mut t, "\x1b[8G");
    assert_eq!(t.cursor().col, 7);
    feed(&mut t, "\x1b[5d");
    assert_eq!(t.cursor().row, 4);
    feed(&mut t, "\x1b[2;2f");
    assert_eq!(t.cursor(), Cursor { col: 1, row: 1 });
    feed(&mut t, "\x1b[2E");
    assert_eq!(t.cursor(), Cursor { col: 0, row: 3 });
    feed(&mut t, "\x1b[1F");
    assert_eq!(t.cursor(), Cursor { col: 0, row: 2 });
    // Clamping.
    feed(&mut t, "\x1b[99;99H");
    assert_eq!(t.cursor(), Cursor { col: 9, row: 5 });
    feed(&mut t, "\x1b[H");
    assert_eq!(t.cursor(), Cursor { col: 0, row: 0 });
    feed(&mut t, "\x1b[99A\x1b[99D");
    assert_eq!(t.cursor(), Cursor { col: 0, row: 0 });
}

#[test]
fn erase_display_variants() {
    let mut t = term(5, 3);
    feed(&mut t, "aaaaa\r\nbbbbb\r\nccccc\x1b[2;3H");
    feed(&mut t, "\x1b[J");
    assert_eq!(snap(&t), vec!["aaaaa", "bb", ""]);

    let mut t = term(5, 3);
    feed(&mut t, "aaaaa\r\nbbbbb\r\nccccc\x1b[2;3H");
    feed(&mut t, "\x1b[1J");
    assert_eq!(snap(&t), vec!["", "   bb", "ccccc"]);

    feed(&mut t, "\x1b[2J");
    assert_eq!(snap(&t), vec!["", "", ""]);
}

#[test]
fn erase_display_3_clears_scrollback() {
    let mut t = term(4, 2);
    feed(&mut t, "1\r\n2\r\n3\r\n4");
    assert!(t.scrollback_len() > 0);
    feed(&mut t, "\x1b[3J");
    assert_eq!(t.scrollback_len(), 0);
}

#[test]
fn erase_line_variants() {
    let mut t = term(6, 3);
    feed(&mut t, "abcdef\x1b[1;3H\x1b[K");
    assert_eq!(snap(&t)[0], "ab");
    feed(&mut t, "\x1b[2;1Habcdef\x1b[2;3H\x1b[1K");
    assert_eq!(snap(&t)[1], "   def");
    feed(&mut t, "\x1b[3;1Habcdef\x1b[2K");
    assert_eq!(snap(&t)[2], "");
}

#[test]
fn erase_chars_ech() {
    let mut t = term(8, 2);
    feed(&mut t, "abcdefgh\x1b[1;3H\x1b[3X");
    assert_eq!(snap(&t)[0], "ab   fgh");
    assert_eq!(t.cursor().col, 2);
}

#[test]
fn insert_and_delete_chars() {
    let mut t = term(8, 2);
    feed(&mut t, "abcdefgh\x1b[1;3H\x1b[2@");
    assert_eq!(snap(&t)[0], "ab  cdef");
    feed(&mut t, "\x1b[3P");
    assert_eq!(snap(&t)[0], "abdef");
}

#[test]
fn insert_and_delete_lines() {
    let mut t = term(4, 5);
    feed(&mut t, "1\r\n2\r\n3\r\n4\r\n5\x1b[2;1H\x1b[2L");
    assert_eq!(snap(&t), vec!["1", "", "", "2", "3"]);
    feed(&mut t, "\x1b[1M");
    assert_eq!(snap(&t), vec!["1", "", "2", "3", ""]);
    // IL/DL do not touch scrollback.
    assert_eq!(t.scrollback_len(), 0);
    assert_eq!(t.cursor().col, 0);
}

#[test]
fn scroll_up_down_su_sd() {
    let mut t = term(4, 4);
    feed(&mut t, "1\r\n2\r\n3\r\n4\x1b[2S");
    assert_eq!(snap(&t), vec!["3", "4", "", ""]);
    assert_eq!(t.scrollback_len(), 2);
    feed(&mut t, "\x1b[1T");
    assert_eq!(snap(&t), vec!["", "3", "4", ""]);
}

#[test]
fn decstbm_scroll_region() {
    let mut t = term(4, 5);
    feed(&mut t, "1\r\n2\r\n3\r\n4\r\n5");
    feed(&mut t, "\x1b[2;4r");
    assert_eq!(t.cursor(), Cursor { col: 0, row: 0 });
    feed(&mut t, "\x1b[4;1H\n");
    assert_eq!(snap(&t), vec!["1", "3", "4", "", "5"]);
    // Scrolling a sub-region does not feed scrollback.
    assert_eq!(t.scrollback_len(), 0);
    feed(&mut t, "\x1b[2;1H\x1bM");
    assert_eq!(snap(&t), vec!["1", "", "3", "4", "5"]);
    // Reset.
    feed(&mut t, "\x1b[r\x1b[5;1H\n");
    assert_eq!(t.scrollback_len(), 1);
}

#[test]
fn origin_mode_is_relative_to_region() {
    let mut t = term(5, 6);
    feed(&mut t, "\x1b[3;5r\x1b[?6h\x1b[1;1Hx");
    assert_eq!(snap(&t)[2], "x");
    feed(&mut t, "\x1b[6n");
    assert_eq!(t.drain_responses(), b"\x1b[1;2R");
}

#[test]
fn decsc_decrc() {
    let mut t = term(10, 5);
    feed(&mut t, "\x1b[3;4H\x1b[1;31m\x1b7\x1b[H\x1b[0m\x1b8x");
    assert_eq!(t.cursor(), Cursor { col: 4, row: 2 });
    let c = cell(&t, 2, 3);
    assert_eq!(c.ch, 'x');
    assert_eq!(c.fg, Color::Indexed(1));
    assert!(c.bold());
    // CSI s / CSI u variants.
    feed(&mut t, "\x1b[1;1H\x1b[s\x1b[5;5H\x1b[u");
    assert_eq!(t.cursor(), Cursor { col: 0, row: 0 });
}

#[test]
fn tab_stops_hts_tbc_cbt() {
    let mut t = term(40, 2);
    feed(&mut t, "\t");
    assert_eq!(t.cursor().col, 8);
    feed(&mut t, "\x1b[4G\x1bH\x1b[1G\t");
    assert_eq!(t.cursor().col, 3);
    feed(&mut t, "\x1b[g\x1b[1G\t"); // clear nothing at col 0 (tab at 0 default stop)
    feed(&mut t, "\x1b[3g\x1b[1G\t");
    assert_eq!(t.cursor().col, 39);
    // Reinstate: set stops at 5 and 10, backtab.
    let mut t = term(40, 2);
    feed(&mut t, "\x1b[3g\x1b[6G\x1bH\x1b[11G\x1bH\x1b[20G\x1b[Z");
    assert_eq!(t.cursor().col, 10);
    feed(&mut t, "\x1b[Z");
    assert_eq!(t.cursor().col, 5);
    feed(&mut t, "\x1b[2I");
    assert_eq!(t.cursor().col, 39);
}

#[test]
fn rep_repeats_last_char() {
    let mut t = term(10, 1);
    feed(&mut t, "a\x1b[3b");
    assert_eq!(snap(&t)[0], "aaaa");
}

// ---------------------------------------------------------------- SGR

#[test]
fn sgr_basic_attributes() {
    let mut t = term(20, 2);
    feed(&mut t, "\x1b[1;2;3;4;5;7;8;9mX\x1b[22;23;24;25;27;28;29mY");
    let x = cell(&t, 0, 0);
    assert!(x.flags.contains(Flags::BOLD));
    assert!(x.flags.contains(Flags::DIM));
    assert!(x.flags.contains(Flags::ITALIC));
    assert!(x.flags.contains(Flags::BLINK));
    assert!(x.flags.contains(Flags::INVERSE));
    assert!(x.flags.contains(Flags::HIDDEN));
    assert!(x.flags.contains(Flags::STRIKETHROUGH));
    assert_eq!(x.underline, Underline::Single);
    let y = cell(&t, 0, 1);
    assert!(y.flags.is_empty());
    assert_eq!(y.underline, Underline::None);
}

#[test]
fn sgr_colors_16_256_rgb() {
    let mut t = term(20, 2);
    feed(&mut t, "\x1b[31;42mA\x1b[91;102mB\x1b[38;5;200;48;5;17mC");
    assert_eq!(cell(&t, 0, 0).fg, Color::Indexed(1));
    assert_eq!(cell(&t, 0, 0).bg, Color::Indexed(2));
    assert_eq!(cell(&t, 0, 1).fg, Color::Indexed(9));
    assert_eq!(cell(&t, 0, 1).bg, Color::Indexed(10));
    assert_eq!(cell(&t, 0, 2).fg, Color::Indexed(200));
    assert_eq!(cell(&t, 0, 2).bg, Color::Indexed(17));
    feed(&mut t, "\x1b[39;49mD");
    assert_eq!(cell(&t, 0, 3).fg, Color::Default);
    assert_eq!(cell(&t, 0, 3).bg, Color::Default);
}

#[test]
fn sgr_truecolor_semicolon_and_colon() {
    let mut t = term(20, 2);
    feed(&mut t, "\x1b[38;2;10;20;30;48;2;1;2;3mA");
    assert_eq!(cell(&t, 0, 0).fg, Color::Rgb(10, 20, 30));
    assert_eq!(cell(&t, 0, 0).bg, Color::Rgb(1, 2, 3));
    feed(&mut t, "\x1b[38:2:40:50:60mB\x1b[48:2::7:8:9mC\x1b[38:5:99mD");
    assert_eq!(cell(&t, 0, 1).fg, Color::Rgb(40, 50, 60));
    assert_eq!(cell(&t, 0, 2).bg, Color::Rgb(7, 8, 9));
    assert_eq!(cell(&t, 0, 3).fg, Color::Indexed(99));
    // Following params still apply after an extended colour.
    feed(&mut t, "\x1b[0;38;2;1;2;3;1mE");
    assert!(cell(&t, 0, 4).bold());
    assert_eq!(cell(&t, 0, 4).fg, Color::Rgb(1, 2, 3));
}

#[test]
fn sgr_underline_styles_and_color() {
    let mut t = term(20, 2);
    let styles = [
        ("4:0", Underline::None),
        ("4:1", Underline::Single),
        ("4:2", Underline::Double),
        ("4:3", Underline::Curly),
        ("4:4", Underline::Dotted),
        ("4:5", Underline::Dashed),
        ("21", Underline::Double),
        ("4", Underline::Single),
    ];
    for (i, (code, want)) in styles.iter().enumerate() {
        feed(&mut t, &format!("\x1b[{}mx", code));
        assert_eq!(cell(&t, 0, i).underline, *want, "{}", code);
    }
    feed(&mut t, "\x1b[58;2;1;2;3mu\x1b[58:5:100mv\x1b[59mw");
    assert_eq!(cell(&t, 1, 0).ch, ' ');
    let row = t.grid().row(0);
    assert_eq!(row.cells[8].underline_color, Color::Rgb(1, 2, 3));
    assert_eq!(row.cells[9].underline_color, Color::Indexed(100));
    assert_eq!(row.cells[10].underline_color, Color::Default);
}

#[test]
fn sgr_reset_and_empty() {
    let mut t = term(10, 1);
    feed(&mut t, "\x1b[1;31mA\x1b[mB\x1b[1m\x1b[0mC");
    assert!(!cell(&t, 0, 1).bold());
    assert_eq!(cell(&t, 0, 1).fg, Color::Default);
    assert!(!cell(&t, 0, 2).bold());
}

#[test]
fn bce_erase_uses_background() {
    let mut t = term(5, 2);
    feed(&mut t, "\x1b[44m\x1b[2J\x1b[K");
    assert_eq!(cell(&t, 1, 4).bg, Color::Indexed(4));
    assert_eq!(cell(&t, 1, 4).ch, ' ');
}

// ---------------------------------------------------------------- alt screen

#[test]
fn alt_screen_1049_preserves_main() {
    let mut t = term(10, 3);
    feed(&mut t, "main\x1b[?1049h");
    assert_eq!(t.screen(), Screen::Alt);
    assert_eq!(snap(&t), vec!["", "", ""]);
    feed(&mut t, "\x1b[2;2Halt");
    assert_eq!(snap(&t)[1], " alt");
    feed(&mut t, "\x1b[?1049l");
    assert_eq!(t.screen(), Screen::Main);
    assert_eq!(snap(&t)[0], "main");
    assert_eq!(t.cursor(), Cursor { col: 4, row: 0 });
    assert_eq!(t.alt_enter_count(), 1);
    feed(&mut t, "\x1b[?1049h");
    assert_eq!(t.alt_enter_count(), 2);
    assert_eq!(snap(&t), vec!["", "", ""]);
}

#[test]
fn alt_screen_47_and_1047() {
    for m in [47, 1047] {
        let mut t = term(10, 3);
        feed(&mut t, &format!("hi\x1b[?{}hxx\x1b[?{}l", m, m));
        assert_eq!(t.screen(), Screen::Main);
        assert_eq!(snap(&t)[0], "hi");
    }
}

#[test]
fn alt_screen_has_no_scrollback_and_abs_rows_reset() {
    let mut t = term(5, 2);
    feed(&mut t, "1\r\n2\r\n3");
    assert_eq!(t.scrollback_len(), 1);
    let main_abs = t.cursor_abs_row();
    assert_eq!(main_abs, 2);
    feed(&mut t, "\x1b[?1049h");
    assert_eq!(t.cursor_abs_row(), 1); // counter reset, cursor on row 1? (saved col/row kept)
    feed(&mut t, "a\r\nb\r\nc");
    assert_eq!(t.scrollback_len(), 0);
    assert!(t.cursor_abs_row() >= 2);
    feed(&mut t, "\x1b[?1049l");
    assert_eq!(t.cursor_abs_row(), main_abs);
}

#[test]
fn alt_screen_kitty_keyboard_stack_is_separate() {
    let mut t = term(10, 3);
    feed(&mut t, "\x1b[>1u");
    assert_eq!(t.keyboard_flags(), 1);
    feed(&mut t, "\x1b[?1049h");
    assert_eq!(t.keyboard_flags(), 0);
    feed(&mut t, "\x1b[>5u");
    assert_eq!(t.keyboard_flags(), 5);
    feed(&mut t, "\x1b[?1049l");
    assert_eq!(t.keyboard_flags(), 1);
}

// ---------------------------------------------------------------- scrollback

#[test]
fn scrollback_limit_and_abs_rows() {
    let mut t = Terminal::new(4, 2, 3);
    for i in 0..10 {
        feed(&mut t, &format!("l{}\r\n", i));
    }
    assert_eq!(t.scrollback_len(), 3);
    assert_eq!(t.grid().scrolled_total(), 9);
    assert_eq!(t.cursor_abs_row(), 9 + t.cursor().row as u64);
    assert_eq!(t.oldest_abs_row(), 6);
    assert_eq!(t.row_at_abs(6).unwrap().text(), "l6");
    assert_eq!(t.row_at_abs(9).unwrap().text(), "l9");
    assert!(t.row_at_abs(5).is_none());
    assert_eq!(t.abs_row_of_visible_row(0), 9);
    assert_eq!(t.abs_row_of_visible_row(1), 10);
}

#[test]
fn scrollback_limit_changes_at_runtime() {
    let mut t = Terminal::new(4, 2, 10);
    for i in 0..8 {
        feed(&mut t, &format!("l{}\r\n", i));
    }
    assert_eq!(t.scrollback_len(), 7);
    t.scroll_display(7);
    assert_eq!(t.display_offset(), 7);
    assert!(t.set_scrollback_limit(3));
    assert_eq!(t.scrollback_len(), 3);
    assert_eq!(t.display_offset(), 3);
    assert_eq!(t.oldest_abs_row(), 4);
    assert_eq!(t.row_at_abs(4).unwrap().text(), "l4");
    // Growing the limit drops nothing.
    assert!(!t.set_scrollback_limit(100));
    assert_eq!(t.scrollback_len(), 3);
}

#[test]
fn zero_scrollback_keeps_counting() {
    let mut t = Terminal::new(4, 2, 0);
    feed(&mut t, "a\r\nb\r\nc\r\nd");
    assert_eq!(t.scrollback_len(), 0);
    assert_eq!(t.grid().scrolled_total(), 2);
}

#[test]
fn display_offset_scrolling_view() {
    let mut t = Terminal::new(4, 3, 50);
    for i in 0..8 {
        feed(&mut t, &format!("{}\r\n", i));
    }
    // Rows now: scrollback 0..=5, visible "6","7","".
    assert_eq!(snap(&t), vec!["6", "7", ""]);
    assert!(t.scroll_display(2));
    assert_eq!(t.display_offset(), 2);
    assert_eq!(snap(&t), vec!["4", "5", "6"]);
    assert_eq!(t.abs_row_of_visible_row(0), t.grid().scrolled_total() - 2);
    // New output keeps the view anchored.
    feed(&mut t, "x\r\n");
    assert_eq!(t.display_offset(), 3);
    assert_eq!(snap(&t)[0], "4");
    assert!(t.scroll_display(-100));
    assert_eq!(t.display_offset(), 0);
    assert!(!t.scroll_display(-1));
    assert!(t.scroll_display(1000));
    assert_eq!(t.display_offset(), t.scrollback_len());
    assert!(t.scroll_display_to_bottom());
    assert_eq!(t.display_offset(), 0);
}

#[test]
fn scroll_display_marks_full_damage() {
    let mut t = Terminal::new(4, 3, 50);
    feed(&mut t, "a\r\nb\r\nc\r\nd\r\n");
    t.take_damage();
    t.scroll_display(1);
    assert_eq!(t.take_damage(), Damage::Full);
}

// ---------------------------------------------------------------- wide chars

#[test]
fn wide_char_occupies_two_cells() {
    let mut t = term(6, 2);
    feed(&mut t, "a日b");
    assert_eq!(cell(&t, 0, 1).ch, '日');
    assert!(cell(&t, 0, 1).is_wide());
    assert!(cell(&t, 0, 2).is_spacer());
    assert_eq!(cell(&t, 0, 3).ch, 'b');
    assert_eq!(snap(&t)[0], "a日b");
    assert_eq!(t.cursor().col, 4);
}

#[test]
fn wide_char_wraps_at_right_edge() {
    let mut t = term(5, 3);
    feed(&mut t, "abcd日");
    // No room for a wide char in the last column: it wraps to the next line.
    assert_eq!(snap(&t), vec!["abcd", "日", ""]);
    assert!(t.grid().row(0).wrapped);
    assert!(cell(&t, 0, 4).is_empty());
    assert_eq!(t.cursor().col, 2);
}

#[test]
fn wide_char_fits_exactly_at_edge() {
    let mut t = term(5, 3);
    feed(&mut t, "abc日d");
    assert_eq!(snap(&t), vec!["abc日", "d", ""]);
}

#[test]
fn wide_char_without_autowrap_is_dropped_at_edge() {
    let mut t = term(5, 2);
    feed(&mut t, "\x1b[?7labcd日");
    assert_eq!(snap(&t), vec!["abcd", ""]);
}

#[test]
fn overwriting_half_of_wide_char_clears_other_half() {
    let mut t = term(6, 2);
    feed(&mut t, "日本\x1b[1;2Hx");
    assert_eq!(cell(&t, 0, 0).ch, ' ');
    assert_eq!(cell(&t, 0, 1).ch, 'x');
    assert!(!cell(&t, 0, 1).is_spacer());
    feed(&mut t, "\x1b[1;3H\x1b[1;4Hy");
    // overwrote spacer of 本 => its base cleared
    assert_eq!(cell(&t, 0, 2).ch, ' ');
    assert_eq!(cell(&t, 0, 3).ch, 'y');
}

#[test]
fn erase_and_delete_do_not_leave_orphans() {
    let mut t = term(6, 2);
    feed(&mut t, "日本語\x1b[1;2H\x1b[P");
    for c in &t.grid().row(0).cells {
        assert!(!c.is_spacer() || c.ch == ' ');
    }
    // Orphaned spacer at col 0 would be invalid.
    assert!(!cell(&t, 0, 0).is_spacer());
    let mut t = term(6, 2);
    feed(&mut t, "日本語\x1b[1;2H\x1b[X");
    assert_eq!(cell(&t, 0, 0).ch, ' ');
    assert!(!cell(&t, 0, 0).is_wide());
}

#[test]
fn combining_marks_attach_to_previous_cell() {
    let mut t = term(6, 1);
    feed(&mut t, "e\u{301}x");
    assert_eq!(cell(&t, 0, 0).ch, 'e');
    assert_eq!(cell(&t, 0, 0).combining, vec!['\u{301}']);
    assert_eq!(cell(&t, 0, 1).ch, 'x');
    assert_eq!(snap(&t)[0], "e\u{301}x");
}

#[test]
fn emoji_vs16_widens_and_zwj_sequences_join() {
    let mut t = term(8, 1);
    feed(&mut t, "\u{2764}\u{fe0f}x");
    assert!(cell(&t, 0, 0).is_wide());
    assert_eq!(cell(&t, 0, 2).ch, 'x');
    let mut t = term(8, 1);
    feed(&mut t, "👨\u{200d}👩x");
    assert_eq!(cell(&t, 0, 0).ch, '👨');
    assert_eq!(cell(&t, 0, 0).combining, vec!['\u{200d}', '👩']);
    assert_eq!(cell(&t, 0, 2).ch, 'x');
}

#[test]
fn flag_regional_indicators_form_one_cluster() {
    let mut t = term(8, 1);
    feed(&mut t, "🇺🇸x");
    assert_eq!(cell(&t, 0, 0).ch, '🇺');
    assert_eq!(cell(&t, 0, 0).combining, vec!['🇸']);
    assert!(cell(&t, 0, 0).is_wide());
    assert_eq!(cell(&t, 0, 2).ch, 'x');
}

// ---------------------------------------------------------------- modes

#[test]
fn decset_basic_modes() {
    let mut t = term(10, 3);
    assert!(t.modes().cursor_visible);
    feed(&mut t, "\x1b[?25l");
    assert!(!t.cursor_visible());
    feed(&mut t, "\x1b[?25h");
    assert!(t.cursor_visible());
    feed(&mut t, "\x1b[?1h");
    assert!(t.app_cursor_keys());
    feed(&mut t, "\x1b[?1l");
    assert!(!t.app_cursor_keys());
    feed(&mut t, "\x1b[?12l");
    assert!(!t.cursor_blink());
    feed(&mut t, "\x1b[?2004h");
    assert!(t.bracketed_paste());
    feed(&mut t, "\x1b[?2004l");
    assert!(!t.bracketed_paste());
    feed(&mut t, "\x1b[?1004h");
    assert!(t.focus_reporting());
    assert!(!t.win32_input_mode());
    feed(&mut t, "\x1b[?9001h");
    assert!(t.win32_input_mode());
    feed(&mut t, "\x1b[?9001l");
    assert!(!t.win32_input_mode());
    feed(&mut t, "\x1b[4h");
    assert!(t.modes().insert);
    feed(&mut t, "\x1b[4l");
    assert!(!t.modes().insert);
}

#[test]
fn insert_mode_shifts_text() {
    let mut t = term(8, 1);
    feed(&mut t, "abcd\x1b[1;2H\x1b[4hXY");
    assert_eq!(snap(&t)[0], "aXYbcd");
}

#[test]
fn mouse_modes_and_events() {
    let mut t = term(10, 3);
    feed(&mut t, "\x1b[?1000h");
    assert_eq!(t.mouse_mode(), MouseMode::Press);
    assert_eq!(t.drain_events(), vec![TermEvent::SetMouseMode]);
    feed(&mut t, "\x1b[?1002h\x1b[?1006h");
    assert_eq!(t.mouse_mode(), MouseMode::Drag);
    assert_eq!(t.mouse_encoding(), MouseEncoding::Sgr);
    // Two changes in one batch coalesce to one notification.
    assert_eq!(t.drain_events(), vec![TermEvent::SetMouseMode]);
    feed(&mut t, "\x1b[?1003h");
    assert_eq!(t.mouse_mode(), MouseMode::Any);
    feed(&mut t, "\x1b[?1003l\x1b[?1006l");
    assert_eq!(t.mouse_mode(), MouseMode::None);
    assert_eq!(t.mouse_encoding(), MouseEncoding::Default);
    // Setting an unchanged mode emits nothing.
    t.drain_events();
    feed(&mut t, "\x1b[?1000l");
    assert!(t.drain_events().is_empty());
}

#[test]
fn decrqm_reports_mode_state() {
    let mut t = term(10, 3);
    feed(&mut t, "\x1b[?2004$p");
    assert_eq!(t.drain_responses(), b"\x1b[?2004;2$y");
    feed(&mut t, "\x1b[?2004h\x1b[?2004$p");
    assert_eq!(t.drain_responses(), b"\x1b[?2004;1$y");
    feed(&mut t, "\x1b[?2026$p");
    assert_eq!(t.drain_responses(), b"\x1b[?2026;2$y");
    feed(&mut t, "\x1b[?9999$p");
    assert_eq!(t.drain_responses(), b"\x1b[?9999;0$y");
}

#[test]
fn decscusr_zero_and_ris_return_cursor_style_to_the_host_default() {
    let mut t = term(10, 3);
    assert_eq!((t.cursor_shape_override(), t.cursor_blink_override()), (None, None));
    feed(&mut t, "\x1b[?12l");
    assert_eq!(t.cursor_blink_override(), Some(false));
    feed(&mut t, "\x1b[5 q");
    assert_eq!((t.cursor_shape_override(), t.cursor_blink_override()), (Some(CursorShape::Bar), Some(true)));
    feed(&mut t, "\x1b[0 q");
    assert_eq!((t.cursor_shape_override(), t.cursor_blink_override()), (None, None));
    feed(&mut t, "\x1b[6 q");
    assert_eq!(t.cursor_blink_override(), Some(false));
    feed(&mut t, "\x1bc");
    assert_eq!((t.cursor_shape_override(), t.cursor_blink_override()), (None, None));
}

#[test]
fn decscusr_cursor_shape() {
    let mut t = term(10, 3);
    feed(&mut t, "\x1b[5 q");
    assert_eq!(t.cursor_shape(), CursorShape::Bar);
    assert!(t.cursor_blink());
    assert_eq!(t.drain_events(), vec![TermEvent::CursorShape(CursorShape::Bar)]);
    feed(&mut t, "\x1b[4 q");
    assert_eq!(t.cursor_shape(), CursorShape::Underline);
    assert!(!t.cursor_blink());
    feed(&mut t, "\x1b[2 q");
    assert_eq!(t.cursor_shape(), CursorShape::Block);
    assert!(!t.cursor_blink());
}

// ---------------------------------------------------------------- synchronized output

#[test]
fn synchronized_output_suppresses_damage() {
    let mut t = term(10, 3);
    feed(&mut t, "hi");
    assert_ne!(t.take_damage(), Damage::None);
    assert_eq!(t.take_damage(), Damage::None);
    feed(&mut t, "\x1b[?2026hbuffered");
    assert!(t.synchronized_output());
    assert_eq!(t.take_damage(), Damage::None);
    assert!(t.is_dirty());
    feed(&mut t, "\x1b[?2026l");
    assert!(!t.synchronized_output());
    assert_eq!(t.take_damage(), Damage::Full);
    assert_eq!(snap(&t)[0], "hibuffered");
}

#[test]
fn synchronized_output_times_out() {
    let mut t = term(10, 3);
    feed(&mut t, "\x1b[?2026h");
    std::thread::sleep(std::time::Duration::from_millis(20));
    assert!(t.check_sync_timeout(std::time::Duration::from_millis(5)));
    assert!(!t.synchronized_output());
}

// ---------------------------------------------------------------- damage

#[test]
fn damage_tracks_changed_rows() {
    let mut t = term(10, 5);
    assert_eq!(t.take_damage(), Damage::Full); // initial
    assert_eq!(t.take_damage(), Damage::None);
    feed(&mut t, "\x1b[3;1Hx");
    match t.take_damage() {
        Damage::Rows(r) => {
            assert!(r.contains(&2));
            assert!(!r.contains(&4));
        }
        d => panic!("unexpected {:?}", d),
    }
    // Cursor moves dirty old and new rows.
    feed(&mut t, "\x1b[1;1H");
    match t.take_damage() {
        Damage::Rows(r) => assert_eq!(r, vec![0, 2]),
        d => panic!("unexpected {:?}", d),
    }
    feed(&mut t, "\x1b[2;1H");
    assert_eq!(t.dirty_rows(), vec![0, 1]);
    t.clear_damage();
    assert!(!t.is_dirty());
    t.mark_all();
    assert_eq!(t.dirty_rows().len(), 5);
}

#[test]
fn resize_marks_full_damage() {
    let mut t = term(10, 5);
    t.take_damage();
    t.resize(12, 6);
    assert_eq!(t.take_damage(), Damage::Full);
}

// ---------------------------------------------------------------- responses

#[test]
fn da1_da2_dsr_cpr() {
    let mut t = term(80, 24);
    feed(&mut t, "\x1b[c");
    assert_eq!(t.drain_responses(), b"\x1b[?62;22c");
    feed(&mut t, "\x1b[0c");
    assert_eq!(t.drain_responses(), b"\x1b[?62;22c");
    feed(&mut t, "\x1b[>c");
    assert_eq!(t.drain_responses(), b"\x1b[>1;4000;0c");
    feed(&mut t, "\x1b[5n");
    assert_eq!(t.drain_responses(), b"\x1b[0n");
    feed(&mut t, "\x1b[4;9H\x1b[6n");
    assert_eq!(t.drain_responses(), b"\x1b[4;9R");
    feed(&mut t, "\x1b[?6n");
    assert_eq!(t.drain_responses(), b"\x1b[?4;9R");
    assert!(t.drain_responses().is_empty());
}

#[test]
fn xtversion() {
    let mut t = term(80, 24);
    feed(&mut t, "\x1b[>q");
    let r = String::from_utf8(t.drain_responses()).unwrap();
    assert_eq!(
        r,
        format!("\x1bP>|grimalkin {}\x1b\\", env!("CARGO_PKG_VERSION"))
    );
    feed(&mut t, "\x1b[>0q");
    assert!(!t.drain_responses().is_empty());
}

#[test]
fn window_ops_pixel_reports() {
    let mut t = term(80, 24);
    t.set_cell_pixel_size(10u32, 20u32);
    feed(&mut t, "\x1b[14t");
    assert_eq!(t.drain_responses(), b"\x1b[4;480;800t");
    feed(&mut t, "\x1b[16t");
    assert_eq!(t.drain_responses(), b"\x1b[6;20;10t");
    feed(&mut t, "\x1b[18t");
    assert_eq!(t.drain_responses(), b"\x1b[8;24;80t");
    assert_eq!(t.cell_pixel_size(), (10, 20));
}

#[test]
fn push_response_and_drain() {
    let mut t = term(10, 3);
    t.push_response(b"abc");
    t.push_response(b"def");
    assert_eq!(t.drain_responses(), b"abcdef");
    assert!(t.drain_responses().is_empty());
}

// ---------------------------------------------------------------- kitty keyboard

#[test]
fn kitty_keyboard_push_pop_set_query() {
    let mut t = term(10, 3);
    assert_eq!(t.keyboard_flags(), 0);
    feed(&mut t, "\x1b[?u");
    assert_eq!(t.drain_responses(), b"\x1b[?0u");
    feed(&mut t, "\x1b[>1u");
    assert_eq!(t.keyboard_flags(), 1);
    feed(&mut t, "\x1b[>11u");
    assert_eq!(t.keyboard_flags(), 11);
    feed(&mut t, "\x1b[?u");
    assert_eq!(t.drain_responses(), b"\x1b[?11u");
    feed(&mut t, "\x1b[<u");
    assert_eq!(t.keyboard_flags(), 1);
    feed(&mut t, "\x1b[<u");
    assert_eq!(t.keyboard_flags(), 0);
    // Pop on empty stack stays at 0.
    feed(&mut t, "\x1b[<5u");
    assert_eq!(t.keyboard_flags(), 0);
}

#[test]
fn kitty_keyboard_set_modes() {
    let mut t = term(10, 3);
    feed(&mut t, "\x1b[=5u");
    assert_eq!(t.keyboard_flags(), 5);
    feed(&mut t, "\x1b[=2;2u");
    assert_eq!(t.keyboard_flags(), 7);
    feed(&mut t, "\x1b[=1;3u");
    assert_eq!(t.keyboard_flags(), 6);
    feed(&mut t, "\x1b[=8;1u");
    assert_eq!(t.keyboard_flags(), 8);
}

#[test]
fn kitty_keyboard_pop_multiple_and_stack_limit() {
    let mut t = term(10, 3);
    for i in 1..=3 {
        feed(&mut t, &format!("\x1b[>{}u", i));
    }
    feed(&mut t, "\x1b[<2u");
    assert_eq!(t.keyboard_flags(), 1);
    // Overflow drops the oldest entries, never panics.
    for _ in 0..40 {
        feed(&mut t, "\x1b[>1u");
    }
    for _ in 0..40 {
        feed(&mut t, "\x1b[<u");
    }
    assert_eq!(t.keyboard_flags(), 0);
}

#[test]
fn csi_u_without_intermediate_restores_cursor() {
    let mut t = term(10, 3);
    feed(&mut t, "\x1b[2;2H\x1b7\x1b[H\x1b[u");
    assert_eq!(t.cursor(), Cursor { col: 1, row: 1 });
    assert_eq!(t.keyboard_flags(), 0);
}

// ---------------------------------------------------------------- OSC

#[test]
fn osc_title_both_terminators() {
    let mut t = term(10, 3);
    feed(&mut t, "\x1b]0;hello world\x07");
    feed(&mut t, "\x1b]2;second; with semi\x1b\\");
    // Only the last title of a batch is reported.
    assert_eq!(t.drain_events(), vec![TermEvent::Title("second; with semi".into())]);
    assert_eq!(t.title(), "second; with semi");
}

#[test]
fn osc_cwd() {
    let mut t = term(10, 3);
    feed(&mut t, "\x1b]7;file://myhost/Users/me/my%20dir\x07");
    assert_eq!(
        t.drain_events(),
        vec![TermEvent::CwdChanged("/Users/me/my dir".into())]
    );
    assert_eq!(t.cwd(), Some("/Users/me/my dir"));
}

#[test]
fn osc_hyperlinks() {
    let mut t = term(20, 2);
    feed(&mut t, "a\x1b]8;;http://x.test/\x07link\x1b]8;;\x07b");
    assert_eq!(cell(&t, 0, 0).hyperlink_id, None);
    let id = cell(&t, 0, 1).hyperlink_id.expect("link id");
    assert_eq!(cell(&t, 0, 4).hyperlink_id, Some(id));
    assert_eq!(cell(&t, 0, 5).hyperlink_id, None);
    assert_eq!(t.hyperlink_uri(id), Some("http://x.test/"));
    // Same explicit id+uri reuses the id; different uri does not.
    feed(
        &mut t,
        "\r\n\x1b]8;id=k;http://a/\x1b\\p\x1b]8;;\x1b\\\x1b]8;id=k;http://a/\x1b\\q\x1b]8;;\x1b\\\x1b]8;id=k;http://b/\x1b\\r",
    );
    let p = cell(&t, 1, 0).hyperlink_id.unwrap();
    let q = cell(&t, 1, 1).hyperlink_id.unwrap();
    let r = cell(&t, 1, 2).hyperlink_id.unwrap();
    assert_eq!(p, q);
    assert_ne!(p, r);
    assert_eq!(t.hyperlink(r).unwrap().uri, "http://b/");
    assert_eq!(t.hyperlink(p).unwrap().id.as_deref(), Some("k"));
}

#[test]
fn osc_hyperlink_uri_with_semicolons() {
    let mut t = term(20, 2);
    feed(&mut t, "\x1b]8;;http://x/?a=1;b=2\x07z");
    let id = cell(&t, 0, 0).hyperlink_id.unwrap();
    assert_eq!(t.hyperlink_uri(id), Some("http://x/?a=1;b=2"));
}

#[test]
fn osc_clipboard_set_and_query() {
    let mut t = term(10, 3);
    feed(&mut t, "\x1b]52;c;aGVsbG8=\x07");
    assert_eq!(t.drain_events(), vec![TermEvent::ClipboardSet("hello".into())]);
    feed(&mut t, "\x1b]52;p;?\x07");
    assert_eq!(t.drain_events(), vec![TermEvent::ClipboardQuery]);
    t.respond_clipboard("hi");
    assert_eq!(t.drain_responses(), b"\x1b]52;p;aGk=\x1b\\");
    // Bad base64 is ignored.
    feed(&mut t, "\x1b]52;c;!!!\x07");
    assert!(t.drain_events().is_empty());
}

#[test]
fn osc_palette_set_query_reset() {
    let mut t = term(10, 3);
    feed(&mut t, "\x1b]4;1;rgb:ff/00/00\x07");
    assert_eq!(t.colors().palette[1], (255, 0, 0));
    assert_eq!(t.drain_events(), vec![TermEvent::ColorChanged]);
    feed(&mut t, "\x1b]4;1;?\x07");
    assert_eq!(t.drain_responses(), b"\x1b]4;1;rgb:ffff/0000/0000\x07");
    feed(&mut t, "\x1b]4;2;#00ff00;3;?\x1b\\");
    assert_eq!(t.colors().palette[2], (0, 255, 0));
    assert_eq!(
        t.drain_responses(),
        format!(
            "\x1b]4;3;{}\x1b\\",
            "rgb:cdcd/cdcd/0000"
        )
        .into_bytes()
    );
    feed(&mut t, "\x1b]104;1\x07");
    assert_eq!(t.colors().palette[1], default_palette_rgb(1));
    assert_eq!(t.colors().palette[2], (0, 255, 0));
    feed(&mut t, "\x1b]104\x07");
    assert_eq!(t.colors().palette[2], default_palette_rgb(2));
}

#[test]
fn osc_fg_bg_cursor_colors() {
    let mut t = term(10, 3);
    t.set_default_colors((1, 2, 3), (4, 5, 6), None);
    feed(&mut t, "\x1b]10;?\x07");
    assert_eq!(t.drain_responses(), b"\x1b]10;rgb:0101/0202/0303\x07");
    feed(&mut t, "\x1b]11;?\x1b\\");
    assert_eq!(t.drain_responses(), b"\x1b]11;rgb:0404/0505/0606\x1b\\");
    feed(&mut t, "\x1b]12;?\x07");
    assert_eq!(t.drain_responses(), b"\x1b]12;rgb:0101/0202/0303\x07");
    t.drain_events();
    feed(&mut t, "\x1b]10;#ffffff\x07\x1b]11;rgb:00/00/00\x07\x1b]12;#ff8000\x07");
    assert_eq!(t.colors().foreground, (255, 255, 255));
    assert_eq!(t.colors().background, (0, 0, 0));
    assert_eq!(t.colors().cursor, Some((255, 128, 0)));
    assert_eq!(t.drain_events(), vec![TermEvent::ColorChanged]);
    feed(&mut t, "\x1b]110\x07\x1b]111\x07\x1b]112\x07");
    assert_eq!(t.colors().foreground, (1, 2, 3));
    assert_eq!(t.colors().background, (4, 5, 6));
    assert_eq!(t.colors().cursor, None);
}

#[test]
fn osc_133_line_marks() {
    let mut t = term(20, 6);
    feed(&mut t, "\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07out\r\n\x1b]133;D;3\x07\x1b]133;A\x07$ ");
    let marks = t.line_marks();
    let kinds: Vec<_> = marks.iter().map(|(r, m)| (*r, m.kind)).collect();
    assert_eq!(
        kinds,
        vec![
            (0, MarkKind::PromptStart),
            (0, MarkKind::PromptEnd),
            (1, MarkKind::OutputStart),
            (2, MarkKind::CommandEnd),
            (2, MarkKind::PromptStart),
        ]
    );
    assert_eq!(marks[3].1.exit_code, Some(3));
    assert_eq!(t.marks_at(1).len(), 1);
    assert_eq!(
        t.find_mark(MarkKind::PromptStart, 2, Direction::Backward),
        Some(0)
    );
    assert_eq!(
        t.find_mark(MarkKind::PromptStart, 0, Direction::Forward),
        Some(2)
    );
    assert_eq!(t.find_mark(MarkKind::PromptStart, 0, Direction::Backward), None);
}

#[test]
fn line_marks_scroll_into_scrollback() {
    let mut t = Terminal::new(10, 2, 10);
    feed(&mut t, "\x1b]133;A\x07p\r\nb\r\nc\r\nd");
    assert_eq!(t.grid().scrollback_len(), 2);
    assert_eq!(t.line_marks()[0], (0, LineMark { kind: MarkKind::PromptStart, exit_code: None }));
}

// ---------------------------------------------------------------- APC / graphics

#[test]
fn apc_graphics_command_emitted() {
    let mut t = term(10, 3);
    feed(&mut t, "ab\x1b_Ga=T,f=100;AAAA\x1b\\cd");
    assert_eq!(
        t.drain_events(),
        vec![TermEvent::GraphicsCommand(b"a=T,f=100;AAAA".to_vec())]
    );
    // The APC is not interpreted as text.
    assert_eq!(snap(&t)[0], "abcd");
}

#[test]
fn apc_split_across_chunks_at_every_boundary() {
    let input = b"x\x1b_Gi=1,m=1;QUJD\x1b\\y\x1b_Gi=1;REVG\x1b\\z";
    for split in 0..=input.len() {
        let mut t = term(10, 2);
        t.advance(&input[..split]);
        t.advance(&input[split..]);
        let ev = t.drain_events();
        assert_eq!(
            ev,
            vec![
                TermEvent::GraphicsCommand(b"i=1,m=1;QUJD".to_vec()),
                TermEvent::GraphicsCommand(b"i=1;REVG".to_vec()),
            ],
            "split {}",
            split
        );
        assert_eq!(snap(&t)[0], "xyz", "split {}", split);
    }
}

#[test]
fn apc_byte_at_a_time() {
    let mut t = term(10, 2);
    for b in b"\x1b_Gq=2;zz\x1b\\ok" {
        t.advance(&[*b]);
    }
    assert_eq!(t.drain_events(), vec![TermEvent::GraphicsCommand(b"q=2;zz".to_vec())]);
    assert_eq!(snap(&t)[0], "ok");
}

#[test]
fn non_graphics_apc_is_swallowed() {
    let mut t = term(10, 2);
    feed(&mut t, "a\x1b_Xsomething\x1b\\b\x1b_\x1b\\c");
    assert!(t.drain_events().is_empty());
    assert_eq!(snap(&t)[0], "abc");
}

#[test]
fn apc_aborted_by_can_or_esc() {
    let mut t = term(10, 2);
    feed(&mut t, "a\x1b_Gjunk\x18b");
    assert!(t.drain_events().is_empty());
    assert_eq!(snap(&t)[0], "ab");
    // ESC followed by something other than `\` aborts and starts a new sequence.
    feed(&mut t, "\x1b_Gjunk\x1b[1;1Hz");
    assert!(t.drain_events().is_empty());
    assert_eq!(snap(&t)[0], "zb");
}

#[test]
fn apc_payload_with_high_bytes_and_esc_esc() {
    let mut t = term(10, 2);
    t.advance(b"\x1b\x1b_Ga=q;\xff\xfe\x1b\\!");
    assert_eq!(
        t.drain_events(),
        vec![TermEvent::GraphicsCommand(b"a=q;\xff\xfe".to_vec())]
    );
    assert_eq!(snap(&t)[0], "!");
}

#[test]
fn osc_interrupted_by_apc_does_not_break_parser() {
    let mut t = term(10, 2);
    feed(&mut t, "\x1b]0;tit\x1b_Gi=1\x1b\\after");
    assert_eq!(snap(&t)[0], "after");
    assert!(matches!(t.drain_events().last(), Some(TermEvent::GraphicsCommand(_))));
}

#[test]
fn move_cursor_rel_wraps_and_scrolls() {
    let mut t = term(5, 3);
    feed(&mut t, "\x1b[1;4H");
    t.move_cursor_rel(3, 0);
    assert_eq!(t.cursor(), Cursor { col: 1, row: 1 });
    assert!(t.grid().row(0).wrapped);
    t.move_cursor_rel(0, 5);
    assert_eq!(t.cursor().row, 2);
    assert_eq!(t.grid().scrolled_total(), 4);
    t.move_cursor_rel(-1, -1);
    assert_eq!(t.cursor(), Cursor { col: 0, row: 1 });
    t.move_cursor_rel(-100, -100);
    assert_eq!(t.cursor(), Cursor { col: 0, row: 0 });
}

// ---------------------------------------------------------------- resize

#[test]
fn resize_columns_rewraps_and_extends() {
    let mut t = term(8, 3);
    feed(&mut t, "abcdefgh");
    t.resize(4, 3);
    assert_eq!(snap(&t)[..2], ["abcd", "efgh"]);
    assert_eq!(t.cursor(), Cursor { col: 3, row: 1 });
    t.resize(10, 3);
    assert_eq!(snap(&t)[0], "abcdefgh");
    assert_eq!(t.cursor(), Cursor { col: 8, row: 0 });
    assert_eq!(t.size(), (10, 3));
    assert_eq!(t.grid().row(0).len(), 10);
}

#[test]
fn resize_truncating_wide_char_leaves_no_orphan() {
    let mut t = term(6, 2);
    feed(&mut t, "abcd日");
    t.resize(5, 2);
    assert!(!cell(&t, 0, 4).is_wide());
}

#[test]
fn resize_shrink_rows_pushes_to_scrollback_when_cursor_low() {
    let mut t = term(4, 4);
    feed(&mut t, "1\r\n2\r\n3\r\n4");
    t.resize(4, 2);
    assert_eq!(snap(&t), vec!["3", "4"]);
    assert_eq!(t.cursor().row, 1);
    assert_eq!(t.scrollback_len(), 2);
    // abs row of cursor stays consistent with content.
    assert_eq!(t.cursor_abs_row(), 3);
}

#[test]
fn resize_grow_rows_pulls_from_scrollback() {
    let mut t = term(4, 2);
    feed(&mut t, "1\r\n2\r\n3\r\n4");
    assert_eq!(t.scrollback_len(), 2);
    let abs = t.cursor_abs_row();
    t.resize(4, 4);
    assert_eq!(snap(&t), vec!["1", "2", "3", "4"]);
    assert_eq!(t.scrollback_len(), 0);
    assert_eq!(t.cursor_abs_row(), abs);
}

#[test]
fn resize_shrink_rows_drops_bottom_when_cursor_high() {
    let mut t = term(4, 4);
    feed(&mut t, "1\r\n2\x1b[H");
    t.resize(4, 2);
    assert_eq!(snap(&t), vec!["1", "2"]);
    assert_eq!(t.scrollback_len(), 0);
    assert_eq!(t.cursor().row, 0);
}

#[test]
fn resize_resets_region_and_clamps_cursor() {
    let mut t = term(10, 10);
    feed(&mut t, "\x1b[2;5r\x1b[9;9H");
    t.resize(5, 5);
    assert!(t.cursor().col < 5 && t.cursor().row < 5);
    let before = t.grid().scrolled_total();
    feed(&mut t, "\x1b[5;1H\n");
    assert_eq!(t.grid().scrolled_total(), before + 1);
}

#[test]
fn resize_while_on_alt_screen() {
    let mut t = term(6, 4);
    feed(&mut t, "main\x1b[?1049hALT");
    t.resize(8, 3);
    assert_eq!(t.screen(), Screen::Alt);
    assert_eq!(t.size(), (8, 3));
    feed(&mut t, "\x1b[?1049l");
    assert_eq!(snap(&t)[0], "main");
    assert_eq!(t.grid().rows(), 3);
}

#[test]
fn resize_noop_and_generic_ints() {
    let mut t = Terminal::new(80u16, 24u32, 1000usize);
    t.resize(80u16, 24u16);
    assert_eq!(t.size(), (80, 24));
    let t2 = Terminal::new(0, 0, 0);
    assert_eq!(t2.size(), (1, 1));
}

// ---------------------------------------------------------------- misc

#[test]
fn dec_graphics_charset() {
    let mut t = term(10, 1);
    feed(&mut t, "\x1b(0lqk\x1b(Bx");
    assert_eq!(snap(&t)[0], "┌─┐x");
    feed(&mut t, "\x1b[2J\x1b[H\x1b)0\x0eq\x0fq");
    assert_eq!(snap(&t)[0], "─q");
}

#[test]
fn ris_resets_state() {
    let mut t = term(10, 3);
    feed(&mut t, "\x1b[1;31mabc\x1b[?25l\x1b[?2004h\x1bc");
    assert_eq!(snap(&t), vec!["", "", ""]);
    assert!(t.cursor_visible());
    assert!(!t.bracketed_paste());
    feed(&mut t, "x");
    assert_eq!(cell(&t, 0, 0).fg, Color::Default);
}

#[test]
fn index_nel_and_ri() {
    let mut t = term(5, 3);
    feed(&mut t, "ab\x1bEc\x1bDd\x1bMe");
    assert_eq!(snap(&t), vec!["ab", "c e", " d"]);
    assert_eq!(t.cursor().row, 1);
}

#[test]
fn decaln_fills_screen() {
    let mut t = term(3, 2);
    feed(&mut t, "\x1b#8");
    assert_eq!(snap(&t), vec!["EEE", "EEE"]);
}

#[test]
fn display_helper_trims() {
    let mut t = term(5, 2);
    feed(&mut t, "ab  ");
    assert_eq!(lines(&t), "ab\n");
}

#[test]
fn visible_rows_iterates_styled_cells() {
    let mut t = term(4, 2);
    feed(&mut t, "\x1b[1;32mhi");
    let rows: Vec<&Row> = t.visible_rows().collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].cells[0].ch, 'h');
    assert_eq!(rows[0].cells[0].fg, Color::Indexed(2));
    assert!(rows[0].cells[1].bold());
    assert_eq!(t.colors().resolve(Color::Indexed(2), (0, 0, 0)), t.colors().palette[2]);
}

#[test]
fn ignores_garbage_without_panicking() {
    let mut t = term(7, 4);
    // Deterministic pseudo-random bytes, including partial UTF-8 and escapes.
    let mut x: u32 = 12345;
    let mut buf = Vec::new();
    for _ in 0..200_000 {
        x = x.wrapping_mul(1664525).wrapping_add(1013904223);
        buf.push((x >> 24) as u8);
    }
    for chunk in buf.chunks(97) {
        t.advance(chunk);
        if (chunk[0] & 7) == 0 {
            t.resize(3 + (chunk[1] % 9) as usize, 2 + (chunk[2] % 6) as usize);
        }
        t.drain_events();
        t.drain_responses();
        t.take_damage();
    }
    let (c, r) = t.size();
    assert!(t.cursor().col < c && t.cursor().row < r);
}

#[test]
fn escape_heavy_garbage_does_not_panic() {
    let mut t = term(9, 5);
    let alphabet = b"\x1b[]_P\\;:?>=<0123456789 mHJKrhlfGdXL@PMSTucnbq!$ABCDEF\x07\x18\x1a\xe6\x97\xa5";
    let mut x: u32 = 99;
    for _ in 0..300_000 {
        x = x.wrapping_mul(1103515245).wrapping_add(12345);
        let b = alphabet[(x >> 16) as usize % alphabet.len()];
        t.advance(&[b]);
        if x.is_multiple_of(5000) {
            t.drain_events();
            t.drain_responses();
        }
    }
    let (c, r) = t.size();
    assert!(t.cursor().col < c && t.cursor().row < r);
}

// ------------------------------------------------- reply amplification (F25)

#[test]
fn response_queue_is_capped_between_drains() {
    let mut t = term(10, 3);
    // XTVERSION: a 4-byte query with a larger answer.
    for _ in 0..100_000 {
        feed(&mut t, "\x1b[>q");
    }
    let r = t.drain_responses();
    assert!(!r.is_empty());
    assert!(r.len() <= crate::MAX_PENDING_RESPONSE_BYTES, "{} bytes", r.len());
    // Capacity is restored after a drain.
    feed(&mut t, "\x1b[5n");
    assert_eq!(t.drain_responses(), b"\x1b[0n");
}

#[test]
fn clipboard_queries_are_deduplicated_and_reply_is_bounded() {
    let mut t = term(10, 3);
    let mut flood = String::new();
    for _ in 0..1000 {
        flood.push_str("\x1b]52;c;?\x07");
    }
    feed(&mut t, &flood);
    assert_eq!(t.drain_events(), vec![TermEvent::ClipboardQuery]);
    // After a drain a new query is reported again.
    feed(&mut t, "\x1b]52;c;?\x07");
    assert_eq!(t.drain_events(), vec![TermEvent::ClipboardQuery]);

    // The selection echoed back is sanitised and short.
    let sel = "c".repeat(100_000) + "\u{1b}zz";
    feed(&mut t, &format!("\x1b]52;{sel};?\x07"));
    t.drain_events();
    t.respond_clipboard("x");
    let r = String::from_utf8(t.drain_responses()).unwrap();
    assert!(r.len() < 64, "{} bytes", r.len());

    // Oversized clipboard contents are answered empty, never truncated.
    let huge = "a".repeat(crate::MAX_CLIPBOARD_BYTES + 1);
    t.respond_clipboard(&huge);
    let r = String::from_utf8(t.drain_responses()).unwrap();
    assert!(r.ends_with(";\x1b\\"), "{r:?}");
    // A second reply while one is pending beyond the cap is not stacked.
    t.respond_clipboard(&"a".repeat(crate::MAX_CLIPBOARD_BYTES));
    t.respond_clipboard(&"a".repeat(crate::MAX_CLIPBOARD_BYTES));
    assert!(t.drain_responses().len() < crate::MAX_CLIPBOARD_BYTES * 2);
}

#[test]
fn event_floods_are_coalesced_and_bounded() {
    // F15: a BEL / title / clipboard flood queued one event per sequence.
    let mut t = term(10, 3);
    let flood: Vec<u8> = "\x07\x1b]0;t\x07\x1b]52;c;QQ==\x07".repeat(100_000).into_bytes();
    t.advance(&flood);
    assert!(t.events.len() < 4096, "queue grew to {}", t.events.len());
    let ev = t.drain_events();
    assert_eq!(ev.iter().filter(|e| matches!(e, TermEvent::Bell)).count(), 1);
    assert_eq!(ev.iter().filter(|e| matches!(e, TermEvent::Title(_))).count(), 1);
    assert_eq!(ev.iter().filter(|e| matches!(e, TermEvent::ClipboardSet(_))).count(), 1);
    // Graphics commands and distinct queries are not lost.
    let mut v = vec![TermEvent::GraphicsCommand(vec![1]), TermEvent::Bell, TermEvent::GraphicsCommand(vec![2]), TermEvent::Bell];
    coalesce_events(&mut v);
    assert_eq!(v, vec![TermEvent::GraphicsCommand(vec![1]), TermEvent::GraphicsCommand(vec![2]), TermEvent::Bell]);
}
