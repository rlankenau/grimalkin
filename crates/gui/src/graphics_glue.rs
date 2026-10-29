//! Glue between the vt crate and the kitty graphics protocol implementation.
//!
//! [`Graphics`] owns the [`Protocol`] (image store + placements) and applies
//! the protocol's actions to the [`Terminal`]:
//!
//! * `Respond(bytes)` -> `Terminal::push_response`
//! * `MoveCursor { dcols, drows }` -> `Terminal::move_cursor_rel` (dropped when
//!   `[pty] graphics_cursor` resolves to "none", see
//!   [`crate::config::GraphicsCursor`]: ConPTY does not learn about the move)
//! * `Dirty` -> a redraw is needed ([`Graphics::take_dirty`])
//!
//! Timing matters: vt queues `TermEvent::GraphicsCommand` while it parses, and
//! a graphics command must be handled with the cursor where it was when the
//! command arrived (text may follow in the same read). [`Graphics::feed`]
//! therefore advances the terminal in segments that end at every `ESC \`
//! (string terminator) and handles graphics commands right after the segment
//! that completed them.
//!
//! Placements are anchored to absolute rows, so they scroll with the text for
//! free. [`Graphics::housekeeping`] drops the ones that fell out of scrollback
//! (`gc_below`), the alternate-screen ones when it is left (`clear_screen`) and
//! the visible ones when the program erases the display (ED 2 / RIS).
//!
//! Drawing lives in `images.rs`.

use grimalkin_graphics::{Action, CursorCtx, Protocol, Screen as GScreen};
use grimalkin_vt::{Screen, TermEvent, Terminal};

pub struct Graphics {
    protocol: Protocol,
    dirty: bool,
    /// Non-graphics events seen while splitting; returned by `drain_events`.
    deferred: Vec<TermEvent>,
    /// The previous byte fed was ESC (an `ESC \` may straddle two reads).
    prev_esc: bool,
    last_screen: Screen,
    last_alt_enter: u64,
    last_clear_count: u64,
    last_reset_count: u64,
    /// Oldest retained absolute row per screen (`[main, alt]`).
    last_oldest: [u64; 2],
    /// Apply the protocol's `MoveCursor` actions (false = behave as if every
    /// placement had `C=1`).
    move_cursor: bool,
}

fn map_screen(s: Screen) -> GScreen {
    match s {
        Screen::Main => GScreen::Main,
        Screen::Alt => GScreen::Alt,
    }
}

fn screen_index(s: Screen) -> usize {
    match s {
        Screen::Main => 0,
        Screen::Alt => 1,
    }
}

/// Absolute row of the first row of the *live* screen (ignores scrollback view).
fn live_top(term: &Terminal) -> u64 {
    term.abs_row_of_visible_row(0) + term.display_offset() as u64
}

impl Graphics {
    #[cfg(test)]
    pub fn new() -> Graphics {
        Graphics::with_cursor_moves(true)
    }

    /// Like [`Graphics::new`], choosing whether placements move the cursor.
    pub fn with_cursor_moves(move_cursor: bool) -> Graphics {
        Graphics {
            move_cursor,
            protocol: Protocol::new(),
            dirty: false,
            deferred: Vec::new(),
            prev_esc: false,
            last_screen: Screen::Main,
            last_alt_enter: 0,
            last_clear_count: 0,
            last_reset_count: 0,
            last_oldest: [0; 2],
        }
    }

    /// The protocol state, for the renderer.
    pub fn protocol_mut(&mut self) -> &mut Protocol {
        &mut self.protocol
    }

