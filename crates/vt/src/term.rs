//! The `Terminal` state machine: state, public API, printing and cursor ops.
//! Escape-sequence dispatch lives in `perform.rs`.

use crate::cell::*;
use crate::grid::Grid;
use crate::util;
use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Which screen buffer is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Screen {
    Main,
    Alt,
}

/// Cursor position (0-based, relative to the visible grid).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cursor {
    pub col: usize,
    pub row: usize,
}

/// Cursor shape (DECSCUSR).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
}

/// Mouse reporting mode (1000/1002/1003).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MouseMode {
    #[default]
    None,
    /// 1000: report button press/release.
    Press,
    /// 1002: also report motion while a button is held.
    Drag,
    /// 1003: report all motion.
    Any,
}

/// Mouse coordinate encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MouseEncoding {
    #[default]
    Default,
    /// 1005
    Utf8,
    /// 1006
    Sgr,
    /// 1015
    Urxvt,
}

/// DEC private / ANSI modes the terminal tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Modes {
    /// DECCKM (1)
    pub app_cursor: bool,
    /// DECSCNM (5)
    pub reverse_video: bool,
    /// DECOM (6)
    pub origin: bool,
    /// DECAWM (7)
    pub wrap: bool,
    /// Cursor blink (12)
    pub cursor_blink: bool,
    /// DECTCEM (25)
    pub cursor_visible: bool,
    /// IRM (ANSI 4)
    pub insert: bool,
    pub mouse_mode: MouseMode,
    pub mouse_encoding: MouseEncoding,
    /// Focus in/out reporting (1004)
    pub focus_reporting: bool,
    /// Bracketed paste (2004)
    pub bracketed_paste: bool,
    /// Synchronized output (2026)
    pub synchronized_output: bool,
    /// DECKPAM / DECKPNM
    pub app_keypad: bool,
}

impl Default for Modes {
    fn default() -> Self {
        Modes {
            app_cursor: false,
            reverse_video: false,
            origin: false,
            wrap: true,
            cursor_blink: true,
            cursor_visible: true,
            insert: false,
            mouse_mode: MouseMode::None,
            mouse_encoding: MouseEncoding::Default,
            focus_reporting: false,
            bracketed_paste: false,
            synchronized_output: false,
            app_keypad: false,
        }
    }
}

/// Events queued for the embedding application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermEvent {
    Title(String),
    Bell,
    /// OSC 52 set: decoded clipboard text.
    ClipboardSet(String),
    /// OSC 52 query; answer with [`Terminal::respond_clipboard`].
    ClipboardQuery,
    /// OSC 7: working directory as a plain path (file:// URIs are decoded).
    CwdChanged(String),
    /// Palette, default fg/bg or cursor colour changed (OSC 4/10/11/12/104/...).
    ColorChanged,
    CursorShape(CursorShape),
    /// Mouse mode / encoding / focus reporting changed; read via `modes()`.
    SetMouseMode,
    /// APC `G` payload (bytes between `ESC _ G` and `ESC \`, exclusive of `G`).
    GraphicsCommand(Vec<u8>),
}

/// RGB triple.
pub type Rgb = (u8, u8, u8);

/// Palette and default colours, with OSC-set overrides on top of base values.
#[derive(Debug, Clone)]
pub struct ColorTable {
    pub palette: [Rgb; 256],
    pub foreground: Rgb,
    pub background: Rgb,
    /// `None` = renderer should use the foreground colour.
    pub cursor: Option<Rgb>,
    base_palette: [Rgb; 256],
    base_foreground: Rgb,
    base_background: Rgb,
    base_cursor: Option<Rgb>,
}

impl ColorTable {
    fn new() -> Self {
        let mut palette = [(0, 0, 0); 256];
        for (i, p) in palette.iter_mut().enumerate() {
            *p = default_palette_rgb(i as u8);
        }
        ColorTable {
            palette,
            foreground: (0xe5, 0xe5, 0xe5),
            background: (0x1e, 0x1e, 0x1e),
            cursor: None,
            base_palette: palette,
            base_foreground: (0xe5, 0xe5, 0xe5),
            base_background: (0x1e, 0x1e, 0x1e),
            base_cursor: None,
        }
    }

    /// Resolve a [`Color`] to RGB; `Default` resolves to `default`.
    pub fn resolve(&self, c: Color, default: Rgb) -> Rgb {
        match c {
            Color::Default => default,
            Color::Indexed(i) => self.palette[i as usize],
            Color::Rgb(r, g, b) => (r, g, b),
        }
    }
}

/// Damage reported to the renderer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Damage {
    #[default]
    None,
    /// Everything must be redrawn.
    Full,
    /// Only these visible rows (ascending) changed.
    Rows(Vec<usize>),
}

/// A hyperlink (OSC 8) referenced by `Cell::hyperlink_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hyperlink {
    pub id: Option<String>,
    pub uri: String,
}

