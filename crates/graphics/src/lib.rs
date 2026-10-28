//! Kitty graphics protocol implementation and image store for grimalkin.
//!
//! See <https://sw.kovidgoyal.net/kitty/graphics-protocol/>. The entry point
//! is [`Protocol::handle`], which takes the bytes between `ESC _ G` and
//! `ESC \` plus a [`CursorCtx`] describing the terminal at that moment and
//! returns [`Action`]s for the terminal to apply.
//!
//! Notes for the glue code:
//!
//! * `MoveCursor { dcols, drows }` is relative to the cursor given in the
//!   [`CursorCtx`]: kitty-style, the cursor ends on the last row the image
//!   covers, in the cell after its last column (clamped to the last column of
//!   the screen).
//! * Delete commands that refer to "the visible screen" or to cell
//!   coordinates need to know the absolute row of the top visible line. Call
//!   [`Protocol::set_viewport_top`] before `handle` when it is known; without
//!   it the cursor is assumed to be on the bottom row.
//! * Virtual placements (`U=1`) are stored but are NOT returned by
//!   [`Protocol::placements_in`]; use [`Protocol::virtual_placements`].

mod b64;
mod cmd;
mod loader;

use cmd::Cmd;
use loader::{err, GErr};
use std::collections::HashMap;
use std::ops::{Bound, RangeBounds};
use std::sync::Arc;

/// Maximum accepted image width or height in pixels.
pub const MAX_DIMENSION: u32 = 10_000;
/// Default image memory quota (matches kitty).
pub const DEFAULT_MEMORY_LIMIT: usize = 320 * 1024 * 1024;
const FALLBACK_CELL_W: u32 = 8;
const FALLBACK_CELL_H: u32 = 16;
const FIRST_AUTO_ID: u32 = 0x8000_0000;

/// Which screen buffer a placement belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Screen {
    Main,
    Alt,
}

impl Screen {
    fn idx(self) -> usize {
        match self {
            Screen::Main => 0,
            Screen::Alt => 1,
        }
    }
}

/// Terminal state needed to interpret a command.
#[derive(Debug, Clone, Copy)]
pub struct CursorCtx {
    pub col: u16,
    pub abs_row: u64,
    pub cols: u16,
    pub rows: u16,
    pub cell_w_px: u32,
    pub cell_h_px: u32,
    pub screen: Screen,
}

/// What the terminal should do after a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Bytes to write back to the pty (a complete APC response).
    Respond(Vec<u8>),
    /// Move the cursor relative to the [`CursorCtx`] cursor.
    MoveCursor { dcols: i32, drows: i32 },
    /// Placements changed; the screen needs a redraw.
    Dirty,
}

/// A stored image (always RGBA8).
#[derive(Debug, Clone)]
pub struct Image {
    pub id: u32,
    /// Image number (`I` key) or 0.
    pub number: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
    /// Changes every time the pixel data for this id is (re)transmitted.
    pub texture_version: u64,
}

/// Source rectangle in image pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// One display of an image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub image_id: u32,
    /// 0 when the placement has no id.
    pub placement_id: u32,
    pub screen: Screen,
    pub col: u16,
    pub abs_row: u64,
    pub z: i32,
    /// Cells covered horizontally / vertically.
    pub cols: u32,
    pub rows: u32,
    /// Resolved (clamped, non-empty) source rectangle.
    pub src_rect: Rect,
    /// `X`, `Y` pixel offset inside the first cell.
    pub pixel_offset: (u32, u32),
    /// Size in pixels the source rectangle is stretched to when displayed
    /// (native size without `c`/`r`; `c*cell_w` / `r*cell_h` otherwise, with
    /// the other axis derived to keep the aspect ratio when only one is set).
    pub dest_px: (u32, u32),
    /// Virtual placement for unicode placeholders (`U=1`); has no position.
    pub is_virtual: bool,
}

impl Placement {
    /// Last absolute row covered.
    pub fn last_row(&self) -> u64 {
        self.abs_row.saturating_add(self.rows.max(1) as u64 - 1)
    }

