//! Text selection (click-drag, double-click word, triple-click line, copy).
//!
//! TODO(selection agent): this module is a skeleton. `app.rs` already calls
//! `on_press` / `on_drag` / `on_release` for every mouse event it receives
//! (after mouse reporting declined the event) and `selected_text` for the copy
//! shortcut; `renderer.rs` calls `contains` for every visible cell while a
//! selection is active (see `is_active`). Implementing the selection should not
//! require touching the structure of `app.rs`: fill in the bodies below, and
//! return `true` from the hooks when the selection changed so the app can
//! request a redraw.
//!
//! Coordinates: selections are stored in the terminal's absolute row space
//! (`Terminal::abs_row_of_visible_row`) so they survive scrolling.

use std::time::Instant;

use grimalkin_vt::Terminal;
use winit::event::MouseButton;
use winit::keyboard::ModifiersState;

/// A cell position in the visible viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CellPos {
    pub col: usize,
    /// Visible row (0 = top of the viewport).
    pub row: usize,
}

#[derive(Debug, Default)]
pub struct Selection {
    // TODO: anchor/head in (abs_row, col), mode (cell/word/line/block), click
    // count tracking.
}

impl Selection {
    pub fn new() -> Self {
        Selection::default()
    }

    /// True when there is something to highlight. The renderer skips the
    /// per-cell `contains` calls when this is false.
    pub fn is_active(&self) -> bool {
        false
    }

    pub fn clear(&mut self) {}

    /// Is the cell at `(abs_row, col)` selected?
    pub fn contains(&self, _abs_row: u64, _col: usize) -> bool {
        false
    }

    /// Mouse button pressed over `pos`. Returns true if the selection changed.
    pub fn on_press(
        &mut self,
        _term: &Terminal,
        _pos: CellPos,
        _button: MouseButton,
        _mods: ModifiersState,
        _now: Instant,
    ) -> bool {
        false
    }

    /// Mouse moved with the left button held. Returns true if it changed.
    pub fn on_drag(&mut self, _term: &Terminal, _pos: CellPos) -> bool {
        false
    }

    /// Mouse button released. Returns true if the selection changed.
    pub fn on_release(&mut self, _term: &Terminal, _pos: CellPos, _button: MouseButton) -> bool {
        false
    }

    /// The selected text, for copying.
    pub fn selected_text(&self, _term: &Terminal) -> Option<String> {
        None
    }
}