/// Direction for mark searches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Backward,
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Charset {
    Ascii,
    DecGraphics,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SavedCursor {
    pub col: usize,
    pub row: usize,
    pub pen: Style,
    pub origin: bool,
    pub pending_wrap: bool,
    pub charsets: [Charset; 4],
    pub gl: usize,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct KbState {
    pub flags: u8,
    pub stack: Vec<u8>,
}

pub(crate) const KB_STACK_LIMIT: usize = 16;
/// Synchronized output (mode 2026) is force-ended after this long.
pub(crate) const SYNC_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_APC: usize = 64 * 1024 * 1024;
pub(crate) const MAX_HYPERLINKS: usize = 1 << 17;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApcState {
    Ground,
    Esc,
    Apc,
    ApcEsc,
}

/// The terminal emulator state machine.
pub struct Terminal {
    parser: Option<vte::Parser>,
    apc: ApcState,
    apc_buf: Vec<u8>,
    apc_overflow: bool,

    pub(crate) main: Grid,
    pub(crate) alt: Grid,
    pub(crate) active: Screen,
    pub(crate) cols: usize,
    pub(crate) rows: usize,

    pub(crate) cursor: Cursor,
    pub(crate) pending_wrap: bool,
    pub(crate) pen: Style,
    pub(crate) region_top: usize,
    pub(crate) region_bottom: usize,
    pub(crate) tabs: Vec<bool>,
    pub(crate) modes: Modes,
    pub(crate) charsets: [Charset; 4],
    pub(crate) gl: usize,
    pub(crate) saved: [Option<SavedCursor>; 2],
    pub(crate) kb: [KbState; 2],
    display_offset: usize,
    pub(crate) cursor_shape: CursorShape,
    pub(crate) title: String,
    pub(crate) cwd: Option<String>,
    pub(crate) colors: ColorTable,
    pub(crate) hyperlinks: Vec<Hyperlink>,
    pub(crate) hyperlink_map: HashMap<(String, String), u32>,
    pub(crate) responses: Vec<u8>,
    pub(crate) events: Vec<TermEvent>,
    dirty: Vec<bool>,
    full_redraw: bool,
    pub(crate) last_cell: Option<(usize, usize)>,
    pub(crate) last_char: Option<char>,
    pub(crate) cell_px: (u32, u32),
    alt_enter_count: u64,
    pub(crate) display_clear_count: u64,
    sync_since: Option<Instant>,
    pub(crate) clip_selection: String,
    drawn_cursor: (Cursor, bool, CursorShape, bool),
}

fn to_usize<T: TryInto<usize>>(v: T) -> usize {
    v.try_into().unwrap_or(0)
}

impl Terminal {
    /// Create a terminal. Integer arguments of any type are accepted for
    /// ergonomics; sizes are clamped to at least 1.
    pub fn new<C, R, S>(cols: C, rows: R, scrollback_limit: S) -> Terminal
    where
        C: TryInto<usize>,
        R: TryInto<usize>,
        S: TryInto<usize>,
    {
        let cols = to_usize(cols).max(1);
        let rows = to_usize(rows).max(1);
        let sb = to_usize(scrollback_limit);
        let mut t = Terminal {
            parser: Some(vte::Parser::new()),
            apc: ApcState::Ground,
            apc_buf: Vec::new(),
            apc_overflow: false,
            main: Grid::new(cols, rows, sb),
            alt: Grid::new(cols, rows, 0),
            active: Screen::Main,
            cols,
            rows,
            cursor: Cursor::default(),
            pending_wrap: false,
            pen: Style::default(),
            region_top: 0,
            region_bottom: rows - 1,
            tabs: Vec::new(),
            modes: Modes::default(),
            charsets: [Charset::Ascii; 4],
            gl: 0,
            saved: [None, None],
            kb: [KbState::default(), KbState::default()],
            display_offset: 0,
            cursor_shape: CursorShape::Block,
            title: String::new(),
            cwd: None,
            colors: ColorTable::new(),
            hyperlinks: Vec::new(),
            hyperlink_map: HashMap::new(),
            responses: Vec::new(),
            events: Vec::new(),
            dirty: vec![true; rows],
            full_redraw: true,
            last_cell: None,
            last_char: None,
            cell_px: (8, 16),
            alt_enter_count: 0,
            display_clear_count: 0,
            sync_since: None,
            clip_selection: "c".into(),
            drawn_cursor: (Cursor::default(), true, CursorShape::Block, true),
        };
        t.reset_tabs();
        t
    }

    // ------------------------------------------------------------------
    // Feeding bytes
    // ------------------------------------------------------------------

    /// Feed bytes received from the pty.
    pub fn advance(&mut self, bytes: &[u8]) {
        self.check_sync_timeout(SYNC_TIMEOUT);
        let mut parser = self.parser.take().unwrap_or_default();
        let mut seg = 0usize;
        let mut i = 0usize;
        while i < bytes.len() {
            let b = bytes[i];
            match self.apc {
                ApcState::Ground => {
                    if b == 0x1b {
                        self.apc = ApcState::Esc;
                    }
                    i += 1;
                }
                ApcState::Esc => match b {
                    b'_' => {
                        // Hand `ESC _` to vte (it will swallow the string),
                        // then collect the payload ourselves.
                        parser.advance(self, &bytes[seg..=i]);
                        i += 1;
                        seg = i;
                        self.apc = ApcState::Apc;
                        self.apc_buf.clear();
                        self.apc_overflow = false;
                    }
                    0x1b => i += 1,
                    _ => {
                        self.apc = ApcState::Ground;
                        i += 1;
                    }
                },
                ApcState::Apc => {
                    match b {
                        0x1b => self.apc = ApcState::ApcEsc,
                        0x18 | 0x1a => {
                            // CAN / SUB abort the string.
                            parser.advance(self, &[b]);
                            self.apc = ApcState::Ground;
                        }
                        _ => {
                            if self.apc_buf.len() < MAX_APC {
                                self.apc_buf.push(b);
                            } else {
                                self.apc_overflow = true;
                            }
                        }
                    }
                    i += 1;
                    seg = i;
                }
                ApcState::ApcEsc => {
                    if b == b'\\' {
                        let buf = std::mem::take(&mut self.apc_buf);
                        if !self.apc_overflow && buf.first() == Some(&b'G') {
                            self.events
                                .push(TermEvent::GraphicsCommand(buf[1..].to_vec()));
                        }
                        self.apc_overflow = false;
                        // Let vte leave its APC state.
                        parser.advance(self, &[0x1b, b'\\']);
                        self.apc = ApcState::Ground;
                        i += 1;
                        seg = i;
                    } else {
                        // ESC aborted the APC and starts a new sequence.
                        self.apc_buf.clear();
                        parser.advance(self, &[0x1b]);
                        self.apc = ApcState::Esc;
                        seg = i; // reprocess `b` in Esc state
                    }
                }
            }
        }
        if seg < bytes.len() && !matches!(self.apc, ApcState::Apc | ApcState::ApcEsc) {
            parser.advance(self, &bytes[seg..]);
        }
        self.parser = Some(parser);
        self.update_cursor_damage();
    }

    /// Drain bytes the terminal wants written back to the pty.
    pub fn drain_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.responses)
    }

    /// Queue raw bytes to be sent to the pty (used by the graphics glue).
    pub fn push_response(&mut self, bytes: &[u8]) {
        self.responses.extend_from_slice(bytes);
    }

    /// Drain queued events.
    pub fn drain_events(&mut self) -> Vec<TermEvent> {
        std::mem::take(&mut self.events)
    }

    /// Answer a [`TermEvent::ClipboardQuery`] with the clipboard contents.
    pub fn respond_clipboard(&mut self, text: &str) {
        let r = format!(
            "\x1b]52;{};{}\x1b\\",
            self.clip_selection,
            util::base64_encode(text.as_bytes())
        );
        self.push_response(r.as_bytes());
    }

    // ------------------------------------------------------------------
    // Accessors
    // ------------------------------------------------------------------

    /// `(cols, rows)`
    pub fn size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }
    pub fn cols(&self) -> usize {
        self.cols
    }
    pub fn rows(&self) -> usize {
        self.rows
    }
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }
    pub fn screen(&self) -> Screen {
        self.active
    }
    /// The active screen's grid (visible lines + scrollback).
    pub fn grid(&self) -> &Grid {
        match self.active {
            Screen::Main => &self.main,
            Screen::Alt => &self.alt,
        }
    }
    pub(crate) fn grid_mut(&mut self) -> &mut Grid {
        match self.active {
            Screen::Main => &mut self.main,
            Screen::Alt => &mut self.alt,
        }
    }
    pub(crate) fn sidx(&self) -> usize {
        match self.active {
            Screen::Main => 0,
            Screen::Alt => 1,
        }
    }
    pub fn modes(&self) -> &Modes {
        &self.modes
    }
    pub fn colors(&self) -> &ColorTable {
        &self.colors
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }
    pub fn cursor_shape(&self) -> CursorShape {
        self.cursor_shape
    }
    pub fn cursor_visible(&self) -> bool {
        self.modes.cursor_visible
    }
    pub fn cursor_blink(&self) -> bool {
        self.modes.cursor_blink
    }
    pub fn mouse_mode(&self) -> MouseMode {
        self.modes.mouse_mode
    }
    pub fn mouse_encoding(&self) -> MouseEncoding {
        self.modes.mouse_encoding
    }
    pub fn bracketed_paste(&self) -> bool {
        self.modes.bracketed_paste
    }
    pub fn focus_reporting(&self) -> bool {
        self.modes.focus_reporting
    }
    pub fn app_cursor_keys(&self) -> bool {
        self.modes.app_cursor
    }
    pub fn synchronized_output(&self) -> bool {
        self.modes.synchronized_output
    }
    /// Current kitty keyboard protocol flags for the active screen.
    pub fn keyboard_flags(&self) -> u8 {
        self.kb[self.sidx()].flags
    }
    pub fn cell_pixel_size(&self) -> (u32, u32) {
        self.cell_px
    }
    pub fn set_cell_pixel_size<W: TryInto<u32>, H: TryInto<u32>>(&mut self, w: W, h: H) {
        self.cell_px = (w.try_into().unwrap_or(0), h.try_into().unwrap_or(0));
    }
    /// Number of times the alternate screen has been entered.
    pub fn alt_enter_count(&self) -> u64 {
        self.alt_enter_count
    }
    /// Incremented on every ED 2 (erase whole display) and RIS, so a host can
    /// drop things that live outside the grid (kitty graphics placements).
    pub fn display_clear_count(&self) -> u64 {
        self.display_clear_count
    }
    pub fn hyperlink(&self, id: u32) -> Option<&Hyperlink> {
        self.hyperlinks.get(id as usize)
    }
    pub fn hyperlink_uri(&self, id: u32) -> Option<&str> {
        self.hyperlink(id).map(|h| h.uri.as_str())
    }

    /// Set the default (base) foreground / background / cursor colours, e.g.
    /// from the user's config. Also used as the target of OSC 110/111/112.
    pub fn set_default_colors(&mut self, fg: Rgb, bg: Rgb, cursor: Option<Rgb>) {
        self.colors.foreground = fg;
        self.colors.background = bg;
        self.colors.cursor = cursor;
        self.colors.base_foreground = fg;
        self.colors.base_background = bg;
        self.colors.base_cursor = cursor;
        self.mark_all();
    }
    /// Set a base palette entry (also resets any OSC 4 override of it).
    pub fn set_palette_color(&mut self, idx: u8, rgb: Rgb) {
        self.colors.palette[idx as usize] = rgb;
        self.colors.base_palette[idx as usize] = rgb;
        self.mark_all();
    }
    pub(crate) fn reset_palette_entry(&mut self, idx: usize) {
        self.colors.palette[idx] = self.colors.base_palette[idx];
    }
    pub(crate) fn reset_dyn_color(&mut self, which: u8) {
        match which {
            10 => self.colors.foreground = self.colors.base_foreground,
            11 => self.colors.background = self.colors.base_background,
            _ => self.colors.cursor = self.colors.base_cursor,
        }
    }

    // ------------------------------------------------------------------
    // Visible rows / scrollback viewing
    // ------------------------------------------------------------------

    /// Current scrollback view offset in lines (0 = live view). Always 0 on
    /// the alternate screen.
    pub fn display_offset(&self) -> usize {
        match self.active {
            Screen::Main => self.display_offset,
            Screen::Alt => 0,
        }
    }
    pub fn scrollback_len(&self) -> usize {
        self.grid().scrollback_len()
    }
    /// Change the scrollback limit of the main screen at runtime. Returns
    /// true if history rows were dropped (absolute row numbers stay valid,
    /// but the oldest ones no longer exist).
    pub fn set_scrollback_limit(&mut self, limit: usize) -> bool {
        let dropped = self.main.set_scrollback_limit(limit) > 0;
        self.display_offset = self.display_offset.min(self.main.scrollback_len());
        dropped
    }
    /// Scroll the viewport; positive `delta` moves toward older history.
    /// Returns true if the offset changed.
    pub fn scroll_display(&mut self, delta: i32) -> bool {
        if self.active == Screen::Alt {
            return false;
        }
        let max = self.main.scrollback_len() as i64;
        let new = (self.display_offset as i64 + delta as i64).clamp(0, max) as usize;
        if new != self.display_offset {
            self.display_offset = new;
            self.mark_all();
            true
        } else {
            false
        }
    }
    pub fn scroll_display_to_bottom(&mut self) -> bool {
        let d = self.display_offset as i32;
        self.scroll_display(-d)
    }
    pub fn scroll_display_to_top(&mut self) -> bool {
        self.scroll_display(self.main.scrollback_len() as i32)
    }

    /// Row shown at visible row `r` honouring `display_offset`.
    pub fn visible_row(&self, r: usize) -> &Row {
        let g = self.grid();
        let off = self.display_offset();
        if r < off {
            let sb = g.scrollback_len();
            g.scrollback_row(sb.saturating_sub(off) + r)
                .unwrap_or_else(|| g.row(0))
        } else {
            g.row((r - off).min(g.rows() - 1))
        }
    }
    /// Iterate over the visible rows (top to bottom) as shown on screen.
    pub fn visible_rows(&self) -> impl Iterator<Item = &Row> + '_ {
        (0..self.rows).map(move |r| self.visible_row(r))
    }
    /// Absolute row index of visible row `r`.
    pub fn abs_row_of_visible_row(&self, r: usize) -> u64 {
        (self.grid().scrolled_total() + r as u64).saturating_sub(self.display_offset() as u64)
    }
    /// `scrolled_lines_total + cursor.row` for the active screen.
    pub fn cursor_abs_row(&self) -> u64 {
        self.grid().scrolled_total() + self.cursor.row as u64
    }
    /// Absolute index of the oldest line still held (scrollback start).
    pub fn oldest_abs_row(&self) -> u64 {
        let g = self.grid();
        g.scrolled_total() - g.scrollback_len() as u64
    }
    /// Row at an absolute index (scrollback or live grid) if still present.
    pub fn row_at_abs(&self, abs: u64) -> Option<&Row> {
        let g = self.grid();
        let first = self.oldest_abs_row();
        if abs < first {
            return None;
        }
        let idx = (abs - first) as usize;
        let sb = g.scrollback_len();
        if idx < sb {
            g.scrollback_row(idx)
        } else if idx - sb < g.rows() {
            Some(g.row(idx - sb))
        } else {
            None
        }
    }

    // ------------------------------------------------------------------
    // Line marks (OSC 133)
    // ------------------------------------------------------------------

    pub(crate) fn add_mark(&mut self, mark: LineMark) {
        let r = self.cursor.row;
        let row = self.grid_mut().row_mut(r);
        if mark.kind == MarkKind::PromptStart
            && row.marks.iter().any(|m| m.kind == MarkKind::PromptStart)
        {
            return;
        }
        row.marks.push(mark);
    }
    /// Marks on an absolute row.
    pub fn marks_at(&self, abs: u64) -> &[LineMark] {
        self.row_at_abs(abs).map(|r| r.marks.as_slice()).unwrap_or(&[])
    }
    /// All marks currently held, as `(abs_row, mark)` in ascending row order.
    pub fn line_marks(&self) -> Vec<(u64, LineMark)> {
        let g = self.grid();
        let first = self.oldest_abs_row();
        let mut out = Vec::new();
        let sb = g.scrollback_len();
        for i in 0..sb {
            for m in &g.scrollback_row(i).unwrap().marks {
                out.push((first + i as u64, *m));
            }
        }
        for (i, row) in g.lines().iter().enumerate() {
            for m in &row.marks {
                out.push((first + (sb + i) as u64, *m));
            }
        }
        out
    }
    /// Find the nearest row with a mark of `kind` strictly before/after
    /// `from_abs`.
    pub fn find_mark(&self, kind: MarkKind, from_abs: u64, dir: Direction) -> Option<u64> {
        let marks = self.line_marks();
        match dir {
            Direction::Backward => marks
                .iter()
                .rev()
                .find(|(r, m)| *r < from_abs && m.kind == kind)
                .map(|(r, _)| *r),
            Direction::Forward => marks
                .iter()
                .find(|(r, m)| *r > from_abs && m.kind == kind)
                .map(|(r, _)| *r),
        }
    }

    // ------------------------------------------------------------------
    // Damage
    // ------------------------------------------------------------------

    pub(crate) fn mark_row(&mut self, r: usize) {
        if let Some(d) = self.dirty.get_mut(r) {
            *d = true;
        }
    }
    pub(crate) fn mark_rows(&mut self, a: usize, b: usize) {
        for r in a..=b.min(self.rows.saturating_sub(1)) {
            self.dirty[r] = true;
        }
    }
    pub fn mark_all(&mut self) {
        self.full_redraw = true;
    }
    /// True if there is pending damage (ignores synchronized output).
    pub fn is_dirty(&self) -> bool {
        self.full_redraw || self.dirty.iter().any(|d| *d)
    }
    /// Rows currently marked dirty (all rows if a full redraw is pending).
    pub fn dirty_rows(&self) -> Vec<usize> {
        if self.full_redraw {
            (0..self.rows).collect()
        } else {
            self.dirty
                .iter()
                .enumerate()
                .filter(|(_, d)| **d)
                .map(|(i, _)| i)
                .collect()
        }
    }
    pub fn needs_full_redraw(&self) -> bool {
        self.full_redraw
    }
    /// Take and clear accumulated damage. While synchronized output (mode
    /// 2026) is active nothing is reported and damage keeps accumulating, so
    /// the renderer keeps showing the last complete frame.
    pub fn take_damage(&mut self) -> Damage {
        // A client that never ends synchronized output must not freeze the
        // display, even if no further bytes arrive to trigger the check in
        // `advance`.
        self.check_sync_timeout(SYNC_TIMEOUT);
        // Cursor moves done through the API (e.g. `move_cursor_rel`) since
        // the last `advance` must dirty their rows too.
        self.update_cursor_damage();
        if self.modes.synchronized_output {
            return Damage::None;
        }
        let scrolled_view = self.display_offset() > 0;
        let any = self.is_dirty();
        let full = self.full_redraw;
        let rows = self.dirty_rows();
        self.clear_damage();
        if !any {
            Damage::None
        } else if full || scrolled_view {
            Damage::Full
        } else {
            Damage::Rows(rows)
        }
    }
    pub fn clear_damage(&mut self) {
        self.full_redraw = false;
        self.dirty.fill(false);
    }
    /// How long synchronized output has been active, if it is.
    pub fn sync_elapsed(&self) -> Option<Duration> {
        self.sync_since.map(|t| t.elapsed())
    }
    /// Force-end synchronized output if it has been active longer than
    /// `timeout`. Returns true if it was ended.
    pub fn check_sync_timeout(&mut self, timeout: Duration) -> bool {
        if let Some(t) = self.sync_since {
            if self.modes.synchronized_output && t.elapsed() > timeout {
                self.set_synchronized(false);
                return true;
            }
        }
        false
    }
    #[cfg(test)]
    pub(crate) fn backdate_sync_for_test(&mut self, by: Duration) {
        self.sync_since = Some(Instant::now().checked_sub(by).unwrap());
    }

    pub(crate) fn set_synchronized(&mut self, on: bool) {
        if on {
            if !self.modes.synchronized_output {
                self.sync_since = Some(Instant::now());
            }
        } else {
            self.sync_since = None;
            if self.modes.synchronized_output {
                self.mark_all();
            }
        }
        self.modes.synchronized_output = on;
    }

    fn update_cursor_damage(&mut self) {
        let now = (
            self.cursor,
            self.modes.cursor_visible,
            self.cursor_shape,
            self.modes.cursor_blink,
        );
        if now != self.drawn_cursor {
            self.mark_row(self.drawn_cursor.0.row);
            self.mark_row(self.cursor.row);
            self.drawn_cursor = now;
        }
    }

    // ------------------------------------------------------------------
    // Text snapshot helpers
    // ------------------------------------------------------------------

    /// Visible grid as plain text, one line per row, trailing blanks trimmed.
    pub fn render_snapshot(&self) -> String {
        let lines: Vec<String> = self.visible_rows().map(|r| r.text()).collect();
        lines.join("\n")
    }
    /// Visible rows as strings.
    pub fn snapshot_lines(&self) -> Vec<String> {
        self.visible_rows().map(|r| r.text()).collect()
    }

    // ------------------------------------------------------------------
    // Resize
    // ------------------------------------------------------------------

    /// Resize the terminal without reflow: columns truncate/extend; rows
    /// truncate (pushing top lines into scrollback if the cursor would fall
    /// off) or extend (pulling lines back from scrollback if any).
    pub fn resize<C: TryInto<usize>, R: TryInto<usize>>(&mut self, cols: C, rows: R) {
        let cols = to_usize(cols).max(1);
        let rows = to_usize(rows).max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }
        let main_cursor_row = if self.active == Screen::Main {
            self.cursor.row
        } else {
            self.saved[0].map(|s| s.row).unwrap_or(0)
        };
        let shift_main = self.main.resize(cols, rows, main_cursor_row);
        let alt_cursor_row = if self.active == Screen::Alt {
            self.cursor.row
        } else {
            0
        };
        let shift_alt = self.alt.resize(cols, rows, alt_cursor_row);
        match self.active {
            Screen::Main => {
                self.cursor.row = (self.cursor.row as isize + shift_main).max(0) as usize;
            }
            Screen::Alt => {
                self.cursor.row = (self.cursor.row as isize + shift_alt).max(0) as usize;
                if let Some(s) = &mut self.saved[0] {
                    s.row = (s.row as isize + shift_main).max(0) as usize;
                }
            }
        }
        self.cols = cols;
        self.rows = rows;
        self.cursor.col = self.cursor.col.min(cols - 1);
        self.cursor.row = self.cursor.row.min(rows - 1);
        for s in self.saved.iter_mut().flatten() {
            s.col = s.col.min(cols - 1);
            s.row = s.row.min(rows - 1);
        }
        self.region_top = 0;
        self.region_bottom = rows - 1;
        self.pending_wrap = false;
        self.last_cell = None;
        let old_tabs = std::mem::take(&mut self.tabs);
        self.tabs = (0..cols)
            .map(|i| old_tabs.get(i).copied().unwrap_or(i % 8 == 0))
            .collect();
        self.dirty = vec![true; rows];
        self.full_redraw = true;
        if self.display_offset > self.main.scrollback_len() {
            self.display_offset = self.main.scrollback_len();
        }
    }

    // ------------------------------------------------------------------
    // Cursor movement
    // ------------------------------------------------------------------

    /// Move the cursor relative to its position, wrapping at line ends and
    /// scrolling at the bottom as printing would (used by the graphics glue).
    pub fn move_cursor_rel(&mut self, dcols: i32, drows: i32) {
        self.pending_wrap = false;
        self.last_cell = None;
        if drows > 0 {
            // Scrolling more than a screenful per call is pointless.
            for _ in 0..(drows as usize).min(self.rows * 2 + 2) {
                self.linefeed();
            }
        } else if drows < 0 {
            self.cursor.row = self.cursor.row.saturating_sub(drows.unsigned_abs() as usize);
        }
        if dcols > 0 {
            let mut col = self.cursor.col + dcols as usize;
            let wraps = col / self.cols;
            col %= self.cols;
            // More than a few screenfuls of wraps is indistinguishable from
            // fewer, and a huge count must not spin for billions of iterations.
            for _ in 0..wraps.min(self.rows * 2 + 2) {
                let r = self.cursor.row;
                self.grid_mut().row_mut(r).wrapped = true;
                self.linefeed();
            }
            self.cursor.col = col;
        } else if dcols < 0 {
            self.cursor.col = self.cursor.col.saturating_sub(dcols.unsigned_abs() as usize);
        }
        self.update_cursor_damage();
    }

    pub(crate) fn clamp_cursor(&mut self) {
        self.cursor.col = self.cursor.col.min(self.cols - 1);
        self.cursor.row = self.cursor.row.min(self.rows - 1);
    }

    pub(crate) fn set_cursor_pos(&mut self, row: usize, col: usize) {
        // row/col 0-based, row relative to origin when DECOM is set.
        let (top, bottom) = if self.modes.origin {
            (self.region_top, self.region_bottom)
        } else {
            (0, self.rows - 1)
        };
        self.cursor.row = (top + row).min(bottom);
        self.cursor.col = col.min(self.cols - 1);
        self.pending_wrap = false;
    }

    pub(crate) fn linefeed(&mut self) {
        self.pending_wrap = false;
        if self.cursor.row == self.region_bottom {
            self.scroll_up(self.region_top, self.region_bottom, 1);
        } else if self.cursor.row + 1 < self.rows {
            self.cursor.row += 1;
        }
    }

    pub(crate) fn reverse_index(&mut self) {
        self.pending_wrap = false;
        if self.cursor.row == self.region_top {
            self.scroll_down(self.region_top, self.region_bottom, 1);
        } else if self.cursor.row > 0 {
            self.cursor.row -= 1;
        }
    }

    pub(crate) fn scroll_up(&mut self, top: usize, bottom: usize, n: usize) {
        let bg = self.pen.bg;
        let pushed = if top == 0 && self.grid().scrollback_limit() > 0 {
            n.min(bottom - top + 1)
        } else {
            0
        };
        self.grid_mut().scroll_up(top, bottom, n, bg);
        // Keep a scrolled-back view anchored on the same content: every line
        // pushed into scrollback moves it one further from the live view,
        // even when a full ring evicts an old line at the same time.
        if self.active == Screen::Main && self.display_offset > 0 && pushed > 0 {
            self.display_offset = (self.display_offset + pushed).min(self.main.scrollback_len());
        }
        self.mark_rows(top, bottom);
    }

    pub(crate) fn scroll_down(&mut self, top: usize, bottom: usize, n: usize) {
        let bg = self.pen.bg;
        self.grid_mut().scroll_down(top, bottom, n, bg);
        self.mark_rows(top, bottom);
    }

    pub(crate) fn next_tab(&self, col: usize) -> usize {
        (col + 1..self.cols)
            .find(|&c| self.tabs[c])
            .unwrap_or(self.cols - 1)
    }
    pub(crate) fn prev_tab(&self, col: usize) -> usize {
        (0..col).rev().find(|&c| self.tabs[c]).unwrap_or(0)
    }
    pub(crate) fn reset_tabs(&mut self) {
        self.tabs = (0..self.cols).map(|i| i % 8 == 0).collect();
    }

    // ------------------------------------------------------------------
    // Printing
    // ------------------------------------------------------------------

    fn map_charset(&self, c: char) -> char {
        if self.charsets[self.gl] == Charset::DecGraphics {
            dec_graphics(c)
        } else {
            c
        }
    }

    pub(crate) fn print_char(&mut self, c: char) {
        let c = self.map_charset(c);
        let w = if c.is_ascii() {
            if c < ' ' || c == '\x7f' {
                return;
            }
            1
        } else {
            if self.try_join(c) {
                return;
            }
            c.width().unwrap_or(0)
        };
        if w == 0 {
            return;
        }
        self.put(c, w);
        self.last_char = Some(c);
    }

    /// Try to attach `c` to the previously printed cell as part of the same
    /// grapheme cluster. Returns true if consumed.
    fn try_join(&mut self, c: char) -> bool {
        if (c as u32) < 0x300 {
            return false;
        }
        let Some((r, col)) = self.last_cell else {
            return false;
        };
        if r >= self.rows || col >= self.cols {
            return false;
        }
        let cell = self.grid().cell(r, col);
        if cell.is_spacer() {
            return false;
        }
        let mut s = String::new();
        cell.push_text(&mut s);
        s.push(c);
        if s.graphemes(true).count() != 1 {
            return false;
        }
        let was_wide = cell.is_wide();
        let cluster_w = s.width();
        let g = self.grid_mut();
        g.cell_mut(r, col).combining.push(c);
        self.mark_row(r);
        // Emoji presentation (VS16) etc. can widen a narrow cell.
        if !was_wide && cluster_w >= 2 && col + 1 < self.cols {
            let cols = self.cols;
            let row = self.grid_mut().row_mut(r);
            if row.cells[col + 1].is_wide() && col + 2 < cols {
                row.cells[col + 2] = Cell::erased(row.cells[col + 1].bg);
            }
            let cs = row.cells[col].style();
            row.cells[col].width = CellWidth::Wide;
            row.cells[col + 1] = Cell::with_style(' ', &cs, CellWidth::WideSpacer);
            if self.cursor.row == r && self.cursor.col == col + 1 && !self.pending_wrap {
                if col + 2 >= cols {
                    self.pending_wrap = self.modes.wrap;
                } else {
                    self.cursor.col = col + 2;
                }
            }
        }
        true
    }

    fn put(&mut self, c: char, w: usize) {
        let cols = self.cols;
        if self.pending_wrap {
            self.pending_wrap = false;
            self.wrap_line();
        }
        if w == 2 && self.cursor.col + 1 >= cols {
            if cols < 2 {
                return;
            }
            if self.modes.wrap {
                let (r, col) = (self.cursor.row, self.cursor.col);
                let bg = self.pen.bg;
                let g = self.grid_mut();
                clear_overlap(g.row_mut(r), col, bg);
                g.row_mut(r).cells[col] = Cell::erased(bg);
                self.wrap_line();
            } else {
                return;
            }
        }
        if self.modes.insert {
            self.insert_chars(w);
        }
        let (r, col) = (self.cursor.row, self.cursor.col);
        let style = self.pen;
        {
            let g = self.grid_mut();
            let row = g.row_mut(r);
            clear_overlap(row, col, style.bg);
            if w == 2 {
                clear_overlap(row, col + 1, style.bg);
            }
            row.cells[col] = Cell::with_style(
                c,
                &style,
                if w == 2 {
                    CellWidth::Wide
                } else {
                    CellWidth::Single
                },
            );
            if w == 2 {
                row.cells[col + 1] = Cell::with_style(' ', &style, CellWidth::WideSpacer);
            }
        }
        self.last_cell = Some((r, col));
        self.mark_row(r);
        let nc = col + w;
        if nc >= cols {
            self.cursor.col = cols - 1;
            self.pending_wrap = self.modes.wrap;
        } else {
            self.cursor.col = nc;
        }
    }

    fn wrap_line(&mut self) {
        let r = self.cursor.row;
        self.grid_mut().row_mut(r).wrapped = true;
        self.cursor.col = 0;
        self.linefeed();
    }

    // ------------------------------------------------------------------
    // Editing primitives
    // ------------------------------------------------------------------

    pub(crate) fn insert_chars(&mut self, n: usize) {
        let (r, col) = (self.cursor.row, self.cursor.col);
        let bg = self.pen.bg;
        let cols = self.cols;
        let n = n.min(cols - col);
        let row = self.grid_mut().row_mut(r);
        row.cells[col..].rotate_right(n);
        for c in &mut row.cells[col..col + n] {
            *c = Cell::erased(bg);
        }
        row.fix_wide();
        self.mark_row(r);
    }

    pub(crate) fn delete_chars(&mut self, n: usize) {
        let (r, col) = (self.cursor.row, self.cursor.col);
        let bg = self.pen.bg;
        let cols = self.cols;
        let n = n.min(cols - col);
        let row = self.grid_mut().row_mut(r);
        row.cells[col..].rotate_left(n);
        for c in &mut row.cells[cols - n..] {
            *c = Cell::erased(bg);
        }
        row.fix_wide();
        self.mark_row(r);
    }

    /// Erase cells `[from, to)` on row `r`.
    pub(crate) fn erase_cells(&mut self, r: usize, from: usize, to: usize) {
        let bg = self.pen.bg;
        let to = to.min(self.cols);
        if from >= to {
            return;
        }
        let row = self.grid_mut().row_mut(r);
        for c in &mut row.cells[from..to] {
            *c = Cell::erased(bg);
        }
        if to == row.cells.len() {
            row.wrapped = false;
        }
        row.fix_wide();
        self.mark_row(r);
    }

    pub(crate) fn erase_rows(&mut self, from: usize, to: usize) {
        // rows [from, to)
        let bg = self.pen.bg;
        for r in from..to.min(self.rows) {
            self.grid_mut().row_mut(r).clear(bg);
            self.mark_row(r);
        }
    }

    // ------------------------------------------------------------------
    // Screens, save/restore
    // ------------------------------------------------------------------

    pub(crate) fn save_cursor(&mut self) {
        let i = self.sidx();
        self.saved[i] = Some(SavedCursor {
            col: self.cursor.col,
            row: self.cursor.row,
            pen: self.pen,
            origin: self.modes.origin,
            pending_wrap: self.pending_wrap,
            charsets: self.charsets,
            gl: self.gl,
        });
    }

    pub(crate) fn restore_cursor(&mut self) {
        let i = self.sidx();
        if let Some(s) = self.saved[i] {
            self.cursor.col = s.col.min(self.cols - 1);
            self.cursor.row = s.row.min(self.rows - 1);
            self.pen = s.pen;
            self.modes.origin = s.origin;
            self.pending_wrap = s.pending_wrap;
            self.charsets = s.charsets;
            self.gl = s.gl;
        } else {
            self.cursor = Cursor::default();
            self.pen = Style::default();
            self.modes.origin = false;
            self.pending_wrap = false;
        }
    }

    pub(crate) fn enter_alt(&mut self, save_cursor: bool) {
        if self.active == Screen::Alt {
            return;
        }
        if save_cursor {
            self.save_cursor();
        }
        self.active = Screen::Alt;
        self.alt.reset_scrolled_total();
        self.alt.clear(Color::Default);
        self.region_top = 0;
        self.region_bottom = self.rows - 1;
        self.alt_enter_count += 1;
        self.pending_wrap = false;
        self.last_cell = None;
        self.mark_all();
    }

    pub(crate) fn leave_alt(&mut self, restore_cursor: bool) {
        if self.active == Screen::Main {
            return;
        }
        self.active = Screen::Main;
        self.region_top = 0;
        self.region_bottom = self.rows - 1;
        self.pending_wrap = false;
        self.last_cell = None;
        if restore_cursor {
            self.restore_cursor();
        }
        self.clamp_cursor();
        self.mark_all();
    }

    pub(crate) fn intern_hyperlink(&mut self, id: Option<String>, uri: String) -> Option<u32> {
        if let Some(id) = &id {
            if let Some(&n) = self.hyperlink_map.get(&(id.clone(), uri.clone())) {
                return Some(n);
            }
        }
        if self.hyperlinks.len() >= MAX_HYPERLINKS {
            self.gc_hyperlinks();
            if self.hyperlinks.len() >= MAX_HYPERLINKS {
                return None;
            }
            // Ids were renumbered; re-resolve the pair.
            if let Some(id) = &id {
                if let Some(&n) = self.hyperlink_map.get(&(id.clone(), uri.clone())) {
                    return Some(n);
                }
            }
        }
        let n = self.hyperlinks.len() as u32;
        if let Some(id) = &id {
            self.hyperlink_map.insert((id.clone(), uri.clone()), n);
        }
        self.hyperlinks.push(Hyperlink { id, uri });
        Some(n)
    }

    /// Drop hyperlinks no cell (or saved pen) references any more and
    /// renumber the rest. Called only when the table is full.
    fn gc_hyperlinks(&mut self) {
        let n = self.hyperlinks.len();
        let mut used = vec![false; n];
        let mut mark = |id: Option<u32>| {
            if let Some(i) = id {
                if let Some(u) = used.get_mut(i as usize) {
                    *u = true;
                }
            }
        };
        mark(self.pen.hyperlink_id);
        for s in self.saved.iter().flatten() {
            mark(s.pen.hyperlink_id);
        }
        for g in [&mut self.main, &mut self.alt] {
            g.for_each_row_mut(|row| {
                for c in &row.cells {
                    mark(c.hyperlink_id);
                }
            });
        }
        let mut remap: Vec<Option<u32>> = vec![None; n];
        let mut kept = Vec::new();
        for (i, h) in std::mem::take(&mut self.hyperlinks).into_iter().enumerate() {
            if used[i] {
                remap[i] = Some(kept.len() as u32);
                kept.push(h);
            }
        }
        self.hyperlinks = kept;
        let map = |id: Option<u32>| id.and_then(|i| remap.get(i as usize).copied().flatten());
        self.pen.hyperlink_id = map(self.pen.hyperlink_id);
        for s in self.saved.iter_mut().flatten() {
            s.pen.hyperlink_id = map(s.pen.hyperlink_id);
        }
        for g in [&mut self.main, &mut self.alt] {
            g.for_each_row_mut(|row| {
                for c in &mut row.cells {
                    c.hyperlink_id = map(c.hyperlink_id);
                }
            });
        }
        self.hyperlink_map = self
            .hyperlinks
            .iter()
            .enumerate()
            .filter_map(|(i, h)| h.id.clone().map(|id| ((id, h.uri.clone()), i as u32)))
            .collect();
        self.mark_all();
    }

    /// Full reset (RIS). Keeps size and scrollback.
    pub(crate) fn full_reset(&mut self) {
        self.display_clear_count += 1;
        if self.active == Screen::Alt {
            self.leave_alt(false);
        }
        self.pen = Style::default();
        self.modes = Modes::default();
        self.charsets = [Charset::Ascii; 4];
        self.gl = 0;
        self.saved = [None, None];
        self.kb = [KbState::default(), KbState::default()];
        self.region_top = 0;
        self.region_bottom = self.rows - 1;
        self.reset_tabs();
        self.cursor = Cursor::default();
        self.pending_wrap = false;
        self.cursor_shape = CursorShape::Block;
        self.set_synchronized(false);
        self.main.clear(Color::Default);
        self.display_offset = 0;
        self.last_cell = None;
        for i in 0..256 {
            self.reset_palette_entry(i);
        }
        self.colors.foreground = self.colors.base_foreground;
        self.colors.background = self.colors.base_background;
        self.colors.cursor = self.colors.base_cursor;
        self.mark_all();
        self.events.push(TermEvent::ColorChanged);
        self.events.push(TermEvent::SetMouseMode);
    }

    /// Soft reset (DECSTR).
    pub(crate) fn soft_reset(&mut self) {
        self.pen = Style::default();
        self.modes.origin = false;
        self.modes.wrap = true;
        self.modes.insert = false;
        self.modes.cursor_visible = true;
        self.modes.app_cursor = false;
        self.modes.app_keypad = false;
        self.charsets = [Charset::Ascii; 4];
        self.gl = 0;
        self.region_top = 0;
        self.region_bottom = self.rows - 1;
        self.saved[self.sidx()] = None;
        self.pending_wrap = false;
    }
}