    fn covers_cell(&self, col: u64, row: u64) -> bool {
        col >= self.col as u64
            && col < self.col as u64 + self.cols as u64
            && row >= self.abs_row
            && row <= self.last_row()
    }
}

struct Meta {
    /// Creation order.
    seq: u64,
    /// Last put/creation, for eviction.
    last_used: u64,
    /// Image can never be referenced by id or number again.
    anonymous: bool,
}

struct Pending {
    cmd: Cmd,
    data: Vec<u8>,
}

/// Protocol state machine and image store.
pub struct Protocol {
    images: HashMap<u32, Image>,
    meta: HashMap<u32, Meta>,
    placements: Vec<Placement>,
    pending: Option<Pending>,
    dirty: Vec<u32>,
    removed: Vec<u32>,
    memory_limit: usize,
    used: usize,
    next_seq: u64,
    next_version: u64,
    next_auto_id: u32,
    viewport_top: [Option<u64>; 2],
    changed: bool,
}

impl Default for Protocol {
    fn default() -> Self {
        Self::new()
    }
}

impl Protocol {
    pub fn new() -> Self {
        Self::with_memory_limit(DEFAULT_MEMORY_LIMIT)
    }

    pub fn with_memory_limit(bytes: usize) -> Self {
        Protocol {
            images: HashMap::new(),
            meta: HashMap::new(),
            placements: Vec::new(),
            pending: None,
            dirty: Vec::new(),
            removed: Vec::new(),
            memory_limit: bytes,
            used: 0,
            next_seq: 1,
            next_version: 1,
            next_auto_id: FIRST_AUTO_ID,
            viewport_top: [None, None],
            changed: false,
        }
    }

    /// Change the memory quota; evicts immediately if now over.
    pub fn set_memory_limit(&mut self, bytes: usize) {
        self.memory_limit = bytes;
        self.evict_for(0, 0);
    }

    pub fn memory_limit(&self) -> usize {
        self.memory_limit
    }

    /// Bytes of pixel data currently stored.
    pub fn memory_used(&self) -> usize {
        self.used
    }

    /// Tell the protocol the absolute row of the top visible line of `screen`.
    pub fn set_viewport_top(&mut self, screen: Screen, abs_row_top: u64) {
        self.viewport_top[screen.idx()] = Some(abs_row_top);
    }

    // ---------------------------------------------------------------- queries

    pub fn image(&self, id: u32) -> Option<&Image> {
        self.images.get(&id)
    }

    pub fn image_count(&self) -> usize {
        self.images.len()
    }

    pub fn placement_count(&self) -> usize {
        self.placements.len()
    }

