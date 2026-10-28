//! Glue between the vt crate and the kitty graphics protocol implementation.
//!
//! TODO(graphics agent): this is a stub. `app.rs` calls [`handle`] for every
//! `TermEvent::GraphicsCommand`. The body should (see ARCHITECTURE.md,
//! "Graphics glue"):
//!
//! 1. build a `grimalkin_graphics::CursorCtx` from `term` (cursor column, absolute
//!    cursor row, cols/rows, `term.cell_pixel_size()`, and the screen mapped to
//!    `grimalkin_graphics::Screen`),
//! 2. call `protocol.set_viewport_top(screen, abs_top_row)` and then
//!    `protocol.handle(payload, &ctx)`,
//! 3. apply the returned actions: `Respond(bytes)` -> `term.push_response`,
//!    `MoveCursor { dcols, drows }` -> `term.move_cursor_rel`, `Dirty` -> return
//!    true so the app redraws,
//! 4. garbage collect placements with `protocol.gc_below` as scrollback trims.
//!
//! The renderer part (texture upload from `take_dirty_images`, drawing
//! placements in the two image passes) lives in `renderer.rs`, where the
//! PASS 2 / PASS 4 hooks are marked.

use grimalkin_graphics::Protocol;
use grimalkin_vt::Terminal;

/// Handle one APC `G` payload (the bytes between `ESC _ G` and `ESC \`).
/// Returns true if a redraw is needed.
pub fn handle(term: &mut Terminal, protocol: &mut Protocol, payload: &[u8]) -> bool {
    // TODO(graphics agent): see the module documentation.
    let _ = (term, protocol, payload);
    false
}
