//! Text selection: click-drag, double-click word, triple-click line, Shift+click
//! extend, Ctrl+Alt+drag rectangle; text extraction for copying.
//!
//! Coordinates: selections are stored in the terminal's absolute row space
//! (`Terminal::abs_row_of_visible_row`), so they stay attached to their text
//! while the view scrolls. A selection is dropped when the text under it
//! changes (the program wrote to it), when the screen switches (main <->
//! alternate) and when its first line falls out of the scrollback.
//!
//! The renderer asks `contains` for every visible cell while `is_active`.

use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use grimalkin_vt::{CellWidth, Screen, Terminal};
use winit::event::MouseButton;
use winit::keyboard::ModifiersState;

/// Max gap between clicks to count as a multi-click.
const MULTI_CLICK: Duration = Duration::from_millis(400);
/// Bound on how many wrapped rows make up one logical line we will scan.
const MAX_LOGICAL_ROWS: u64 = 2000;

/// A cell position in the visible viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CellPos {
    pub col: usize,
    /// Visible row (0 = top of the viewport).
    pub row: usize,
}

/// A cell position in absolute row space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point {
    pub abs: u64,
    pub col: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Cell,
    Word,
    Line,
    Block,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    /// Button is down but the pointer has not left the first cell.
    Pressed,
    Active,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Space,
    Word,
    Other,
}

fn class(c: char) -> Class {
    if c == ' ' || c == '\0' || c == '\t' {
        Class::Space
    } else if c.is_alphanumeric() || "_-./~".contains(c) {
        Class::Word
    } else {
        Class::Other
    }
}

#[derive(Debug)]
pub struct Selection {
    mode: Mode,
    state: State,
    /// Where the press happened.
    anchor: Point,
    /// Word/line modes: the span of the originally clicked word or line.
    anchor_span: (Point, Point),
    head: Point,
    /// Normalised inclusive range (block mode: row range with the column
    /// bounds in `start.col` / `end.col`).
    range: Option<(Point, Point)>,
    button_down: bool,
    last_click: Option<(Instant, Point, u8)>,
    screen: Screen,
    alt_count: u64,
    fingerprint: u64,
    completed: bool,
}

impl Default for Selection {
    fn default() -> Self {
        let p = Point { abs: 0, col: 0 };
        Selection {
            mode: Mode::Cell,
            state: State::Idle,
            anchor: p,
            anchor_span: (p, p),
            head: p,
            range: None,
            button_down: false,
            last_click: None,
            screen: Screen::Main,
            alt_count: 0,
            fingerprint: 0,
            completed: false,
        }
    }
}

impl Selection {
    pub fn new() -> Self {
        Selection::default()
    }

    /// True when there is something to highlight. The renderer skips the
    /// per-cell `contains` calls when this is false.
    pub fn is_active(&self) -> bool {
        self.range.is_some()
    }

    pub fn clear(&mut self) {
        self.range = None;
        self.state = State::Idle;
        self.button_down = false;
    }

    /// Is the cell at `(abs_row, col)` selected?
    pub fn contains(&self, abs_row: u64, col: usize) -> bool {
        let Some((s, e)) = self.range else { return false };
        if abs_row < s.abs || abs_row > e.abs {
            return false;
        }
        if self.mode == Mode::Block {
            return col >= s.col && col <= e.col;
        }
        if abs_row == s.abs && col < s.col {
            return false;
        }
        !(abs_row == e.abs && col > e.col)
    }