    /// Real (non-virtual) placements on `screen` whose rows intersect `range`.
    pub fn placements_in<R: RangeBounds<u64>>(
        &self,
        screen: Screen,
        range: R,
    ) -> impl Iterator<Item = &Placement> + '_ {
        let lo = match range.start_bound() {
            Bound::Included(&v) => v,
            Bound::Excluded(&v) => v.saturating_add(1),
            Bound::Unbounded => 0,
        };
        let hi = match range.end_bound() {
            Bound::Included(&v) => Some(v),
            Bound::Excluded(&v) => v.checked_sub(1),
            Bound::Unbounded => Some(u64::MAX),
        };
        self.placements.iter().filter(move |p| {
            !p.is_virtual
                && p.screen == screen
                && hi.is_some_and(|hi| p.abs_row <= hi && p.last_row() >= lo)
        })
    }

    /// Like [`Protocol::placements_in`], but in draw order: ascending `z`,
    /// then ascending image id (kitty: of two overlapping images with the same
    /// z, the lower id is below), then creation order. Draw those with
    /// `z < 0` under the text and the rest over it.
    pub fn placements_in_draw_order<R: RangeBounds<u64>>(&self, screen: Screen, range: R) -> Vec<&Placement> {
        let mut v: Vec<&Placement> = self.placements_in(screen, range).collect();
        v.sort_by_key(|p| (p.z, p.image_id)); // stable: keeps creation order
        v
    }

    /// Virtual (`U=1`) placements on `screen`.
    pub fn virtual_placements(&self, screen: Screen) -> impl Iterator<Item = &Placement> + '_ {
        self.placements.iter().filter(move |p| p.is_virtual && p.screen == screen)
    }

    /// Ids of images whose pixel data is new or replaced since the last call.
    pub fn take_dirty_images(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.dirty)
    }

    /// Ids of images that were deleted/evicted since the last call, so the
    /// renderer can drop their textures.
    pub fn take_removed_images(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.removed)
    }

    /// Drop placements on `screen` that end above `abs_row` (scrolled out of
    /// the scrollback). Orphaned anonymous images are freed.
    pub fn gc_below(&mut self, screen: Screen, abs_row: u64) {
        self.remove_where(false, |p| p.screen == screen && !p.is_virtual && p.last_row() < abs_row);
    }

    /// Remove every placement on `screen` (including virtual ones).
    pub fn clear_screen(&mut self, screen: Screen) {
        self.remove_where(false, |p| p.screen == screen);
    }

    // --------------------------------------------------------------- handling

    /// Process one graphics command (bytes between `ESC _ G` and `ESC \`).
    pub fn handle(&mut self, payload: &[u8], ctx: &CursorCtx) -> Vec<Action> {
        self.changed = false;
        let (ctl, data) = cmd::split_payload(payload);
        let command = cmd::parse(ctl);
        let mut out = Vec::new();
        self.dispatch(command, data, ctx, &mut out);
        if self.changed {
            out.insert(0, Action::Dirty);
        }
        out
    }

    fn pending_cap(&self) -> usize {
        (self.memory_limit / 3).saturating_mul(4).saturating_add(4096)
    }

    fn dispatch(&mut self, command: Cmd, data: &[u8], ctx: &CursorCtx, out: &mut Vec<Action>) {
        if let Some(msg) = command.bad.clone() {
            self.pending = None;
            reply(out, &command, command.id, command.number, command.pid, Err(err("EINVAL", msg)));
            return;
        }
        // Feature queries never touch state, not even a chunked upload.
        if command.action == Some(b'q') && command.feature.is_some() {
            feature_reply(&command, out);
            return;
        }
        if let Some(p) = &self.pending {
            let first_action = p.cmd.action_or_default();
            // Strictly only `m`/`q` may appear on a continuation chunk, but
            // tolerate clients that repeat the same `i`/`I` (and `a`) on it.
            let same_ids = (command.id == 0 || command.id == p.cmd.id)
                && (command.number == 0 || command.number == p.cmd.number);
            let continues = !command.has_transmit_keys
                && same_ids
                && command.action.is_none_or(|a| a == first_action);
            if continues {
                let cap = self.pending_cap();
                let p = self.pending.as_mut().expect("pending checked above");
                if p.data.len() + data.len() > cap {
                    let first = self.pending.take().expect("pending checked above").cmd;
                    reply(out, &first, first.id, first.number, first.pid, Err(err("EFBIG", "Image data too large")));
                    return;
                }
                p.data.extend_from_slice(data);
                if command.more {
                    return;
                }
                let p = self.pending.take().expect("pending checked above");
                let mut first = p.cmd;
                if command.has_quiet {
                    first.quiet = command.quiet;
                }
                first.more = false;
                self.complete(&first, &p.data, ctx, out);
                return;
            }
            // Anything else abandons the partial upload.
            self.pending = None;
        }
        let action = command.action_or_default();
        match action {
            b't' | b'T' | b'q' => {
                if command.more {
                    if data.len() > self.pending_cap() {
                        reply(out, &command, command.id, command.number, command.pid, Err(err("EFBIG", "Image data too large")));
                        return;
                    }
                    self.pending = Some(Pending { cmd: command, data: data.to_vec() });
                } else {
                    self.complete(&command, data, ctx, out);
                }
            }
            b'p' => self.put_cmd(&command, ctx, out),
            b'd' => self.delete(&command, ctx, out),
            b'f' | b'a' | b'c' => {
                // Animation is not implemented; tell clients (once per
                // transfer) so they can fall back.
                if !command.more {
                    reply(out, &command, command.id, command.number, 0, Err(err("EINVAL", "Animation is not supported")));
                }
            }
            _ => {
                reply(out, &command, command.id, command.number, command.pid, Err(err("EINVAL", "Unknown action")));
            }
        }
    }

    /// A complete (possibly reassembled) transmit / transmit+display / query.
    fn complete(&mut self, command: &Cmd, data: &[u8], ctx: &CursorCtx, out: &mut Vec<Action>) {
        let action = command.action_or_default();
        let place_pid = if action == b'T' { command.pid } else { 0 };
        if command.id != 0 && command.number != 0 {
            reply(out, command, command.id, command.number, place_pid, Err(err("EINVAL", "Cannot specify both i and I")));
            return;
        }
        let decoded = match loader::load(command, data, self.memory_limit) {
            Ok(d) => d,
            Err(e) => {
                reply(out, command, command.id, command.number, place_pid, Err(e));
                return;
            }
        };
        if action == b'q' {
            reply(out, command, command.id, command.number, 0, Ok(()));
            return;
        }
        let anonymous = command.id == 0 && command.number == 0;
        let id = if command.id != 0 { command.id } else { self.alloc_id() };
        if let Err(e) = self.store(id, command.number, anonymous, decoded) {
            reply(out, command, command.id, command.number, place_pid, Err(e));
            return;
        }
        let mut ok_pid = 0;
        if action == b'T' {
            let pid = if anonymous { 0 } else { command.pid };
            if let Err(e) = self.place(command, id, pid, ctx, out) {
                if anonymous {
                    // Nothing can ever refer to this image again.
                    self.remove_image(id);
                }
                reply(out, command, if anonymous { 0 } else { id }, command.number, pid, Err(e));
                return;
            }
            ok_pid = pid;
        }
        let shown_id = if anonymous { 0 } else { id };
        reply(out, command, shown_id, command.number, ok_pid, Ok(()));
    }

    fn put_cmd(&mut self, command: &Cmd, ctx: &CursorCtx, out: &mut Vec<Action>) {
        let fail = |out: &mut Vec<Action>, e: GErr| {
            reply(out, command, command.id, command.number, command.pid, Err(e));
        };
        if command.id != 0 && command.number != 0 {
            return fail(out, err("EINVAL", "Cannot specify both i and I"));
        }
        let id = if command.id != 0 {
            command.id
        } else if command.number != 0 {
            match self.newest_with_number(command.number) {
                Some(id) => id,
                None => return fail(out, err("ENOENT", "Image number not found")),
            }
        } else {
            return fail(out, err("ENOENT", "No image id or number given"));
        };
        if !self.images.contains_key(&id) {
            return fail(out, err("ENOENT", "Image not found"));
        }
        if let Err(e) = self.place(command, id, command.pid, ctx, out) {
            return fail(out, e);
        }
        let seq = self.bump();
        if let Some(m) = self.meta.get_mut(&id) {
            m.last_used = seq;
        }
        let shown = if command.number != 0 { id } else { command.id };
        reply(out, command, shown, command.number, command.pid, Ok(()));
    }

    fn newest_with_number(&self, number: u32) -> Option<u32> {
        self.images
            .values()
            .filter(|i| i.number == number)
            .max_by_key(|i| self.meta.get(&i.id).map_or(0, |m| m.seq))
            .map(|i| i.id)
    }

    fn bump(&mut self) -> u64 {
        let s = self.next_seq;
        self.next_seq += 1;
        s
    }

    fn alloc_id(&mut self) -> u32 {
        loop {
            self.next_auto_id = self.next_auto_id.checked_add(1).unwrap_or(FIRST_AUTO_ID);
            if !self.images.contains_key(&self.next_auto_id) {
                return self.next_auto_id;
            }
        }
    }

    // ---------------------------------------------------------------- storage

    fn store(&mut self, id: u32, number: u32, anonymous: bool, d: loader::Decoded) -> Result<(), GErr> {
        let size = d.rgba.len();
        if size > self.memory_limit {
            return Err(err("ENOSPC", "Image exceeds the storage quota"));
        }
        // Retransmitting an id wipes the old image and all its placements.
        self.remove_image(id);
        self.evict_for(size, id);
        let seq = self.bump();
        let version = self.next_version;
        self.next_version += 1;
        self.images.insert(
            id,
            Image {
                id,
                number,
                width: d.width,
                height: d.height,
                rgba: Arc::from(d.rgba),
                texture_version: version,
            },
        );
        self.meta.insert(id, Meta { seq, last_used: seq, anonymous });
        self.used += size;
        self.dirty.push(id);
        // A re-upload must not tell the renderer to drop the new texture.
        self.removed.retain(|&r| r != id);
        Ok(())
    }

    /// Evict images (never `keep`) until `extra` more bytes fit.
    fn evict_for(&mut self, extra: usize, keep: u32) {
        while self.used.saturating_add(extra) > self.memory_limit {
            let with_placements: std::collections::HashSet<u32> =
                self.placements.iter().map(|p| p.image_id).collect();
            let victim = self
                .meta
                .iter()
                .filter(|(&id, _)| id != keep)
                .min_by_key(|(id, m)| (with_placements.contains(id), m.last_used))
                .map(|(&id, _)| id);
            match victim {
                Some(v) => self.remove_image(v),
                None => break,
            }
        }
    }

    fn remove_image(&mut self, id: u32) {
        if let Some(img) = self.images.remove(&id) {
            self.used = self.used.saturating_sub(img.rgba.len());
            self.meta.remove(&id);
            self.dirty.retain(|&d| d != id);
            self.removed.push(id);
        }
        let before = self.placements.len();
        self.placements.retain(|p| p.image_id != id);
        if self.placements.len() != before {
            self.changed = true;
        }
    }

    fn remove_where(&mut self, free: bool, pred: impl Fn(&Placement) -> bool) {
        let mut affected = Vec::new();
        self.placements.retain(|p| {
            if pred(p) {
                affected.push(p.image_id);
                false
            } else {
                true
            }
        });
        if affected.is_empty() {
            return;
        }
        self.changed = true;
        affected.sort_unstable();
        affected.dedup();
        for id in affected {
            if self.placements.iter().any(|p| p.image_id == id) {
                continue;
            }
            let anon = self.meta.get(&id).is_some_and(|m| m.anonymous);
            if free || anon {
                self.remove_image(id);
            }
        }
    }

    // -------------------------------------------------------------- placement

    fn place(&mut self, c: &Cmd, image_id: u32, pid: u32, ctx: &CursorCtx, out: &mut Vec<Action>) -> Result<(), GErr> {
        let (iw, ih) = match self.images.get(&image_id) {
            Some(i) => (i.width, i.height),
            None => return Err(err("ENOENT", "Image not found")),
        };
        if c.parent_image != 0 || c.parent_placement != 0 {
            let parent_exists = self
                .placements
                .iter()
                .any(|p| p.image_id == c.parent_image && p.placement_id == c.parent_placement);
            return Err(if parent_exists {
                err("EINVAL", "Relative placements are not supported")
            } else {
                err("ENOPARENT", "Parent placement not found")
            });
        }
        if c.x >= iw || c.y >= ih {
            return Err(err("EINVAL", "Source rectangle is outside the image"));
        }
        let w = if c.w == 0 { iw - c.x } else { c.w.min(iw - c.x) };
        let h = if c.h == 0 { ih - c.y } else { c.h.min(ih - c.y) };
        let cw = if ctx.cell_w_px == 0 { FALLBACK_CELL_W } else { ctx.cell_w_px } as u64;
        let ch = if ctx.cell_h_px == 0 { FALLBACK_CELL_H } else { ctx.cell_h_px } as u64;
        let xoff = (c.xoff as u64).min(cw - 1);
        let yoff = (c.yoff as u64).min(ch - 1);
        let (sw, sh) = (w as u64, h as u64);
        let cap = u16::MAX as u64;
        let (cc, rr) = (c.cols as u64, c.rows as u64);
        let (cols, rows) = match (cc.min(cap), rr.min(cap)) {
            (0, 0) => ((sw + xoff).div_ceil(cw), (sh + yoff).div_ceil(ch)),
            (cc, 0) => (cc, (cc * cw * sh).div_ceil(sw * ch)),
            (0, rr) => ((rr * ch * sw).div_ceil(sh * cw), rr),
            (cc, rr) => (cc, rr),
        };
        let cols = cols.clamp(1, cap) as u32;
        let rows = rows.clamp(1, cap) as u32;
        let dest_px = {
            let (cc, rr) = (cc.min(cap), rr.min(cap));
            let (dw, dh) = match (cc, rr) {
                (0, 0) => (sw, sh),
                (cc, 0) => (cc * cw, (cc * cw * sh + sw / 2) / sw),
                (0, rr) => ((rr * ch * sw + sh / 2) / sh, rr * ch),
                (cc, rr) => (cc * cw, rr * ch),
            };
            (dw.clamp(1, u32::MAX as u64) as u32, dh.clamp(1, u32::MAX as u64) as u32)
        };
        let is_virtual = c.virt == 1;

        if pid != 0 {
            self.placements.retain(|p| !(p.image_id == image_id && p.placement_id == pid));
        }
        self.placements.push(Placement {
            image_id,
            placement_id: pid,
            screen: ctx.screen,
            col: if is_virtual { 0 } else { ctx.col },
            abs_row: if is_virtual { 0 } else { ctx.abs_row },
            z: c.z,
            cols,
            rows,
            src_rect: Rect { x: c.x, y: c.y, w, h },
            pixel_offset: (xoff as u32, yoff as u32),
            dest_px,
            is_virtual,
        });
        self.changed = true;

        if !is_virtual && c.cursor_policy != 1 {
            let max_dcols = (ctx.cols as i64 - 1 - ctx.col as i64).max(0);
            let dcols = (cols as i64).min(max_dcols) as i32;
            let drows = (rows - 1).min(i32::MAX as u32) as i32;
            if dcols != 0 || drows != 0 {
                out.push(Action::MoveCursor { dcols, drows });
            }
        }
        Ok(())
    }

    // ----------------------------------------------------------------- delete

    fn delete(&mut self, c: &Cmd, ctx: &CursorCtx, out: &mut Vec<Action>) {
        let d = c.delete.unwrap_or(b'a');
        let free = d.is_ascii_uppercase();
        let screen = ctx.screen;
        let top = self.viewport_top[screen.idx()]
            .unwrap_or_else(|| ctx.abs_row.saturating_sub(ctx.rows.saturating_sub(1) as u64));
        let bottom = top.saturating_add(ctx.rows.saturating_sub(1) as u64);
        let real = move |p: &Placement| !p.is_virtual && p.screen == screen;
        let cell_x = c.x.saturating_sub(1) as u64;
        let cell_y = top.saturating_add(c.y.saturating_sub(1) as u64);

        match d.to_ascii_lowercase() {
            b'a' => self.remove_where(free, |p| real(p) && p.abs_row <= bottom && p.last_row() >= top),
            b'i' => {
                if c.id == 0 {
                    return;
                }
                self.delete_image_placements(c.id, c.pid, free);
            }
            b'n' => {
                if let Some(id) = self.newest_with_number(c.number) {
                    self.delete_image_placements(id, c.pid, free);
                }
            }
            b'c' => {
                let (x, y) = (ctx.col as u64, ctx.abs_row);
                self.remove_where(free, |p| real(p) && p.covers_cell(x, y));
            }
            b'p' => self.remove_where(free, |p| real(p) && p.covers_cell(cell_x, cell_y)),
            b'q' => {
                let z = c.z;
                self.remove_where(free, |p| real(p) && p.z == z && p.covers_cell(cell_x, cell_y));
            }
            b'r' => {
                let (lo, hi) = (c.x, c.y);
                self.remove_where(free, |p| p.image_id >= lo && p.image_id <= hi);
                if free {
                    let ids: Vec<u32> = self.images.keys().copied().filter(|&i| i >= lo && i <= hi).collect();
                    for id in ids {
                        self.remove_image(id);
                    }
                }
            }
            b'x' => {
                self.remove_where(free, |p| {
                    real(p) && cell_x >= p.col as u64 && cell_x < p.col as u64 + p.cols as u64
                })
            }
            b'y' => self.remove_where(free, |p| real(p) && cell_y >= p.abs_row && cell_y <= p.last_row()),
            b'z' => {
                let z = c.z;
                self.remove_where(free, |p| real(p) && p.z == z);
            }
            b'f' => {} // animation frames are not implemented
            _ => reply(out, c, c.id, c.number, 0, Err(err("EINVAL", "Unknown delete specifier"))),
        }
    }

    fn delete_image_placements(&mut self, id: u32, pid: u32, free: bool) {
        self.remove_where(free, |p| p.image_id == id && (pid == 0 || p.placement_id == pid));
        // Uppercase without a placement id also frees data that had no placements.
        if free && pid == 0 {
            self.remove_image(id);
        }
    }
}