    /// True (once) if a graphics command changed what should be on screen.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Feed pty output to the terminal, handling graphics commands at the
    /// point in the stream where they occur.
    pub fn feed(&mut self, term: &mut Terminal, bytes: &[u8]) {
        let mut start = 0;
        let mut prev_esc = self.prev_esc;
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'\\' && prev_esc {
                term.advance(&bytes[start..=i]);
                start = i + 1;
                self.pump(term);
            }
            prev_esc = b == 0x1b;
        }
        if start < bytes.len() {
            term.advance(&bytes[start..]);
        }
        self.prev_esc = prev_esc;
        self.housekeeping(term);
    }

    /// Terminal events not consumed by the graphics layer.
    pub fn drain_events(&mut self, term: &mut Terminal) -> Vec<TermEvent> {
        self.pump(term);
        std::mem::take(&mut self.deferred)
    }

    fn pump(&mut self, term: &mut Terminal) {
        for ev in term.drain_events() {
            match ev {
                TermEvent::GraphicsCommand(payload) => self.handle(term, &payload),
                other => self.deferred.push(other),
            }
        }
    }

    fn context(term: &Terminal) -> CursorCtx {
        let (cw, ch) = term.cell_pixel_size();
        CursorCtx {
            col: term.cursor().col.min(u16::MAX as usize) as u16,
            abs_row: term.cursor_abs_row(),
            cols: term.cols().min(u16::MAX as usize) as u16,
            rows: term.rows().min(u16::MAX as usize) as u16,
            cell_w_px: cw,
            cell_h_px: ch,
            screen: map_screen(term.screen()),
        }
    }

    /// Handle one APC `G` payload (the bytes between `ESC _ G` and `ESC \`).
    fn handle(&mut self, term: &mut Terminal, payload: &[u8]) {
        self.housekeeping(term);
        let ctx = Self::context(term);
        self.protocol.set_viewport_top(ctx.screen, live_top(term));
        for action in self.protocol.handle(payload, &ctx) {
            match action {
                Action::Respond(bytes) => term.push_response(&bytes),
                Action::MoveCursor { dcols, drows } => {
                    if self.move_cursor {
                        term.move_cursor_rel(dcols, drows);
                    }
                }
                Action::Dirty => self.dirty = true,
            }
        }
    }

    /// Drop placements that can no longer be shown.
    pub fn housekeeping(&mut self, term: &Terminal) {
        let screen = term.screen();
        let enters = term.alt_enter_count();
        if enters != self.last_alt_enter || (self.last_screen == Screen::Alt && screen == Screen::Main) {
            // The alternate screen was left (or re-entered): its images go.
            self.protocol.clear_screen(GScreen::Alt);
            self.last_oldest[1] = 0;
            self.dirty = true;
        }
        self.last_alt_enter = enters;
        self.last_screen = screen;

        let resets = term.reset_count();
        if resets != self.last_reset_count {
            // RIS wipes scrollback too: nothing placed earlier can be shown
            // again, so drop every placement and the image data with it.
            self.last_reset_count = resets;
            self.protocol.clear_all();
            self.last_oldest = [0; 2];
            self.dirty = true;
        }

        let clears = term.display_clear_count();
        if clears != self.last_clear_count {
            self.last_clear_count = clears;
            // ED 2 / RIS: delete what is visible (kitty `a=d,d=a`); the image
            // data stays, so later `a=p` puts still work.
            let ctx = Self::context(term);
            self.protocol.set_viewport_top(ctx.screen, live_top(term));
            let _ = self.protocol.handle(b"a=d,d=a,q=2", &ctx);
            self.dirty = true;
        }

        let oldest = term.oldest_abs_row();
        let idx = screen_index(screen);
        if oldest > self.last_oldest[idx] {
            self.protocol.gc_below(map_screen(screen), oldest);
        }
        self.last_oldest[idx] = oldest;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grimalkin_graphics::Screen as GS;

    /// One 1x1 RGB pixel, transmit and display, quiet.
    const PUT: &[u8] = b"\x1b_Ga=T,f=24,s=1,v=1,i=7,q=2;/wAA\x1b\\";

    fn setup() -> (Terminal, Graphics) {
        let mut t = Terminal::new(20, 5, 100);
        t.set_cell_pixel_size(10, 20);
        (t, Graphics::new())
    }

    fn placements(g: &mut Graphics) -> Vec<(u16, u64)> {
        g.protocol_mut()
            .placements_in(GS::Main, 0..=u64::MAX)
            .map(|p| (p.col, p.abs_row))
            .collect()
    }

    #[test]
    fn placement_uses_cursor_at_the_time_of_the_command() {
        let (mut t, mut g) = setup();
        let mut bytes = b"abc".to_vec();
        bytes.extend_from_slice(PUT);
        bytes.extend_from_slice(b"\r\nlater text");
        g.feed(&mut t, &bytes);
        assert_eq!(placements(&mut g), vec![(3, 0)]);
    }

    #[test]
    fn terminator_split_across_reads() {
        let (mut t, mut g) = setup();
        let (a, b) = PUT.split_at(PUT.len() - 1); // ... ESC | \
        g.feed(&mut t, b"xy");
        g.feed(&mut t, a);
        g.feed(&mut t, b);
        g.feed(&mut t, b"zzzz");
        assert_eq!(placements(&mut g), vec![(2, 0)]);
        assert!(g.take_dirty());
    }

    #[test]
    fn other_events_are_passed_through() {
        let (mut t, mut g) = setup();
        let mut bytes = b"\x1b]0;hello\x1b\\".to_vec();
        bytes.extend_from_slice(PUT);
        g.feed(&mut t, &bytes);
        let evs = g.drain_events(&mut t);
        assert!(evs.iter().any(|e| matches!(e, TermEvent::Title(s) if s == "hello")));
        assert!(!evs.iter().any(|e| matches!(e, TermEvent::GraphicsCommand(_))));
    }

    #[test]
    fn query_response_reaches_the_pty() {
        let (mut t, mut g) = setup();
        g.feed(&mut t, b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;/wAA\x1b\\");
        let resp = t.drain_responses();
        assert!(String::from_utf8_lossy(&resp).contains("i=31;OK"), "{resp:?}");
    }

    #[test]
    fn erase_display_removes_visible_placements_but_keeps_data() {
        let (mut t, mut g) = setup();
        g.feed(&mut t, PUT);
        assert_eq!(placements(&mut g).len(), 1);
        g.feed(&mut t, b"\x1b[2J");
        assert!(placements(&mut g).is_empty());
        assert!(g.protocol_mut().image(7).is_some());
    }

    #[test]
    fn ris_drops_scrolled_back_placements_and_image_data() {
        let (mut t, mut g) = setup();
        g.feed(&mut t, PUT);
        g.feed(&mut t, "\r\nx".repeat(8).as_bytes());
        assert!(t.scrollback_len() > 0);
        assert_eq!(placements(&mut g).len(), 1, "scrolled into history, still placed");
        assert!(g.protocol_mut().image(7).is_some());
        g.feed(&mut t, b"\x1bc");
        assert_eq!(t.scrollback_len(), 0);
        assert!(placements(&mut g).is_empty());
        assert!(g.protocol_mut().image(7).is_none());
        assert_eq!(g.protocol_mut().image_count(), 0);
        // Later output still works.
        g.feed(&mut t, PUT);
        assert_eq!(placements(&mut g).len(), 1);
    }

    #[test]
    fn alt_screen_placements_die_when_leaving_alt() {
        let (mut t, mut g) = setup();
        g.feed(&mut t, b"\x1b[?1049h");
        g.feed(&mut t, PUT);
        assert_eq!(g.protocol_mut().placements_in(GS::Alt, 0..=u64::MAX).count(), 1);
        g.feed(&mut t, b"\x1b[?1049l");
        assert_eq!(g.protocol_mut().placements_in(GS::Alt, 0..=u64::MAX).count(), 0);
    }

    #[test]
    fn cursor_moves_only_when_enabled() {
        let cursor_after = |moves: bool| {
            let mut t = Terminal::new(20, 5, 100);
            t.set_cell_pixel_size(10, 20);
            let mut g = Graphics::with_cursor_moves(moves);
            g.feed(&mut t, PUT);
            let c = t.cursor();
            (c.col, t.cursor_abs_row())
        };
        let moved = cursor_after(true);
        let still = cursor_after(false);
        assert_ne!(moved, still);
        assert_eq!(still, (0, 0));
    }

    #[test]
    fn scrolled_off_placements_are_collected() {
        let mut t = Terminal::new(20, 3, 2);
        t.set_cell_pixel_size(10, 20);
        let mut g = Graphics::new();
        g.feed(&mut t, PUT);
        assert_eq!(placements(&mut g).len(), 1);
        g.feed(&mut t, "\r\nx".repeat(10).as_bytes());
        assert!(placements(&mut g).is_empty());
    }
}
