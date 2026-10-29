//! grimalkin-vt: a terminal state machine (no I/O, no GUI).
//!
//! Feed pty output to [`Terminal::advance`], read the grid through
//! [`Terminal::visible_rows`], collect [`Terminal::take_damage`], and write
//! [`Terminal::drain_responses`] back to the pty.

mod cell;
mod grid;
mod perform;
mod term;
mod util;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod review_tests;
#[cfg(test)]
mod limits_tests;

pub use cell::{
    default_palette_rgb, Cell, CellWidth, Color, Flags, LineMark, MarkKind, Row, Style, Underline,
};
pub use grid::Grid;
pub use term::{
    ColorTable, Cursor, CursorShape, Damage, Direction, Hyperlink, Modes, MouseEncoding,
    MouseMode, Rgb, Screen, TermEvent, Terminal, MAX_CLIPBOARD_BYTES, MAX_COMBINING,
    MAX_PENDING_RESPONSE_BYTES, MAX_STRING_BODY,
};