    /// True once after a drag or multi-click selection finished; used for
    /// copy-on-select.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn take_completed(&mut self) -> bool {
        std::mem::take(&mut self.completed)
    }

    fn point(term: &Terminal, pos: CellPos) -> Point {
        Point {
            abs: term.abs_row_of_visible_row(pos.row),
            col: pos.col,
        }
    }

    /// Mouse button pressed over `pos`. Returns true if the selection changed.
    pub fn on_press(
        &mut self,
        term: &Terminal,
        pos: CellPos,
        button: MouseButton,
        mods: ModifiersState,
        now: Instant,
    ) -> bool {
        if button != MouseButton::Left {
            return false;
        }
        let p = Self::point(term, pos);
        self.button_down = true;

        // Shift+click extends the existing selection.
        if mods.shift_key() && self.range.is_some() {
            self.head = p;
            self.state = State::Active;
            self.update(term);
            return true;
        }

        let count = match self.last_click {
            Some((t, lp, n)) if now.saturating_duration_since(t) <= MULTI_CLICK && lp == p => n % 3 + 1,
            _ => 1,
        };
        self.last_click = Some((now, p, count));

        let had = self.range.is_some();
        self.range = None;
        self.screen = term.screen();
        self.alt_count = term.alt_enter_count();
        self.anchor = p;
        self.head = p;
        match count {
            1 => {
                self.mode = if mods.control_key() && mods.alt_key() {
                    Mode::Block
                } else {
                    Mode::Cell
                };
                self.state = State::Pressed;
            }
            n => {
                self.mode = if n == 2 { Mode::Word } else { Mode::Line };
                self.anchor_span = self.expand(term, p);
                self.state = State::Active;
                self.update(term);
            }
        }
        had || self.range.is_some()
    }

    /// Mouse moved with the left button held. Returns true if it changed.
    pub fn on_drag(&mut self, term: &Terminal, pos: CellPos) -> bool {
        if !self.button_down {
            return false;
        }
        let p = Self::point(term, pos);
        match self.state {
            State::Idle => false,
            State::Pressed if p == self.anchor => false,
            State::Pressed | State::Active => {
                if self.state == State::Active && p == self.head {
                    return false;
                }
                self.state = State::Active;
                self.head = p;
                self.update(term);
                true
            }
        }
    }

    /// Mouse button released. Returns true if the selection changed.
    pub fn on_release(&mut self, _term: &Terminal, _pos: CellPos, button: MouseButton) -> bool {
        if button != MouseButton::Left || !self.button_down {
            return false;
        }
        self.button_down = false;
        match self.state {
            State::Active if self.range.is_some() => self.completed = true,
            _ => self.state = State::Idle,
        }
        false
    }

    /// Re-check the selection after the terminal changed. Returns true if it
    /// was dropped (the caller should redraw).
    pub fn validate(&mut self, term: &Terminal) -> bool {
        let Some((s, e)) = self.range else { return false };
        if term.screen() != self.screen || term.alt_enter_count() != self.alt_count || s.abs < term.oldest_abs_row() {
            self.clear();
            return true;
        }
        let fp = self.hash_range(term, s, e);
        if self.button_down {
            // Still selecting; just track.
            self.fingerprint = fp;
            return false;
        }
        if fp != self.fingerprint {
            self.clear();
            return true;
        }
        false
    }

    /// The selected text, for copying. Soft-wrapped rows are joined without a
    /// newline; trailing blanks at the end of a line are dropped.
    pub fn selected_text(&self, term: &Terminal) -> Option<String> {
        let (s, e) = self.range?;
        let block = self.mode == Mode::Block;
        let mut out = String::new();
        for abs in s.abs..=e.abs {
            let Some(row) = term.row_at_abs(abs) else { continue };
            let len = row.cells.len();
            if len == 0 {
                continue;
            }
            let (c0, c1) = if block {
                (s.col, e.col)
            } else {
                (
                    if abs == s.abs { s.col } else { 0 },
                    if abs == e.abs { e.col } else { len - 1 },
                )
            };
            let c1 = c1.min(len - 1);
            let last_row = abs == e.abs;
            let mut line = String::new();
            if c0 <= c1 {
                for cell in &row.cells[c0..=c1] {
                    if cell.width == CellWidth::WideSpacer {
                        continue;
                    }
                    if cell.ch == '\0' {
                        line.push(' ');
                    } else {
                        cell.push_text(&mut line);
                    }
                }
            }
            let reaches_end = c1 + 1 >= len;
            let joins = !block && row.wrapped && reaches_end && !last_row;
            if !joins && (reaches_end || block) {
                let keep = line.trim_end_matches(' ').len();
                line.truncate(keep);
            }
            out.push_str(&line);
            if !joins && !last_row {
                out.push('\n');
            }
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    // -- geometry ----------------------------------------------------------

    /// Recompute `range` from anchor/head and refresh the fingerprint.
    fn update(&mut self, term: &Terminal) {
        let (a, h) = (self.anchor, self.head);
        let range = match self.mode {
            Mode::Cell => {
                let (mut s, mut e) = if a <= h { (a, h) } else { (h, a) };
                Self::widen(term, &mut s, &mut e);
                (s, e)
            }
            Mode::Block => {
                let (r0, r1) = (a.abs.min(h.abs), a.abs.max(h.abs));
                let (c0, c1) = (a.col.min(h.col), a.col.max(h.col));
                (Point { abs: r0, col: c0 }, Point { abs: r1, col: c1 })
            }
            Mode::Word | Mode::Line => {
                let span = self.expand(term, h);
                let s = span.0.min(self.anchor_span.0);
                let e = span.1.max(self.anchor_span.1);
                (s, e)
            }
        };
        self.range = Some(range);
        self.fingerprint = self.hash_range(term, range.0, range.1);
    }

    /// Make a cell-mode range cover whole wide characters.
    fn widen(term: &Terminal, s: &mut Point, e: &mut Point) {
        if let Some(row) = term.row_at_abs(s.abs) {
            if s.col > 0 && row.cells.get(s.col).is_some_and(|c| c.width == CellWidth::WideSpacer) {
                s.col -= 1;
            }
        }
        if let Some(row) = term.row_at_abs(e.abs) {
            if row.cells.get(e.col).is_some_and(|c| c.width == CellWidth::Wide) && e.col + 1 < row.cells.len() {
                e.col += 1;
            }
        }
    }

    fn expand(&self, term: &Terminal, p: Point) -> (Point, Point) {
        match self.mode {
            Mode::Line => expand_line(term, p),
            _ => expand_word(term, p),
        }
    }

    fn hash_range(&self, term: &Terminal, s: Point, e: Point) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for abs in s.abs..=e.abs {
            match term.row_at_abs(abs) {
                Some(row) => {
                    for c in &row.cells {
                        c.ch.hash(&mut h);
                        for &m in &c.combining {
                            m.hash(&mut h);
                        }
                    }
                    row.wrapped.hash(&mut h);
                }
                None => 0u8.hash(&mut h),
            }
        }
        h.finish()
    }
}

