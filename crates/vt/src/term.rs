//! The `Terminal` state machine: state, public API, printing and cursor ops.
//! Escape-sequence dispatch lives in `perform.rs`.

use crate::cell::*;
use crate::grid::Grid;
use crate::reflow::Track;
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
    /// A program chose the blink (DECSCUSR 1-6 or mode 12) and it has not
    /// been put back to "default" (DECSCUSR 0 / RIS); until then the host's
    /// configured blink applies.
    pub cursor_blink_set: bool,
    /// Likewise for the cursor shape (DECSCUSR 1-6).
    pub cursor_shape_set: bool,
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
            cursor_blink_set: false,
            cursor_shape_set: false,
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
    /// The primary screen was rewrapped by a column-count change. Absolute
    /// row numbers handed out before this event no longer mean the same
    /// lines: hosts must drop anything anchored to them (selections, image
    /// placements). `cursor_abs_row` stays monotonic across a reflow.
    Reflowed,
}

const EVENTS_COMPACT_MIN: usize = 1024;

/// Most OSC 52 queries kept per batch; each is answered, but a flood of them
/// is not worth answering in full.
const MAX_QUERIES_PER_BATCH: usize = 16;

/// Collapse a batch of events to what the host needs: only the last title,
/// clipboard write and working directory, and one each of the idempotent
/// notifications (bell, colour / cursor-shape / mouse-mode change), keeping
/// the order of the survivors. Graphics commands are never dropped and
/// clipboard queries are capped at [`MAX_QUERIES_PER_BATCH`] (the latest).
pub fn coalesce_events(events: &mut Vec<TermEvent>) {
    if events.len() < 2 {
        return;
    }
    let mut seen = [false; 8];
    let mut queries = 0usize;
    let mut keep = vec![true; events.len()];
    for (i, ev) in events.iter().enumerate().rev() {
        let kind = match ev {
            TermEvent::Title(_) => 0,
            TermEvent::Bell => 1,
            TermEvent::ClipboardSet(_) => 2,
            TermEvent::CwdChanged(_) => 3,
            TermEvent::ColorChanged => 4,
            TermEvent::CursorShape(_) => 5,
            TermEvent::SetMouseMode => 6,
            TermEvent::Reflowed => 7,
            TermEvent::ClipboardQuery => {
                queries += 1;
                keep[i] = queries <= MAX_QUERIES_PER_BATCH;
                continue;
            }
            TermEvent::GraphicsCommand(_) => continue,
        };
        if seen[kind] {
            keep[i] = false;
        }
        seen[kind] = true;
    }
    let mut it = keep.into_iter();
    events.retain(|_| it.next().unwrap_or(true));
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
/// Hard cap on the accumulated body of any OSC/DCS/APC/SOS/PM string. Longer
/// strings are discarded whole. (Graphics chunks are <= 4 KiB by protocol and
/// the largest legitimate OSC is a clipboard write, capped below this.)
/// Largest OSC 52 clipboard write (decoded bytes) accepted.
pub const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;
/// Base64 length corresponding to [`MAX_CLIPBOARD_BYTES`].
pub(crate) const MAX_CLIPBOARD_B64: usize = MAX_CLIPBOARD_BYTES.div_ceil(3) * 4;
/// Longest OSC 52 selection string (`c`, `p`, `s`, `0`-`7`, ...) kept and
/// echoed back in a reply.
pub(crate) const MAX_CLIP_SELECTION: usize = 8;
/// Most bytes of terminal replies (DA, DSR, XTVERSION, graphics, ...) held
/// between [`Terminal::drain_responses`] calls; further replies are dropped.
/// Bounds the amplification of hostile queries (an XTVERSION query is 4 bytes
/// and its answer 24).
pub const MAX_PENDING_RESPONSE_BYTES: usize = 64 * 1024;
/// Maximum combining marks stored per cell; extras are dropped (kitty: 8).
pub const MAX_COMBINING: usize = 8;
pub const MAX_STRING_BODY: usize = 2 * 1024 * 1024;
/// After a string longer than this, buffers are reallocated to free memory.
const STRING_SHRINK_THRESHOLD: usize = 64 * 1024;
pub(crate) const MAX_HYPERLINKS: usize = 1 << 17;
/// Longest OSC 8 URI accepted (bytes). Longer links are ignored.
pub(crate) const MAX_HYPERLINK_URI: usize = 8 * 1024;
/// Longest OSC 8 `id=` accepted (bytes).
pub(crate) const MAX_HYPERLINK_ID: usize = 256;
/// Cap on the total bytes (ids + URIs) held by the hyperlink table.
pub(crate) const MAX_HYPERLINK_BYTES: usize = 8 * 1024 * 1024;
/// A hyperlink-table GC may run once this many input bytes have arrived per
/// 8 cells the previous GC had to visit (so GC cost is amortised against
/// input, however hostile).
const GC_CELLS_PER_INPUT_BYTE: u64 = 8;
/// Work credit (in cells) earned per input byte; see [`Terminal::try_spend`].
const WORK_PER_BYTE: u64 = 8;
/// Floor of the work-credit bucket (in cells).
const WORK_CAP_MIN: u64 = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApcState {
    Ground,
    Esc,
    Apc,
    ApcEsc,
    /// Inside an OSC/DCS/SOS/PM string (vte parses it; we only bound it).
    Str,
    StrEsc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StrKind {
    Osc,
    Apc,
    /// DCS: vte also ends a hooked DCS on the raw C1 byte 0x9c.
    Dcs,
    /// SOS / PM.
    Other,
}

/// The terminal emulator state machine.
pub struct Terminal {
    parser: Option<vte::Parser>,
    apc: ApcState,
    apc_buf: Vec<u8>,
    str_kind: StrKind,
    str_len: usize,
    str_overflow: bool,
    /// Incomplete UTF-8 sequence at the end of the previous `advance` call,
    /// held back so vte never has to resume a split character (it mishandles
    /// the bytes that follow one).
    utf8_carry: Vec<u8>,
    /// Set by `Perform::unhook`; lets the pre-scanner see vte end a DCS.
    pub(crate) dcs_unhooked: bool,
    /// Leaky bucket (cells) bounding whole-screen work per input byte.
    work_credit: u64,
    /// Input bytes since the last hyperlink-table GC, and that GC's cost.
    gc_idle_bytes: u64,
    gc_last_cost: u64,
    pub(crate) gc_runs: u64,
    /// Total bytes (ids + URIs) held in `hyperlinks`.
    pub(crate) link_bytes: usize,
    /// The newest hyperlink has not been given to any cell yet.
    pub(crate) link_unused: bool,
    reset_count: u64,

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
    /// A `ClipboardQuery` event is queued and not yet drained.
    pub(crate) clip_query_pending: bool,
    events_compact_at: usize,
    dirty: Vec<bool>,
    full_redraw: bool,
    pub(crate) last_cell: Option<(usize, usize)>,
    pub(crate) last_char: Option<char>,
    pub(crate) cell_px: (u32, u32),
    alt_enter_count: u64,
    /// Per screen (`[main, alt]`): ED 2 / RIS count and the absolute row of
    /// the screen's first row when the last one ran.
    display_clears: [(u64, u64); 2],
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
            str_kind: StrKind::Other,
            str_len: 0,
            str_overflow: false,
            utf8_carry: Vec::new(),
            dcs_unhooked: false,
            work_credit: Self::work_cap_for(cols, rows),
            gc_idle_bytes: 0,
            gc_last_cost: 0,
            gc_runs: 0,
            link_bytes: 0,
            link_unused: false,
            reset_count: 0,
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
            clip_query_pending: false,
            events: Vec::new(),
            events_compact_at: EVENTS_COMPACT_MIN,
            dirty: vec![true; rows],
            full_redraw: true,
            last_cell: None,
            last_char: None,
            cell_px: (8, 16),
            alt_enter_count: 0,
            display_clears: [(0, 0); 2],
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
    ///
    /// A thin pre-scanner sits in front of `vte`: it tracks string sequences
    /// (OSC, DCS, SOS, PM and APC), bounds how many body bytes any of them may
    /// accumulate ([`MAX_STRING_BODY`]) and, for APC, collects the payload
    /// itself (vte has no APC callback). A string that exceeds the bound is
    /// discarded: vte's parser (and its internal buffers) is replaced and the
    /// remaining bytes are swallowed up to the terminator, never printed.
    pub fn advance(&mut self, bytes: &[u8]) {
        self.check_sync_timeout(SYNC_TIMEOUT);
        self.work_credit = self
            .work_credit
            .saturating_add((bytes.len() as u64).saturating_mul(WORK_PER_BYTE))
            .min(self.work_cap());
        self.gc_idle_bytes = self.gc_idle_bytes.saturating_add(bytes.len() as u64);
        let mut rest = bytes;
        if !self.utf8_carry.is_empty() {
            rest = self.complete_utf8_carry(rest);
        }
        let keep = incomplete_utf8_tail(rest);
        let (body, tail) = rest.split_at(rest.len() - keep);
        if !body.is_empty() {
            self.advance_scan(body);
        }
        self.utf8_carry.extend_from_slice(tail);
        self.update_cursor_damage();
    }

    /// Finish the UTF-8 character held back by the previous call using the
    /// head of `bytes`; returns the unconsumed rest. A sequence that turns out
    /// to be invalid is replaced by U+FFFD.
    fn complete_utf8_carry<'a>(&mut self, bytes: &'a [u8]) -> &'a [u8] {
        let mut used = 0;
        loop {
            match std::str::from_utf8(&self.utf8_carry) {
                Ok(_) => break,
                Err(e) if e.error_len().is_none() => {
                    // Still a valid prefix: need more input.
                    if used == bytes.len() {
                        return &bytes[used..];
                    }
                    self.utf8_carry.push(bytes[used]);
                    used += 1;
                }
                Err(_) => {
                    // The byte just added broke the sequence; leave it for
                    // normal processing and substitute the truncated char.
                    self.utf8_carry.clear();
                    self.utf8_carry.extend_from_slice("\u{FFFD}".as_bytes());
                    used -= 1;
                    break;
                }
            }
        }
        let carry = std::mem::take(&mut self.utf8_carry);
        self.advance_scan(&carry);
        &bytes[used..]
    }

    /// Work-credit bucket size (cells): a few screens, at least ~1M cells.
    fn work_cap(&self) -> u64 {
        Self::work_cap_for(self.cols, self.rows)
    }
    fn work_cap_for(cols: usize, rows: usize) -> u64 {
        (cols as u64)
            .saturating_mul(rows as u64)
            .saturating_mul(4)
            .max(WORK_CAP_MIN)
    }
    /// Charge `cost` cells of whole-screen work (REP, SU/SD/IL/DL, ED, DECALN,
    /// alt-screen entry). Returns false, charging nothing, when the credit
    /// earned from recent input does not cover it, so a few input bytes can
    /// never request unbounded work.
    pub(crate) fn try_spend(&mut self, cost: u64) -> bool {
        if cost <= self.work_credit {
            self.work_credit -= cost;
            true
        } else {
            false
        }
    }
    /// Charge up to `rows` rows (`cols` cells each); returns how many the
    /// credit covers.
    pub(crate) fn spend_rows(&mut self, rows: usize) -> usize {
        let per = self.cols.max(1) as u64;
        let granted = rows.min((self.work_credit / per).min(usize::MAX as u64) as usize);
        self.work_credit -= granted as u64 * per;
        granted
    }
    /// Charge up to `n` single-cell operations; returns how many are allowed.
    pub(crate) fn spend_cells(&mut self, n: usize) -> usize {
        let granted = (n as u64).min(self.work_credit);
        self.work_credit -= granted;
        granted as usize
    }

    fn advance_scan(&mut self, bytes: &[u8]) {
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
                        self.str_kind = StrKind::Apc;
                        self.str_len = 0;
                        self.str_overflow = false;
                    }
                    b']' | b'P' | b'X' | b'^' => {
                        // Hand the introducer to vte now; only the body is
                        // held back (so an aborted string can be dropped
                        // without losing the text before it).
                        parser.advance(self, &bytes[seg..=i]);
                        i += 1;
                        seg = i;
                        self.str_kind = match b {
                            b']' => StrKind::Osc,
                            b'P' => StrKind::Dcs,
                            _ => StrKind::Other,
                        };
                        self.str_len = 0;
                        self.str_overflow = false;
                        self.apc = ApcState::Str;
                    }
                    // vte's Escape state executes C0 controls and ignores DEL
                    // and 8-bit bytes without leaving it, so `ESC <ctl> ]`
                    // still starts an OSC.
                    0x1b | 0x00..=0x17 | 0x19 | 0x1c..=0x1f | 0x7f | 0x80..=0xff => i += 1,
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
                            self.end_string_buffers();
                            self.apc = ApcState::Ground;
                        }
                        _ => {
                            if self.apc_buf.len() < MAX_STRING_BODY {
                                self.apc_buf.push(b);
                            } else {
                                self.str_overflow = true;
                            }
                        }
                    }
                    i += 1;
                    seg = i;
                }
                ApcState::ApcEsc => {
                    if b == b'\\' {
                        let buf = std::mem::take(&mut self.apc_buf);
                        if !self.str_overflow && buf.first() == Some(&b'G') {
                            self.push_event(TermEvent::GraphicsCommand(buf[1..].to_vec()));
                        }
                        self.end_string_buffers();
                        // Let vte leave its APC state.
                        parser.advance(self, &[0x1b, b'\\']);
                        self.apc = ApcState::Ground;
                        i += 1;
                        seg = i;
                    } else {
                        // ESC aborted the APC and starts a new sequence.
                        self.end_string_buffers();
                        parser.advance(self, &[0x1b]);
                        self.apc = ApcState::Esc;
                        seg = i; // reprocess `b` in Esc state
                    }
                }
                ApcState::Str => {
                    // Inside an OSC/DCS/SOS/PM string. Bytes reach vte
                    // untouched (via `seg`) until the bound is exceeded.
                    if matches!(b, 0x18 | 0x1a) && self.str_kind == StrKind::Osc {
                        // CAN / SUB cancel an OSC: it must not be dispatched
                        // (vte would), so drop it and its parser state.
                        parser = vte::Parser::default();
                        seg = i + 1;
                        self.apc = ApcState::Ground;
                        self.str_overflow = false;
                        self.str_len = 0;
                        i += 1;
                        continue;
                    }
                    if b == 0x9c && self.str_kind == StrKind::Dcs {
                        // vte ends a hooked DCS on a raw 0x9c (C1 ST) but
                        // ignores it in the other DCS states.
                        let ended = if self.str_overflow {
                            seg = i + 1;
                            true
                        } else {
                            self.dcs_unhooked = false;
                            parser.advance(self, &bytes[seg..=i]);
                            seg = i + 1;
                            self.dcs_unhooked
                        };
                        if ended {
                            if !self.str_overflow {
                                self.finish_string(&mut parser);
                            }
                            self.apc = ApcState::Ground;
                            self.str_overflow = false;
                            self.str_len = 0;
                            i += 1;
                            continue;
                        }
                    }
                    let terminates = match b {
                        0x1b => {
                            self.apc = ApcState::StrEsc;
                            false
                        }
                        0x18 | 0x1a => true,
                        0x07 => self.str_kind == StrKind::Osc,
                        _ => false,
                    };
                    if terminates {
                        if self.str_overflow {
                            seg = i + 1; // parser already reset; swallow
                        } else {
                            parser.advance(self, &bytes[seg..=i]);
                            seg = i + 1;
                            self.finish_string(&mut parser);
                        }
                        self.apc = ApcState::Ground;
                        self.str_overflow = false;
                        self.str_len = 0;
                    } else if self.apc == ApcState::Str {
                        self.str_len = self.str_len.saturating_add(1);
                        if self.str_overflow {
                            seg = i + 1;
                        } else if self.str_len > MAX_STRING_BODY {
                            // Too long: drop everything vte has accumulated
                            // and swallow the rest of the string.
                            self.str_overflow = true;
                            parser = vte::Parser::default();
                            seg = i + 1;
                        }
                    } else if self.str_overflow {
                        seg = i + 1; // the ESC of a possible ST
                    }
                    i += 1;
                }
                ApcState::StrEsc => {
                    if b == b'\\' {
                        if self.str_overflow {
                            seg = i + 1;
                        } else {
                            parser.advance(self, &bytes[seg..=i]);
                            seg = i + 1;
                            self.finish_string(&mut parser);
                        }
                        self.apc = ApcState::Ground;
                        self.str_overflow = false;
                        self.str_len = 0;
                        i += 1;
                    } else if b == 0x1b {
                        if self.str_overflow {
                            seg = i + 1;
                        }
                        i += 1;
                    } else {
                        // The ESC aborted the string and starts a sequence.
                        if self.str_overflow {
                            self.str_overflow = false;
                            parser.advance(self, &[0x1b]);
                            seg = i;
                        }
                        self.apc = ApcState::Esc;
                        // reprocess `b` in Esc state
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

    /// A bounded string just ended normally; give memory back if it was big.
    fn finish_string(&mut self, parser: &mut vte::Parser) {
        if self.str_len > STRING_SHRINK_THRESHOLD {
            *parser = vte::Parser::default();
        }
        self.str_len = 0;
    }

    /// An APC string ended (any way); release oversized buffers.
    fn end_string_buffers(&mut self) {
        self.apc_buf.clear();
        if self.apc_buf.capacity() > STRING_SHRINK_THRESHOLD {
            self.apc_buf = Vec::new();
        }
        self.str_overflow = false;
    }

    /// Body bytes held for the string sequence currently being parsed
    /// (never more than [`MAX_STRING_BODY`]). Test/diagnostic hook.
    #[cfg(test)]
    pub(crate) fn string_bytes_buffered(&self) -> usize {
        match self.apc {
            ApcState::Apc | ApcState::ApcEsc => self.apc_buf.len(),
            _ if self.str_overflow => 0,
            _ => self.str_len.min(MAX_STRING_BODY),
        }
    }

    #[cfg(test)]
    pub(crate) fn string_buffer_capacity(&self) -> usize {
        self.apc_buf.capacity()
    }

    /// Drain bytes the terminal wants written back to the pty.
    pub fn drain_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.responses)
    }

    /// Queue raw bytes to be sent to the pty (used by the graphics glue).
    /// Dropped once [`MAX_PENDING_RESPONSE_BYTES`] are already queued.
    pub fn push_response(&mut self, bytes: &[u8]) {
        if self.responses.len().saturating_add(bytes.len()) > MAX_PENDING_RESPONSE_BYTES {
            return;
        }
        self.responses.extend_from_slice(bytes);
    }

    /// Drain queued events (already coalesced, see [`coalesce_events`]).
    pub fn drain_events(&mut self) -> Vec<TermEvent> {
        self.clip_query_pending = false;
        self.events_compact_at = EVENTS_COMPACT_MIN;
        let mut v = std::mem::take(&mut self.events);
        coalesce_events(&mut v);
        v
    }

    /// Queue an event for the host. A flood of BEL / title / clipboard
    /// sequences would otherwise grow this queue without bound (and make the
    /// host do one OS call per sequence), so the queue is coalesced whenever
    /// it reaches a threshold that doubles while it stays large.
    pub(crate) fn push_event(&mut self, ev: TermEvent) {
        self.events.push(ev);
        if self.events.len() >= self.events_compact_at {
            coalesce_events(&mut self.events);
            self.events_compact_at = (self.events.len() * 2).max(EVENTS_COMPACT_MIN);
        }
    }

    /// Answer a [`TermEvent::ClipboardQuery`] with the clipboard contents.
    /// Contents longer than [`MAX_CLIPBOARD_BYTES`] are reported as empty
    /// rather than truncated (a cut-off base64 payload would be corrupt).
    pub fn respond_clipboard(&mut self, text: &str) {
        let text = if text.len() > MAX_CLIPBOARD_BYTES { "" } else { text };
        let r = format!(
            "\x1b]52;{};{}\x1b\\",
            self.clip_selection,
            util::base64_encode(text.as_bytes())
        );
        // A single reply (<= ~1.4 MiB) is exempt from the small-reply cap, but
        // only one clipboard reply may be pending at a time.
        if self.responses.len() <= MAX_PENDING_RESPONSE_BYTES {
            self.responses.extend_from_slice(r.as_bytes());
        }
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
    #[cfg(test)]
    pub(crate) fn main_grid(&self) -> &Grid {
        &self.main
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
    /// The blink a program asked for, or `None` when the host's configured
    /// default should apply (nothing set it, or DECSCUSR 0 / RIS undid it).
    pub fn cursor_blink_override(&self) -> Option<bool> {
        self.modes.cursor_blink_set.then_some(self.modes.cursor_blink)
    }
    /// The shape a program asked for with DECSCUSR 1-6, or `None` for the
    /// host's configured shape.
    pub fn cursor_shape_override(&self) -> Option<CursorShape> {
        self.modes.cursor_shape_set.then_some(self.cursor_shape)
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
    /// `(count, top)` for `screen`: incremented on every ED 2 (erase whole
    /// display) that ran on it and on RIS (main), with `top` the absolute row
    /// of its first row at that time, so a host can drop things that live
    /// outside the grid (kitty graphics placements) from the right screen even
    /// when the clear and a screen switch arrive in one read.
    pub fn display_clear(&self, screen: Screen) -> (u64, u64) {
        self.display_clears[match screen {
            Screen::Main => 0,
            Screen::Alt => 1,
        }]
    }

    /// Record an ED 2 on the active screen.
    pub(crate) fn note_display_clear(&mut self) {
        self.note_display_clear_on(self.active);
    }

    fn note_display_clear_on(&mut self, screen: Screen) {
        let (i, top) = match screen {
            Screen::Main => (0, self.main.scrolled_total()),
            Screen::Alt => (1, self.alt.scrolled_total()),
        };
        self.display_clears[i] = (self.display_clears[i].0 + 1, top);
    }
    /// Incremented on every RIS (full reset), which also discards scrollback,
    /// so a host can drop everything it keeps outside the grid (images).
    pub fn reset_count(&self) -> u64 {
        self.reset_count
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
        // One mark per kind per row (the latest wins) bounds the memory a
        // flood of marks on a fixed row can take.
        match row.marks.iter_mut().find(|m| m.kind == mark.kind) {
            Some(m) => *m = mark,
            None => row.marks.push(mark),
        }
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

    /// Resize the terminal. A column change rewraps the primary screen
    /// (scrollback and visible rows, see `reflow.rs`) and queues
    /// [`TermEvent::Reflowed`]; the alternate screen just truncates or
    /// extends rows, as kitty does. Row-only changes truncate (pushing top
    /// lines into scrollback if the cursor would fall off) or extend
    /// (pulling lines back from scrollback if any).
    pub fn resize<C: TryInto<usize>, R: TryInto<usize>>(&mut self, cols: C, rows: R) {
        let cols = to_usize(cols).max(1);
        let rows = to_usize(rows).max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }
        let reflow = cols != self.cols;
        let mut reflow_pending = false;
        let shift_main;
        if reflow {
            // Main-screen positions that must follow their characters: the
            // cursor (or, with the alt screen up, the cursor saved on entry)
            // and DECSC's saved cursor.
            let mut track = [Track { row: 0, col: 0, pending: false }; 2];
            let mut n = 1;
            if self.active == Screen::Main {
                track[0] = Track { row: self.cursor.row, col: self.cursor.col, pending: self.pending_wrap };
                if let Some(s) = self.saved[0] {
                    track[1] = Track { row: s.row, col: s.col, pending: false };
                    n = 2;
                }
            } else if let Some(s) = self.saved[0] {
                track[0] = Track { row: s.row, col: s.col, pending: false };
            }
            self.main.reflow(cols, rows, &mut track[..n]);
            if self.active == Screen::Main {
                self.cursor.row = track[0].row;
                self.cursor.col = track[0].col;
                reflow_pending = track[0].pending && self.modes.wrap;
                if n == 2 {
                    if let Some(s) = &mut self.saved[0] {
                        s.row = track[1].row;
                        s.col = track[1].col;
                    }
                }
            } else if let Some(s) = &mut self.saved[0] {
                s.row = track[0].row;
                s.col = track[0].col;
            }
            shift_main = 0;
            self.push_event(TermEvent::Reflowed);
        } else {
            let main_cursor_row = if self.active == Screen::Main {
                self.cursor.row
            } else {
                self.saved[0].map(|s| s.row).unwrap_or(0)
            };
            shift_main = self.main.resize(cols, rows, main_cursor_row);
        }
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
        self.pending_wrap = reflow_pending;
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
        if cell.combining.len() >= MAX_COMBINING {
            // Joins the cluster but the cell is full: drop the extra mark.
            return true;
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
                let mut pad = Cell::erased(bg);
                pad.flags.insert(Flags::WRAP_PAD);
                g.row_mut(r).cells[col] = pad;
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
        if style.hyperlink_id.is_some() {
            self.link_unused = false;
        }
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
        let to = to.min(self.rows);
        if from >= to {
            return;
        }
        let to = from + self.spend_rows(to - from);
        for r in from..to {
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
        // Entering clears the whole alternate screen.
        if !self.try_spend(self.rows as u64 * self.cols as u64) {
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
        if uri.len() > MAX_HYPERLINK_URI || id.as_ref().is_some_and(|i| i.len() > MAX_HYPERLINK_ID) {
            return None;
        }
        let size = uri.len() + id.as_ref().map_or(0, |i| i.len());
        if let Some(id) = &id {
            if let Some(&n) = self.hyperlink_map.get(&(id.clone(), uri.clone())) {
                return Some(n);
            }
        }
        // The newest link no cell has used yet is dead weight when another
        // OSC 8 follows: reuse or overwrite its slot instead of growing the
        // table (a stream of unused links must not accumulate).
        if self.link_unused {
            if let Some(last) = self.hyperlinks.len().checked_sub(1) {
                let pinned = self
                    .saved
                    .iter()
                    .flatten()
                    .any(|s| s.pen.hyperlink_id == Some(last as u32));
                if !pinned {
                    let old = &self.hyperlinks[last];
                    if old.id == id && old.uri == uri {
                        return Some(last as u32);
                    }
                    let old_size = old.uri.len() + old.id.as_ref().map_or(0, |i| i.len());
                    if let Some(oid) = &old.id {
                        self.hyperlink_map.remove(&(oid.clone(), old.uri.clone()));
                    }
                    self.link_bytes = self.link_bytes - old_size + size;
                    if let Some(id) = &id {
                        self.hyperlink_map.insert((id.clone(), uri.clone()), last as u32);
                    }
                    self.hyperlinks[last] = Hyperlink { id, uri };
                    return Some(last as u32);
                }
            }
        }
        let full = |t: &Terminal| {
            t.hyperlinks.len() >= MAX_HYPERLINKS || t.link_bytes + size > MAX_HYPERLINK_BYTES
        };
        if full(self) {
            // GC is a full scan of the scrollback: only run it when the input
            // since the last one pays for it, so a table kept full of live
            // links cannot make every new OSC 8 expensive.
            if self.gc_idle_bytes.saturating_mul(GC_CELLS_PER_INPUT_BYTE) < self.gc_last_cost {
                return None;
            }
            self.gc_hyperlinks();
            if full(self) {
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
        self.link_bytes += size;
        self.link_unused = true;
        Some(n)
    }

    /// Drop hyperlinks no cell (or saved pen) references any more and
    /// renumber the rest. Called only when the table is full.
    fn gc_hyperlinks(&mut self) {
        self.gc_runs += 1;
        self.gc_idle_bytes = 0;
        let cells = ((self.main.scrollback_len() + self.main.rows() + self.alt.rows()) as u64)
            .saturating_mul(self.cols as u64);
        self.gc_last_cost = cells.saturating_mul(2);
        self.link_unused = false;
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
        self.link_bytes = kept
            .iter()
            .map(|h| h.uri.len() + h.id.as_ref().map_or(0, |i| i.len()))
            .sum();
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
        self.reset_count += 1;
        self.note_display_clear_on(Screen::Main);
        self.note_display_clear_on(Screen::Alt);
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
        self.main.clear_scrollback();
        self.alt.clear(Color::Default);
        self.display_offset = 0;
        self.last_cell = None;
        // No cell references a link any more.
        self.hyperlinks = Vec::new();
        self.hyperlink_map = HashMap::new();
        self.link_bytes = 0;
        self.link_unused = false;
        for i in 0..256 {
            self.reset_palette_entry(i);
        }
        self.colors.foreground = self.colors.base_foreground;
        self.colors.background = self.colors.base_background;
        self.colors.cursor = self.colors.base_cursor;
        self.mark_all();
        self.push_event(TermEvent::ColorChanged);
        self.push_event(TermEvent::SetMouseMode);
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

/// Length of an incomplete (but so far valid) UTF-8 sequence at the end of
/// `bytes`, 0 if it ends on a character boundary or in invalid data.
fn incomplete_utf8_tail(bytes: &[u8]) -> usize {
    for k in 1..=3.min(bytes.len()) {
        let tail = &bytes[bytes.len() - k..];
        if let Err(e) = std::str::from_utf8(tail) {
            if e.valid_up_to() == 0 && e.error_len().is_none() {
                return k;
            }
        }
    }
    0
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