// ------------------------------------------------------------------ responses

fn sanitize(msg: &str) -> String {
    msg.chars().map(|c| if (' '..='~').contains(&c) { c } else { '?' }).collect()
}

/// Queue a response, honouring the quiet level and the "no id, no response"
/// rule. `i`/`num`/`pid` are the ids to echo (0 = omit).
fn reply(out: &mut Vec<Action>, cmd: &Cmd, i: u32, num: u32, pid: u32, res: Result<(), GErr>) {
    if i == 0 && num == 0 {
        return;
    }
    match (&res, cmd.quiet) {
        (Ok(_), 1..=2) | (Err(_), 2) => return,
        _ => {}
    }
    let mut s = String::from("\x1b_G");
    let mut first = true;
    let mut push = |s: &mut String, k: char, v: u32| {
        if v != 0 {
            if !first {
                s.push(',');
            }
            first = false;
            s.push(k);
            s.push('=');
            s.push_str(&v.to_string());
        }
    };
    push(&mut s, 'i', i);
    push(&mut s, 'I', num);
    push(&mut s, 'p', pid);
    s.push(';');
    match res {
        Ok(()) => s.push_str("OK"),
        Err(e) => {
            s.push_str(e.code);
            s.push(':');
            s.push_str(&sanitize(&e.msg));
        }
    }
    s.push_str("\x1b\\");
    out.push(Action::Respond(s.into_bytes()));
}