/// First and last absolute row of the logical (soft-wrapped) line holding `abs`.
fn logical_span(term: &Terminal, abs: u64) -> (u64, u64) {
    let oldest = term.oldest_abs_row();
    let mut first = abs;
    while first > oldest && abs - first < MAX_LOGICAL_ROWS && term.row_at_abs(first - 1).is_some_and(|r| r.wrapped) {
        first -= 1;
    }
    let mut last = abs;
    while last - abs < MAX_LOGICAL_ROWS
        && term.row_at_abs(last).is_some_and(|r| r.wrapped)
        && term.row_at_abs(last + 1).is_some()
    {
        last += 1;
    }
    (first, last)
}

fn expand_line(term: &Terminal, p: Point) -> (Point, Point) {
    let (first, last) = logical_span(term, p.abs);
    let len = term.row_at_abs(last).map_or(1, |r| r.cells.len().max(1));
    (Point { abs: first, col: 0 }, Point { abs: last, col: len - 1 })
}

fn expand_word(term: &Terminal, p: Point) -> (Point, Point) {
    let (first, last) = logical_span(term, p.abs);
    // Flatten the logical line into (abs, col, class).
    let mut cells: Vec<(u64, usize, Class)> = Vec::new();
    let mut at = None;
    for abs in first..=last {
        let Some(row) = term.row_at_abs(abs) else { continue };
        for (col, cell) in row.cells.iter().enumerate() {
            let cl = if cell.width == CellWidth::WideSpacer {
                cells.last().map_or(Class::Space, |c| c.2)
            } else {
                class(cell.ch)
            };
            if abs == p.abs && col == p.col {
                at = Some(cells.len());
            }
            cells.push((abs, col, cl));
        }
    }
    let Some(i) = at else { return (p, p) };
    let cl = cells[i].2;
    let mut a = i;
    while a > 0 && cells[a - 1].2 == cl {
        a -= 1;
    }
    let mut b = i;
    while b + 1 < cells.len() && cells[b + 1].2 == cl {
        b += 1;
    }
    (
        Point {
            abs: cells[a].0,
            col: cells[a].1,
        },
        Point {
            abs: cells[b].0,
            col: cells[b].1,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(cols: usize, rows: usize, text: &str) -> Terminal {
        let mut t = Terminal::new(cols, rows, 100usize);
        t.advance(text.as_bytes());
        t
    }

    fn pos(col: usize, row: usize) -> CellPos {
        CellPos { col, row }
    }

    fn click(sel: &mut Selection, t: &Terminal, p: CellPos, at: Instant) {
        let m = ModifiersState::empty();
        sel.on_press(t, p, MouseButton::Left, m, at);
        sel.on_release(t, p, MouseButton::Left);
    }

    fn drag(sel: &mut Selection, t: &Terminal, a: CellPos, b: CellPos, mods: ModifiersState) {
        sel.on_press(t, a, MouseButton::Left, mods, Instant::now());
        sel.on_drag(t, b);
        sel.on_release(t, b, MouseButton::Left);
    }

    #[test]
    fn simple_drag_copies_text() {
        let t = term(20, 5, "hello world\r\nsecond line");
        let mut s = Selection::new();
        drag(&mut s, &t, pos(6, 0), pos(10, 0), ModifiersState::empty());
        assert_eq!(s.selected_text(&t).as_deref(), Some("world"));
        assert!(s.contains(t.abs_row_of_visible_row(0), 6));
        assert!(!s.contains(t.abs_row_of_visible_row(0), 5));
    }

    #[test]
    fn click_without_drag_selects_nothing() {
        let t = term(20, 5, "hello");
        let mut s = Selection::new();
        click(&mut s, &t, pos(2, 0), Instant::now());
        assert!(!s.is_active());
        assert_eq!(s.selected_text(&t), None);
    }

    #[test]
    fn backwards_drag_and_multiline() {
        let t = term(20, 5, "abc\r\ndef\r\nghi");
        let mut s = Selection::new();
        drag(&mut s, &t, pos(1, 2), pos(1, 0), ModifiersState::empty());
        assert_eq!(s.selected_text(&t).as_deref(), Some("bc\ndef\ngh"));
    }

    #[test]
    fn trailing_blanks_trimmed_at_line_end() {
        let t = term(20, 5, "ab\r\ncd");
        let mut s = Selection::new();
        drag(&mut s, &t, pos(0, 0), pos(19, 1), ModifiersState::empty());
        assert_eq!(s.selected_text(&t).as_deref(), Some("ab\ncd"));
    }

    #[test]
    fn wrapped_rows_join_without_newline() {
        // 10 columns: 25 chars wrap over three rows.
        let t = term(10, 5, "abcdefghijklmnopqrstuvwxy\r\nnext");
        let mut s = Selection::new();
        drag(&mut s, &t, pos(0, 0), pos(9, 3), ModifiersState::empty());
        assert_eq!(s.selected_text(&t).as_deref(), Some("abcdefghijklmnopqrstuvwxy\nnext"));
    }

    #[test]
    fn triple_click_selects_whole_wrapped_line() {
        let t = term(10, 5, "abcdefghijklmnopqrstuvwxy\r\nnext");
        let mut s = Selection::new();
        let now = Instant::now();
        click(&mut s, &t, pos(3, 1), now);
        click(&mut s, &t, pos(3, 1), now);
        click(&mut s, &t, pos(3, 1), now);
        assert_eq!(s.selected_text(&t).as_deref(), Some("abcdefghijklmnopqrstuvwxy"));
        assert!(s.contains(t.abs_row_of_visible_row(0), 0));
        assert!(s.contains(t.abs_row_of_visible_row(2), 4));
        assert!(!s.contains(t.abs_row_of_visible_row(3), 0));
    }

    #[test]
    fn double_click_word_chars() {
        let t = term(40, 5, "see ~/src/foo-bar.rs, ok");
        let mut s = Selection::new();
        let now = Instant::now();
        click(&mut s, &t, pos(8, 0), now);
        click(&mut s, &t, pos(8, 0), now);
        assert_eq!(s.selected_text(&t).as_deref(), Some("~/src/foo-bar.rs"));
        // Whitespace is its own run.
        let mut s = Selection::new();
        click(&mut s, &t, pos(3, 0), now);
        click(&mut s, &t, pos(3, 0), now);
        assert_eq!(s.selected_text(&t).as_deref(), Some(" "));
    }

    #[test]
    fn click_count_cycles_and_resets() {
        let t = term(40, 5, "one two");
        let mut s = Selection::new();
        let now = Instant::now();
        click(&mut s, &t, pos(1, 0), now);
        click(&mut s, &t, pos(1, 0), now);
        assert_eq!(s.selected_text(&t).as_deref(), Some("one"));
        click(&mut s, &t, pos(1, 0), now);
        assert_eq!(s.selected_text(&t).as_deref(), Some("one two"));
        // A fourth click starts over with a plain click.
        click(&mut s, &t, pos(1, 0), now);
        assert!(!s.is_active());
        // A late second click is a fresh single click.
        click(&mut s, &t, pos(5, 0), now + Duration::from_secs(2));
        assert!(!s.is_active());
    }

    #[test]
    fn word_drag_extends_by_words() {
        let t = term(40, 5, "alpha beta gamma");
        let mut s = Selection::new();
        let now = Instant::now();
        let m = ModifiersState::empty();
        click(&mut s, &t, pos(1, 0), now);
        s.on_press(&t, pos(1, 0), MouseButton::Left, m, now);
        s.on_drag(&t, pos(12, 0));
        s.on_release(&t, pos(12, 0), MouseButton::Left);
        assert_eq!(s.selected_text(&t).as_deref(), Some("alpha beta gamma"));
    }

    #[test]
    fn shift_click_extends() {
        let t = term(40, 5, "0123456789");
        let mut s = Selection::new();
        drag(&mut s, &t, pos(2, 0), pos(3, 0), ModifiersState::empty());
        assert_eq!(s.selected_text(&t).as_deref(), Some("23"));
        s.on_press(&t, pos(7, 0), MouseButton::Left, ModifiersState::SHIFT, Instant::now());
        s.on_release(&t, pos(7, 0), MouseButton::Left);
        assert_eq!(s.selected_text(&t).as_deref(), Some("234567"));
    }

    #[test]
    fn wide_chars_are_selected_whole() {
        let t = term(20, 5, "a\u{6f22}\u{5b57}b");
        // Columns: a=0, first wide=1-2, second wide=3-4, b=5.
        let mut s = Selection::new();
        drag(&mut s, &t, pos(2, 0), pos(3, 0), ModifiersState::empty());
        assert_eq!(s.selected_text(&t).as_deref(), Some("\u{6f22}\u{5b57}"));
        let abs = t.abs_row_of_visible_row(0);
        assert!(s.contains(abs, 1) && s.contains(abs, 4));
        assert!(!s.contains(abs, 0) && !s.contains(abs, 5));
    }

    #[test]
    fn block_selection() {
        let t = term(20, 5, "abcdef\r\nghijkl\r\nmnopqr");
        let mut s = Selection::new();
        let mods = ModifiersState::CONTROL | ModifiersState::ALT;
        drag(&mut s, &t, pos(1, 0), pos(3, 2), mods);
        assert_eq!(s.selected_text(&t).as_deref(), Some("bcd\nhij\nnop"));
        let abs = t.abs_row_of_visible_row(1);
        assert!(s.contains(abs, 1) && !s.contains(abs, 4));
    }

    #[test]
    fn selection_follows_scrollback() {
        let mut t = Terminal::new(20, 3, 100usize);
        t.advance(b"first\r\nsecond\r\nthird");
        let mut s = Selection::new();
        drag(&mut s, &t, pos(0, 0), pos(4, 0), ModifiersState::empty());
        assert_eq!(s.selected_text(&t).as_deref(), Some("first"));
        t.advance(b"\r\nfourth\r\nfifth");
        // Text moved into scrollback but is unchanged: selection survives.
        assert!(!s.validate(&t));
        assert_eq!(s.selected_text(&t).as_deref(), Some("first"));
        t.scroll_display(2);
        assert!(s.contains(t.abs_row_of_visible_row(0), 0));
        assert!(s.contains(t.abs_row_of_visible_row(0), 4));
    }

    #[test]
    fn writes_into_selection_clear_it() {
        let mut t = Terminal::new(20, 3, 100usize);
        t.advance(b"hello");
        let mut s = Selection::new();
        drag(&mut s, &t, pos(0, 0), pos(4, 0), ModifiersState::empty());
        assert!(s.is_active());
        t.advance(b"\rHELLO");
        assert!(s.validate(&t));
        assert!(!s.is_active());
    }

    #[test]
    fn alt_screen_clears() {
        let mut t = Terminal::new(20, 3, 100usize);
        t.advance(b"hello");
        let mut s = Selection::new();
        drag(&mut s, &t, pos(0, 0), pos(4, 0), ModifiersState::empty());
        t.advance(b"\x1b[?1049h");
        assert!(s.validate(&t));
        assert!(!s.is_active());
    }

    #[test]
    fn trimmed_scrollback_clears() {
        let mut t = Terminal::new(10, 2, 3usize);
        t.advance(b"a\r\nb");
        let mut s = Selection::new();
        drag(&mut s, &t, pos(0, 0), pos(0, 1), ModifiersState::empty());
        for _ in 0..10 {
            t.advance(b"\r\nx");
        }
        assert!(s.validate(&t));
        assert!(!s.is_active());
    }
}
