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

/// True when the last cell of the soft-wrapped `row` is the blank pad left
/// because the wide character at the start of `next` did not fit (the
/// terminal marks that cell `Flags::WRAP_PAD`; a typed space is not marked).
pub fn wide_wrap_pad(row: &grimalkin_vt::Row, next: Option<&grimalkin_vt::Row>) -> bool {
    row.wrapped
        && row.cells.last().is_some_and(|c| c.flags.contains(grimalkin_vt::Flags::WRAP_PAD))
        && next.and_then(|n| n.cells.first()).is_some_and(|c| c.width == CellWidth::Wide)
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
    /// Hashes of scrollback rows (which never change) under the selection.
    row_cache: RowHashCache,
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
            row_cache: RowHashCache::default(),
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
            let pad = c1 + 1 >= len && wide_wrap_pad(row, term.row_at_abs(abs + 1));
            if c0 <= c1 {
                let cells = &row.cells[c0..=c1];
                // A wide char that did not fit at the end of a soft-wrapped
                // row leaves an erased pad cell there; it is not text.
                let cells = if pad { &cells[..cells.len() - 1] } else { cells };
                for cell in cells {
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

    /// A fingerprint of the text under `s..=e`, to notice when a program
    /// writes over it. Rows that have scrolled into history cannot change, so
    /// their hashes are computed once and kept; only the live screen's rows
    /// are rehashed on each call. (Rehashing every cell of a 100k-line
    /// selection took ~40 ms per drag event and per pty chunk.)
    fn hash_range(&mut self, term: &Terminal, s: Point, e: Point) -> u64 {
        let live_top = term.abs_row_of_visible_row(0) + term.display_offset() as u64;
        self.row_cache.prepare(term.screen(), term.cols(), term.oldest_abs_row(), s.abs, live_top);
        let mut acc = 0xcbf2_9ce4_8422_2325u64;
        for abs in s.abs..=e.abs {
            let rh = if abs < live_top {
                self.row_cache.get_or_hash(term, abs)
            } else {
                row_hash(term, abs)
            };
            acc = (acc ^ rh).wrapping_mul(0x0000_0100_0000_01b3).rotate_left(5);
        }
        acc
    }
}

/// Hash of one row's text (and its wrap flag); rows outside the buffer hash
/// as zero.
fn row_hash(term: &Terminal, abs: u64) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
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
    h.finish()
}

/// Cached [`row_hash`]es for a contiguous run of scrollback rows.
#[derive(Debug, Default)]
struct RowHashCache {
    screen: Option<Screen>,
    cols: usize,
    first: u64,
    hashes: Vec<u64>,
}

impl RowHashCache {
    /// Drop the cache when it can no longer be trusted: another screen, a
    /// resize, history trimmed or cleared from under it, or a start row the
    /// run does not cover.
    fn prepare(&mut self, screen: Screen, cols: usize, oldest: u64, start: u64, live_top: u64) {
        let end = self.first + self.hashes.len() as u64;
        let stale = self.screen != Some(screen)
            || self.cols != cols
            || self.first < oldest
            || end > live_top
            || start < self.first
            || start > end;
        if stale {
            self.screen = Some(screen);
            self.cols = cols;
            self.first = start;
            self.hashes.clear();
        }
    }

    /// The hash of scrollback row `abs` (must be `>= first`); rows are cached
    /// in order, so a miss extends the run up to and including `abs`.
    fn get_or_hash(&mut self, term: &Terminal, abs: u64) -> u64 {
        let idx = (abs - self.first) as usize;
        while self.hashes.len() <= idx {
            let next = self.first + self.hashes.len() as u64;
            self.hashes.push(row_hash(term, next));
        }
        self.hashes[idx]
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
    fn wide_char_wrap_pad_is_not_copied() {
        // F38: a spurious space appeared where a wide char wrapped.
        let t = term(10, 4, "aaaaaaaaa\u{4e16}bbb");
        let mut s = Selection::new();
        drag(&mut s, &t, pos(0, 0), pos(9, 1), ModifiersState::empty());
        assert_eq!(s.selected_text(&t).as_deref(), Some("aaaaaaaaa\u{4e16}bbb"));
        let t = term(3, 4, "\u{4e16}\u{4e16}");
        let mut s = Selection::new();
        drag(&mut s, &t, pos(0, 0), pos(2, 1), ModifiersState::empty());
        assert_eq!(s.selected_text(&t).as_deref(), Some("\u{4e16}\u{4e16}"));
        // A real space before a wrapped wide char is kept.
        let t = term(10, 4, "aaaaaaaa \u{4e16}b");
        let mut s = Selection::new();
        drag(&mut s, &t, pos(0, 0), pos(9, 1), ModifiersState::empty());
        assert_eq!(s.selected_text(&t).as_deref(), Some("aaaaaaaa \u{4e16}b"));
    }

    #[test]
    fn scrollback_rows_are_hashed_once_and_changes_still_clear_the_selection() {
        // F44: a large selection rehashed every cell on each drag event.
        let mut text = String::new();
        for i in 0..20_000 {
            text.push_str(&format!("line number {i} with some text to hash\r\n"));
        }
        let mut t = Terminal::new(80, 24, 100_000);
        t.advance(text.as_bytes());
        let mut s = Selection::new();
        let top = CellPos { col: 0, row: 0 };
        let bot = CellPos { col: 5, row: 23 };
        s.on_press(&t, top, MouseButton::Left, ModifiersState::empty(), Instant::now());
        // Scroll the view up so the selection spans history and the live screen.
        t.scroll_display(200);
        s.on_drag(&t, bot);
        s.on_release(&t, bot, MouseButton::Left);
        assert!(s.is_active());
        let (a, b) = s.range.unwrap();
        assert!(b.abs - a.abs > 20, "selection should reach into scrollback");
        // Warm: the whole scrollback part is cached.
        assert!(!s.validate(&t));
        assert!(s.row_cache.hashes.len() as u64 >= (b.abs - a.abs).saturating_sub(24));
        let t0 = Instant::now();
        for _ in 0..50 {
            assert!(!s.validate(&t));
        }
        assert!(t0.elapsed().as_millis() < 500, "{:?}", t0.elapsed());
        // New output elsewhere keeps the selection; overwriting selected text drops it.
        t.advance(b"more output\r\n");
        assert!(!s.validate(&t));
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