fn clear_overlap(row: &mut Row, c: usize, bg: Color) {
    match row.cells[c].width {
        CellWidth::Wide => {
            if c + 1 < row.cells.len() && row.cells[c + 1].is_spacer() {
                row.cells[c + 1] = Cell::erased(bg);
            }
        }
        CellWidth::WideSpacer => {
            if c > 0 && row.cells[c - 1].is_wide() {
                row.cells[c - 1] = Cell::erased(bg);
            }
        }
        CellWidth::Single => {}
    }
}

fn dec_graphics(c: char) -> char {
    match c {
        '`' => '◆',
        'a' => '▒',
        'b' => '␉',
        'c' => '␌',
        'd' => '␍',
        'e' => '␊',
        'f' => '°',
        'g' => '±',
        'h' => '␤',
        'i' => '␋',
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'o' => '⎺',
        'p' => '⎻',
        'q' => '─',
        'r' => '⎼',
        's' => '⎽',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        'y' => '≤',
        'z' => '≥',
        '{' => 'π',
        '|' => '≠',
        '}' => '£',
        '~' => '·',
        _ => c,
    }
}

impl fmt::Display for Terminal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render_snapshot())
    }
}

impl fmt::Debug for Terminal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Terminal")
            .field("size", &(self.cols, self.rows))
            .field("cursor", &self.cursor)
            .field("screen", &self.active)
            .finish()
    }
}