/// Answer an `a=q,e=X` feature query: echo `e` and every non-default key we
/// support for that action. Omitting `e` means "action not supported".
fn feature_reply(c: &Cmd, out: &mut Vec<Action>) {
    if c.id == 0 || c.quiet == 2 {
        return;
    }
    let e = c.feature.unwrap_or(0);
    let mut s = format!("\x1b_Gi={}", c.id);
    if matches!(e, b'T' | b't' | b'p' | b'd' | b'q') {
        s.push_str(&format!(",e={}", e as char));
        match e {
            b'T' | b't' | b'p' => {
                if e != b'p' {
                    if matches!(c.format, 24 | 100) {
                        s.push_str(&format!(",f={}", c.format));
                    }
                    if matches!(c.medium, b'f' | b't') {
                        s.push_str(&format!(",t={}", c.medium as char));
                    }
                    if c.compression == Some(b'z') {
                        s.push_str(",o=z");
                    }
                }
                if e != b't' {
                    for (k, v) in [
                        ('x', c.x),
                        ('y', c.y),
                        ('w', c.w),
                        ('h', c.h),
                        ('X', c.xoff),
                        ('Y', c.yoff),
                        ('c', c.cols),
                        ('r', c.rows),
                    ] {
                        if v != 0 {
                            s.push_str(&format!(",{k}={v}"));
                        }
                    }
                    if c.z != 0 {
                        s.push_str(&format!(",z={}", c.z));
                    }
                    if c.cursor_policy == 1 {
                        s.push_str(",C=1");
                    }
                    if c.virt == 1 {
                        s.push_str(",U=1");
                    }
                }
            }
            b'd' => {
                if let Some(d) = c.delete {
                    if "aAiInNcCpPqQrRxXyYzZ".contains(d as char) {
                        s.push_str(&format!(",d={}", d as char));
                    }
                }
            }
            _ => {}
        }
    }
    s.push_str(";OK\x1b\\");
    out.push(Action::Respond(s.into_bytes()));
}

#[cfg(test)]
mod tests;
