//! Pure skin geometry: 9-slice mapping, frame-space layout and hit testing.
//!
//! Nothing here touches winit or wgpu, so it is all unit-tested. All lengths
//! are physical pixels unless stated otherwise; `k` is the number of
//! destination pixels per source texel (the integer pixel scale, divided by
//! the image's own scale factor when a `@2x` image is used).

use super::parse::{EdgeMode, Insets, RectDef, SkinDef};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ButtonKind {
    Close,
    Minimize,
    Maximize,
    Shade,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeDir {
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// The terminal grid.
    Viewport,
    Titlebar,
    Button(ButtonKind),
    ResizeEdge(ResizeDir),
    /// Transparent: outside the window shape.
    Outside,
    /// Frame decoration that does nothing.
    Chrome,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PxRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Clamp an `i64` into `i32` (the pixel coordinate type).
pub fn sat_i32(v: i64) -> i32 {
    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

impl PxRect {
    /// Exclusive right edge; saturates instead of wrapping.
    fn right(&self) -> i64 {
        self.x as i64 + self.w as i64
    }

    /// Exclusive bottom edge; saturates instead of wrapping.
    fn bottom(&self) -> i64 {
        self.y as i64 + self.h as i64
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x as f64 && y >= self.y as f64 && x < self.right() as f64 && y < self.bottom() as f64
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    fn contains_px(&self, x: i32, y: i32) -> bool {
        let (x, y) = (x as i64, y as i64);
        x >= self.x as i64 && y >= self.y as i64 && x < self.right() && y < self.bottom()
    }

    fn intersect(&self, o: &PxRect) -> PxRect {
        let x0 = self.x.max(o.x) as i64;
        let y0 = self.y.max(o.y) as i64;
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        PxRect {
            x: sat_i32(x0),
            y: sat_i32(y0),
            w: sat_i32((x1 - x0).max(0)),
            h: sat_i32((y1 - y0).max(0)),
        }
    }
}

/// Resolve a frame-space rectangle (see [`RectDef`]) against a window of
/// `win_w` x `win_h` physical pixels at integer scale `s`. All arithmetic is
/// done in `i64` and the result saturates into `i32`, so no input can wrap.
pub fn resolve_rect(r: &RectDef, win_w: i32, win_h: i32, s: i32) -> PxRect {
    let s = s as i64;
    let axis = |pos: i32, len: i32, win: i32| {
        let (pos, len, win) = (pos as i64, len as i64, win as i64);
        let p = if pos < 0 { win + pos * s } else { pos * s };
        let l = match len {
            -1 => win - p,
            l if l < -1 => win + l * s - p,
            l => l * s,
        };
        (sat_i32(p), sat_i32(l.max(0)))
    };
    let (x, w) = axis(r.x, r.w, win_w);
    let (y, h) = axis(r.y, r.h, win_h);
    PxRect { x, y, w, h }
}

// ---------------------------------------------------------------------------
// 9-slice mapping
// ---------------------------------------------------------------------------

/// How a region of a texture is stretched over the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SliceSpec {
    /// x, y, w, h of the used region of the texture, in texels.
    pub region: [u32; 4],
    /// Fixed edge sizes in texels.
    pub caps: Insets,
    pub mode: EdgeMode,
}

/// One rectangle of the 9-slice: where it goes and which texels it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Patch {
    /// x, y, w, h in window pixels.
    pub dst: [i32; 4],
    /// x, y, w, h in texels (absolute in the texture).
    pub src: [u32; 4],
    /// Per axis: true = stretch to fit, false = tile (a cap is a tile of
    /// exactly its own size).
    pub stretch: [bool; 2],
    /// True for the centre patch (the terminal viewport).
    pub centre: bool,
}

/// Index into one axis of a 9-slice: returns the texel offset (relative to
/// the region start) shown at destination pixel `d` of `len`.
fn map_axis(d: i32, len: i32, cap_a: u32, cap_b: u32, tex_len: u32, k: i32, stretch: bool) -> u32 {
    let len = len.max(1);
    let d = d.clamp(0, len - 1);
    let k = k.max(1);
    let (pa, pb) = (cap_a as i32 * k, cap_b as i32 * k);
    let last = tex_len.saturating_sub(1);
    if len < pa + pb {
        // Too small for both caps: left half shows the start, right half the end.
        return if d * 2 < len {
            ((d / k) as u32).min(cap_a.saturating_sub(1)).min(last)
        } else {
            let e = (len - 1 - d) / k;
            last.saturating_sub((e as u32).min(cap_b.saturating_sub(1)))
        };
    }
    if d < pa {
        return (d / k) as u32;
    }
    if d >= len - pb {
        let e = (len - 1 - d) / k;
        return last.saturating_sub(e as u32);
    }
    let mid_tex = tex_len.saturating_sub(cap_a + cap_b);
    if mid_tex == 0 {
        return cap_a.min(last);
    }
    let local = d - pa;
    if stretch {
        let mid_dst = (len - pa - pb).max(1) as i64;
        cap_a + ((local as i64 * mid_tex as i64 / mid_dst) as u32).min(mid_tex - 1)
    } else {
        cap_a + (local as u32 / k as u32) % mid_tex
    }
}

/// Where the two top-edge runs of `EdgeMode::TileSplit` meet: the window
/// centre, moved right (by less than `k`) so the right run is a whole number
/// of texels and ends exactly on a texel boundary at the right cap.
fn split_at(len: i32, pa: i32, pb: i32, k: i32) -> i32 {
    let end = (len - pb).max(pa);
    let cx = (len / 2).clamp(pa, end);
    cx + (end - cx) % k.max(1)
}

/// `map_axis` for the horizontal axis of the top band under `TileSplit`.
fn map_axis_split(d: i32, len: i32, cap_a: u32, cap_b: u32, tex_len: u32, k: i32) -> u32 {
    let k = k.max(1);
    let (pa, pb) = (cap_a as i32 * k, cap_b as i32 * k);
    let tm = tex_len.saturating_sub(cap_a + cap_b);
    let d = d.clamp(0, (len - 1).max(0));
    if tm == 0 || len < pa + pb || d < pa || d >= len - pb {
        return map_axis(d, len, cap_a, cap_b, tex_len, k, false);
    }
    let cx = split_at(len, pa, pb, k);
    if d < cx {
        cap_a + ((d - pa) / k) as u32 % tm
    } else {
        // Counted back from the right cap, so the last texel of a tile sits
        // right next to the cap.
        let e = ((len - pb - 1 - d) / k) as u32;
        cap_a + tm - 1 - e % tm
    }
}

impl SliceSpec {
    fn stretch(&self) -> bool {
        self.mode == EdgeMode::Stretch
    }

    /// Whether window row `y` is in the top band that `TileSplit` lays out
    /// in two runs (only when both vertical caps fit).
    fn split_row(&self, win_h: i32, k: i32, y: i32) -> bool {
        let k = k.max(1);
        let c = self.caps;
        self.mode == EdgeMode::TileSplit && win_h >= (c.top + c.bottom) as i32 * k && y < c.top as i32 * k
    }

    /// Texel (absolute texture coordinates) shown at window pixel (x, y).
    pub fn texel_at(&self, win_w: i32, win_h: i32, k: i32, x: i32, y: i32) -> (u32, u32) {
        let [rx, ry, rw, rh] = self.region;
        let c = self.caps;
        let st = self.stretch();
        let tx = if self.split_row(win_h, k, y) {
            map_axis_split(x, win_w, c.left, c.right, rw, k)
        } else {
            map_axis(x, win_w, c.left, c.right, rw, k, st)
        };
        (rx + tx, ry + map_axis(y, win_h, c.top, c.bottom, rh, k, st))
    }

    /// The 3x3 patches (centre included, flagged) covering a window.
    pub fn patches(&self, win_w: i32, win_h: i32, k: i32) -> Vec<Patch> {
        let [rx, ry, rw, rh] = self.region;
        let c = self.caps;
        let k = k.max(1);
        let st = self.stretch();
        // (dst start, dst len, src start (relative), src len, stretch)
        let segs = |win: i32, ca: u32, cb: u32, tex: u32| -> [(i32, i32, u32, u32, bool); 3] {
            let mut pa = ca as i32 * k;
            let mut pb = cb as i32 * k;
            if pa + pb > win {
                pa = pa.min(win / 2);
                pb = (win - pa).min(pb).max(0);
            }
            let mid = (win - pa - pb).max(0);
            [
                (0, pa, 0, ca, false),
                (pa, mid, ca, tex.saturating_sub(ca + cb), st),
                (pa + mid, pb, tex.saturating_sub(cb), cb, false),
            ]
        };
        let xs = segs(win_w, c.left, c.right, rw);
        let ys = segs(win_h, c.top, c.bottom, rh);
        let mut out = Vec::with_capacity(9);
        for (j, y) in ys.iter().enumerate() {
            for (i, x) in xs.iter().enumerate() {
                if x.1 <= 0 || y.1 <= 0 || x.3 == 0 || y.3 == 0 {
                    continue;
                }
                if self.mode == EdgeMode::TileSplit && j == 0 && i == 1 && self.split_row(win_h, k, 0) {
                    // The left run tiles from the left cap; the right run is
                    // a partial tile followed by whole ones, so it ends on a
                    // tile boundary at the right cap (see `split_at`).
                    let (pa, end, tm) = (x.0, x.0 + x.1, x.3 as i32);
                    let cx = split_at(win_w, pa, win_w - end, k);
                    let mut run = |dx: i32, w: i32, t0: i32, tw: i32| {
                        if w > 0 && tw > 0 {
                            out.push(Patch {
                                dst: [dx, y.0, w, y.1],
                                src: [rx + x.2 + t0 as u32, ry + y.2, tw as u32, y.3],
                                stretch: [false, false],
                                centre: false,
                            });
                        }
                    };
                    let r0 = ((end - cx) / k) % tm;
                    run(pa, cx - pa, 0, tm);
                    run(cx, r0 * k, tm - r0, r0);
                    run(cx + r0 * k, end - cx - r0 * k, 0, tm);
                    continue;
                }
                out.push(Patch {
                    dst: [x.0, y.0, x.1, y.1],
                    src: [rx + x.2, ry + y.2, x.3, y.3],
                    stretch: [x.4, y.4],
                    centre: i == 1 && j == 1,
                });
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Layout and hit testing
// ---------------------------------------------------------------------------

/// Resolved positions of the interactive parts for one window size.
#[derive(Clone, Debug)]
pub struct Layout {
    pub win: (i32, i32),
    #[allow(dead_code)]
    pub scale: i32,
    #[allow(dead_code)]
    pub shaded: bool,
    /// Where the terminal grid draws (empty when shaded).
    pub viewport: PxRect,
    pub titlebar: Option<PxRect>,
    /// Where the window title is drawn (clipped to the bar when shaded).
    pub title_text: Option<PxRect>,
    pub buttons: Vec<(ButtonKind, PxRect)>,
    pub grip: Option<PxRect>,
    /// Resize zone width along the window edges (0 = not resizable).
    pub border: i32,
}

/// Rectangles written for the full-height titlebar do not fit a thin shade
/// bar: shrink/centre them vertically so they stay on it.
fn fit_in_bar(r: PxRect, bar_h: i32) -> PxRect {
    if r.bottom() <= bar_h as i64 {
        return r;
    }
    let h = r.h.min(bar_h);
    PxRect {
        x: r.x,
        y: ((bar_h - h) / 2).max(0),
        w: r.w,
        h,
    }
}

impl Layout {
    pub fn new(def: &SkinDef, win_w: u32, win_h: u32, scale: u32, shaded: bool, resizable: bool) -> Layout {
        // Window sizes are clamped well below i32::MAX by the app; saturate anyway.
        let (w, h, s) = (win_w.min(i32::MAX as u32) as i32, win_h.min(i32::MAX as u32) as i32, scale.clamp(1, 64) as i32);
        let vi = def.frame.viewport_inset;
        let (s64, w64, h64) = (s as i64, w as i64, h as i64);
        let viewport = if shaded {
            PxRect::default()
        } else {
            PxRect {
                x: sat_i32(vi.left as i64 * s64),
                y: sat_i32(vi.top as i64 * s64),
                w: sat_i32((w64 - (vi.left as i64 + vi.right as i64) * s64).max(0)),
                h: sat_i32((h64 - (vi.top as i64 + vi.bottom as i64) * s64).max(0)),
            }
        };
        let win_rect = PxRect { x: 0, y: 0, w, h };
        let adj = |r: PxRect| {
            let r = if shaded { fit_in_bar(r, h) } else { r };
            r.intersect(&win_rect)
        };
        let titlebar = def
            .titlebar
            .as_ref()
            .map(|t| adj(resolve_rect(&t.rect, w, h, s)))
            .filter(|r| r.w > 0 && r.h > 0);
        let title_text = def
            .titlebar
            .as_ref()
            .and_then(|t| t.text.as_ref())
            .map(|t| {
                // The line box is a little taller than the font size.
                let line = (t.font_px.clamp(0.0, 4096.0) * 1.25).ceil() as i32;
                adj(resolve_rect(&RectDef { x: t.x, y: t.y, w: t.w, h: line }, w, h, s))
            })
            .filter(|r| r.w > 0 && r.h > 0);
        let b = &def.buttons;
        let mut buttons = Vec::new();
        for (kind, def) in [
            (ButtonKind::Close, b.close),
            (ButtonKind::Minimize, b.minimize),
            (ButtonKind::Shade, b.shade),
            (ButtonKind::Maximize, b.maximize),
        ] {
            if let Some(d) = def {
                let r = adj(resolve_rect(&d.rect, w, h, s));
                if r.w > 0 && r.h > 0 {
                    buttons.push((kind, r));
                }
            }
        }
        let can_resize = resizable && !shaded;
        let grip = if can_resize {
            def.resize.grip.map(|g| resolve_rect(&g, w, h, s).intersect(&win_rect)).filter(|r| r.w > 0 && r.h > 0)
        } else {
            None
        };
        Layout {
            win: (w, h),
            scale: s,
            shaded,
            viewport,
            titlebar,
            title_text,
            buttons,
            grip,
            border: if can_resize { sat_i32(def.resize.border as i64 * s64) } else { 0 },
        }
    }

    pub fn button_rect(&self, kind: ButtonKind) -> Option<PxRect> {
        self.buttons.iter().find(|(k, _)| *k == kind).map(|(_, r)| *r)
    }

    /// What is under window pixel (x, y)? `inside` tells whether a pixel is
    /// part of the window shape (the frame's alpha mask); the viewport and
    /// buttons count as inside regardless.
    pub fn hit(&self, x: f64, y: f64, inside: impl Fn(i32, i32) -> bool) -> Hit {
        let (w, h) = self.win;
        if x < 0.0 || y < 0.0 || x >= w as f64 || y >= h as f64 {
            return Hit::Outside;
        }
        for (kind, r) in &self.buttons {
            if r.contains(x, y) {
                return Hit::Button(*kind);
            }
        }
        if self.viewport.contains(x, y) {
            return Hit::Viewport;
        }
        let (ix, iy) = (x.floor() as i32, y.floor() as i32);
        if !inside(ix, iy) {
            return Hit::Outside;
        }
        if let Some(g) = &self.grip {
            if g.contains(x, y) {
                return Hit::ResizeEdge(ResizeDir::SE);
            }
        }
        if self.border > 0 {
            if let Some(d) = edge_dir(ix, iy, w, h, self.border) {
                return Hit::ResizeEdge(d);
            }
        }
        if self.titlebar.is_some_and(|t| t.contains(x, y)) {
            return Hit::Titlebar;
        }
        Hit::Chrome
    }
}

/// Resize direction for a point within `b` pixels of the window edge.
/// Corner zones extend to `2b` along the edges.
fn edge_dir(x: i32, y: i32, w: i32, h: i32, b: i32) -> Option<ResizeDir> {
    let c = b * 2;
    let l = x < b;
    let r = x >= w - b;
    let t = y < b;
    let bo = y >= h - b;
    let lc = x < c;
    let rc = x >= w - c;
    let tc = y < c;
    let bc = y >= h - c;
    match () {
        _ if (l && tc) || (t && lc) => Some(ResizeDir::NW),
        _ if (r && tc) || (t && rc) => Some(ResizeDir::NE),
        _ if (l && bc) || (bo && lc) => Some(ResizeDir::SW),
        _ if (r && bc) || (bo && rc) => Some(ResizeDir::SE),
        _ if l => Some(ResizeDir::W),
        _ if r => Some(ResizeDir::E),
        _ if t => Some(ResizeDir::N),
        _ if bo => Some(ResizeDir::S),
        _ => None,
    }
}

/// Alpha threshold (0-255): pixels below it are outside the window.
pub const ALPHA_CUTOFF: u8 = 128;

/// Most rectangles `shape_rects` returns. A region of millions of rectangles
/// (a noisy alpha channel) costs seconds and tens of MB per `WM_SIZE`.
pub const MAX_SHAPE_RECTS: usize = 8192;

/// Axis-aligned rectangles (x0, y0, x1, y1; exclusive max) covering exactly
/// the pixels of the window shape, for `SetWindowRgn`. `alpha(tx, ty)` gives
/// the frame texel alpha; the viewport is always inside.
///
/// When the exact shape needs more than [`MAX_SHAPE_RECTS`] rectangles, falls
/// back to one span per row (from the first to the last inside pixel), which
/// is bounded by the window height and only ever adds pixels between runs.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn shape_rects(
    spec: &SliceSpec,
    win_w: i32,
    win_h: i32,
    k: i32,
    viewport: PxRect,
    alpha: impl Fn(u32, u32) -> u8,
) -> Vec<[i32; 4]> {
    if win_w <= 0 || win_h <= 0 {
        return Vec::new();
    }
    shape_rects_with(spec, win_w, win_h, k, viewport, &alpha, false)
        .or_else(|| shape_rects_with(spec, win_w, win_h, k, viewport, &alpha, true))
        .unwrap_or_default()
}

/// `coarse` = one span per row. Returns `None` when an exact result would
/// exceed [`MAX_SHAPE_RECTS`].
fn shape_rects_with(
    spec: &SliceSpec,
    win_w: i32,
    win_h: i32,
    k: i32,
    viewport: PxRect,
    alpha: &impl Fn(u32, u32) -> u8,
    coarse: bool,
) -> Option<Vec<[i32; 4]>> {
    use std::collections::HashMap;
    // The column mapping depends on the row only for the split top band.
    let cols_of = |y: i32| -> Vec<u32> { (0..win_w).map(|x| spec.texel_at(win_w, win_h, k, x, y).0).collect() };
    let (cols_top, cols) = (cols_of(0), cols_of(win_h - 1));
    let mut rects: Vec<[i32; 4]> = Vec::new();
    // Rectangles still growing downwards, by their (x0, x1) run -> index in `open`.
    let mut open: Vec<[i32; 4]> = Vec::new();
    for y in 0..win_h {
        let ty = spec.texel_at(win_w, win_h, k, 0, y).1;
        let mut runs: Vec<(i32, i32)> = Vec::new();
        let mut start: Option<i32> = None;
        for x in 0..win_w {
            let col = if spec.split_row(win_h, k, y) { &cols_top } else { &cols };
            let inside = viewport.contains_px(x, y) || alpha(col[x as usize], ty) >= ALPHA_CUTOFF;
            match (inside, start) {
                (true, None) => start = Some(x),
                (false, Some(s)) => {
                    runs.push((s, x));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            runs.push((s, win_w));
        }
        if coarse {
            if let (Some(&(first, _)), Some(&(_, last))) = (runs.first(), runs.last()) {
                runs = vec![(first, last)];
            }
        }
        // Extend open rects whose run repeats on this row; close the rest.
        let mut by_run: HashMap<(i32, i32), usize> = open.iter().enumerate().map(|(i, r)| ((r[0], r[2]), i)).collect();
        let mut taken = vec![false; open.len()];
        let mut next_open = Vec::with_capacity(runs.len());
        for (x0, x1) in runs {
            match by_run.remove(&(x0, x1)) {
                Some(i) => {
                    taken[i] = true;
                    let mut r = open[i];
                    r[3] = y + 1;
                    next_open.push(r);
                }
                None => next_open.push([x0, y, x1, y + 1]),
            }
        }
        for (i, r) in open.iter().enumerate() {
            if !taken[i] {
                rects.push(*r);
            }
        }
        open = next_open;
        if !coarse && rects.len() + open.len() > MAX_SHAPE_RECTS {
            return None;
        }
    }
    rects.append(&mut open);
    Some(rects)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skin::parse::SkinDef;

    #[test]
    fn noisy_alpha_cannot_produce_millions_of_rects() {
        // F17: a 1 px checkerboard gave 280k rects at 1080p, millions at 4K.
        let spec = SliceSpec {
            region: [0, 0, 64, 64],
            caps: Insets { left: 8, right: 8, top: 16, bottom: 8 },
            mode: EdgeMode::Tile,
        };
        let none = PxRect { x: 0, y: 0, w: 0, h: 0 };
        let checker = |x: u32, y: u32| if (x + y).is_multiple_of(2) { 255 } else { 0 };
        let t = std::time::Instant::now();
        let r = shape_rects(&spec, 3840, 2160, 1, none, checker);
        assert!(r.len() <= 2160 && r.len() <= MAX_SHAPE_RECTS, "{} rects", r.len());
        assert!(t.elapsed().as_secs() < 20, "{:?}", t.elapsed());
        // The fallback is a superset of the exact shape: every inside pixel is covered.
        let covers = |x: i32, y: i32| r.iter().any(|q| x >= q[0] && x < q[2] && y >= q[1] && y < q[3]);
        assert!(covers(0, 0) && covers(3838, 2158));
        // Ordinary shapes are exact and unchanged.
        let solid = shape_rects(&spec, 300, 200, 1, none, |_, _| 255);
        assert_eq!(solid, vec![[0, 0, 300, 200]]);
    }

    #[test]
    fn sat_i32_clamps() {
        assert_eq!(sat_i32(i64::MAX), i32::MAX);
        assert_eq!(sat_i32(i64::MIN), i32::MIN);
        assert_eq!(sat_i32(-5), -5);
    }

    #[test]
    fn pxrect_intersect_and_contains_saturate() {
        let big = PxRect { x: i32::MAX, y: i32::MAX, w: i32::MAX, h: i32::MAX };
        let win = PxRect { x: 0, y: 0, w: 800, h: 600 };
        let r = big.intersect(&win);
        assert_eq!((r.w, r.h), (0, 0));
        let wide = PxRect { x: i32::MIN, y: i32::MIN, w: i32::MAX, h: i32::MAX };
        let r = wide.intersect(&PxRect { x: i32::MAX - 1, y: 0, w: i32::MAX, h: 10 });
        assert_eq!(r.w, 0);
        assert!(!big.contains(5.0, 5.0));
        assert!(big.contains(i32::MAX as f64, i32::MAX as f64));
        assert!(!big.contains_px(0, 0));
        assert!(big.contains_px(i32::MAX, i32::MAX));
        let neg = PxRect { x: -10, y: -10, w: 20, h: 20 };
        assert_eq!(neg.intersect(&win), PxRect { x: 0, y: 0, w: 10, h: 10 });
    }

    #[test]
    fn resolve_rect_never_wraps() {
        let ext = [i32::MIN, i32::MIN + 1, -2, -1, 0, 1, i32::MAX - 1, i32::MAX];
        for &x in &ext {
            for &w in &ext {
                for s in [1, 2, 16] {
                    let r = resolve_rect(&RectDef { x, y: x, w, h: w }, 800, 600, s);
                    assert!(r.w >= 0 && r.h >= 0, "{x} {w} {s}: {r:?}");
                }
            }
        }
        // The documented forms still resolve as before.
        let r = resolve_rect(&RectDef { x: -22, y: 8, w: 14, h: 14 }, 300, 200, 2);
        assert_eq!(r, PxRect { x: 256, y: 16, w: 28, h: 28 });
        let r = resolve_rect(&RectDef { x: 10, y: 0, w: -1, h: -4 }, 300, 200, 1);
        assert_eq!(r, PxRect { x: 10, y: 0, w: 290, h: 196 });
        // i32::MIN width: a huge margin, so nothing is left.
        let r = resolve_rect(&RectDef { x: i32::MAX, y: 0, w: i32::MIN, h: 5 }, 800, 600, 1);
        assert_eq!(r.w, 0);
    }

    fn spec(mode: EdgeMode) -> SliceSpec {
        SliceSpec {
            region: [0, 0, 64, 64],
            caps: Insets { left: 8, right: 8, top: 16, bottom: 8 },
            mode,
        }
    }

    #[test]
    fn resolves_negative_rects() {
        let r = resolve_rect(&RectDef { x: -22, y: 8, w: 14, h: 14 }, 400, 300, 2);
        assert_eq!(r, PxRect { x: 400 - 44, y: 16, w: 28, h: 28 });
        let full = resolve_rect(&RectDef { x: 0, y: 0, w: -1, h: 36 }, 400, 300, 1);
        assert_eq!(full, PxRect { x: 0, y: 0, w: 400, h: 36 });
        let margin = resolve_rect(&RectDef { x: 40, y: 10, w: -120, h: 12 }, 400, 300, 1);
        assert_eq!((margin.x, margin.w), (40, 400 - 120 - 40));
        let grip = resolve_rect(&RectDef { x: -20, y: -20, w: 20, h: 20 }, 400, 300, 1);
        assert_eq!(grip, PxRect { x: 380, y: 280, w: 20, h: 20 });
    }

    #[test]
    fn caps_are_fixed_and_middle_tiles() {
        let s = spec(EdgeMode::Tile);
        // 1x, 200x100 window.
        assert_eq!(s.texel_at(200, 100, 1, 0, 0), (0, 0));
        assert_eq!(s.texel_at(200, 100, 1, 7, 15), (7, 15));
        assert_eq!(s.texel_at(200, 100, 1, 199, 99), (63, 63));
        assert_eq!(s.texel_at(200, 100, 1, 192, 92), (56, 56));
        // Middle repeats with the middle texel period (48).
        assert_eq!(s.texel_at(200, 100, 1, 8, 5).0, 8);
        assert_eq!(s.texel_at(200, 100, 1, 8 + 48, 5).0, 8);
        assert_eq!(s.texel_at(200, 100, 1, 8 + 49, 5).0, 9);
        // 2x: every texel covers 2 pixels.
        assert_eq!(s.texel_at(400, 200, 2, 15, 0).0, 7);
        assert_eq!(s.texel_at(400, 200, 2, 16, 0).0, 8);
        assert_eq!(s.texel_at(400, 200, 2, 400 - 1, 0).0, 63);
        assert_eq!(s.texel_at(400, 200, 2, 400 - 16, 0).0, 56);
    }

    #[test]
    fn tile_split_ends_runs_on_the_caps() {
        let s = spec(EdgeMode::TileSplit);
        // Middle texels are 8..56 (period 48). 1x, 203 px wide: the left run
        // starts at texel 8 right after the left cap...
        let (w, h) = (203, 100);
        assert_eq!(s.texel_at(w, h, 1, 8, 0).0, 8);
        assert_eq!(s.texel_at(w, h, 1, 9, 0).0, 9);
        // ...and the right run ends with the last middle texel next to the
        // right cap, whatever the width.
        assert_eq!(s.texel_at(w, h, 1, w - 8 - 1, 0).0, 55);
        assert_eq!(s.texel_at(w, h, 1, w - 8, 0).0, 56);
        assert_eq!(s.texel_at(w, h, 1, w - 9 - 47, 0).0, 8);
        // Rows below the top band keep the plain tiling from the left cap.
        assert_eq!(s.texel_at(w, h, 1, w - 8 - 1, 50).0, (w - 9 - 8) as u32 % 48 + 8);
        // Same at 2x with an odd width.
        let w2 = 411;
        assert_eq!(s.texel_at(w2, 200, 2, w2 - 16 - 1, 0).0, 55);
        assert_eq!(s.texel_at(w2, 200, 2, 16, 0).0, 8);
    }

    #[test]
    fn stretch_covers_the_middle_once() {
        let s = spec(EdgeMode::Stretch);
        assert_eq!(s.texel_at(200, 100, 1, 8, 5).0, 8);
        assert_eq!(s.texel_at(200, 100, 1, 191, 5).0, 8 + 47);
        let mid = s.texel_at(200, 100, 1, 100, 5).0;
        assert!((8..56).contains(&mid));
    }

    #[test]
    fn patches_tile_the_window_exactly() {
        for mode in [EdgeMode::Tile, EdgeMode::Stretch] {
            let s = spec(mode);
            let ps = s.patches(300, 200, 2);
            assert_eq!(ps.len(), 9);
            let area: i64 = ps.iter().map(|p| p.dst[2] as i64 * p.dst[3] as i64).sum();
            assert_eq!(area, 300 * 200);
            assert_eq!(ps.iter().filter(|p| p.centre).count(), 1);
            let tl = ps[0];
            assert_eq!(tl.dst, [0, 0, 16, 32]);
            assert_eq!(tl.src, [0, 0, 8, 16]);
            let br = ps[8];
            assert_eq!(br.dst, [300 - 16, 200 - 16, 16, 16]);
            assert_eq!(br.src, [56, 56, 8, 8]);
        }
        // A window smaller than the caps still yields sane patches.
        for mode in [EdgeMode::Tile, EdgeMode::TileSplit] {
            let tiny = spec(mode).patches(10, 10, 1);
            assert!(tiny.iter().all(|p| p.dst[2] > 0 && p.dst[3] > 0));
        }
        // The split top band still covers the window exactly, once.
        for (w, k) in [(300, 2), (131, 1), (97, 3), (64, 1), (65, 1)] {
            let ps = spec(EdgeMode::TileSplit).patches(w, 200, k);
            let area: i64 = ps.iter().map(|p| p.dst[2] as i64 * p.dst[3] as i64).sum();
            assert_eq!(area, w as i64 * 200, "width {w} @{k}x");
        }
    }

    /// The fragment shader's per-patch mapping (chrome.wgsl `axis`), done on
    /// the CPU: it must agree with the mask's `texel_at` everywhere.
    #[test]
    fn patch_mapping_matches_texel_at() {
        fn axis(l: i32, dst_len: i32, src_o: i32, src_len: i32, k: i32, stretch: bool) -> i32 {
            let sl = src_len.max(1);
            let ll = l.clamp(0, (dst_len - 1).max(0));
            if stretch {
                src_o + (ll * sl / dst_len.max(1)).min(sl - 1)
            } else {
                src_o + (ll / k.max(1)) % sl
            }
        }
        for (mode, w, h, k) in [EdgeMode::Tile, EdgeMode::Stretch, EdgeMode::TileSplit]
            .into_iter()
            .flat_map(|m| [1, 2, 3].map(|k| (m, k)))
            .flat_map(|(m, k)| [(131, 97), (200, 97), (97, 75), (73, 90)].map(move |(w, h)| (m, w, h, k)))
        {
            {
                let s = spec(mode);
                let patches = s.patches(w, h, k);
                for y in 0..h {
                    for x in 0..w {
                        let p = patches
                            .iter()
                            .find(|p| x >= p.dst[0] && x < p.dst[0] + p.dst[2] && y >= p.dst[1] && y < p.dst[1] + p.dst[3])
                            .expect("covered");
                        let tx = axis(x - p.dst[0], p.dst[2], p.src[0] as i32, p.src[2] as i32, k, p.stretch[0]);
                        let ty = axis(y - p.dst[1], p.dst[3], p.src[1] as i32, p.src[3] as i32, k, p.stretch[1]);
                        let (ex, ey) = s.texel_at(w, h, k, x, y);
                        assert_eq!((tx as u32, ty as u32), (ex, ey), "mode {mode:?} k {k} at {x},{y}");
                    }
                }
            }
        }
    }

    fn debug_def() -> SkinDef {
        SkinDef::from_toml(
            r#"
            [frame]
            slices = { left = 8, right = 8, top = 16, bottom = 8 }
            viewport_inset = { left = 8, right = 8, top = 16, bottom = 8 }
            [titlebar]
            rect = { x = 0, y = 0, w = -1, h = 16 }
            [buttons]
            close = { rect = { x = -14, y = 3, w = 10, h = 10 }, sprite = [0, 0] }
            shade = { rect = { x = -38, y = 3, w = 10, h = 10 }, sprite = [20, 0] }
            [resize]
            grip = { x = -12, y = -12, w = 12, h = 12 }
            border = 4
            "#,
        )
        .unwrap()
    }

    #[test]
    fn hit_priorities() {
        let l = Layout::new(&debug_def(), 400, 300, 1, false, true);
        let all = |_: i32, _: i32| true;
        assert_eq!(l.hit(200.0, 100.0, all), Hit::Viewport);
        assert_eq!(l.hit(200.0, 8.0, all), Hit::Titlebar);
        assert_eq!(l.hit(400.0 - 9.0, 8.0, all), Hit::Button(ButtonKind::Close));
        assert_eq!(l.hit(400.0 - 33.0, 8.0, all), Hit::Button(ButtonKind::Shade));
        assert_eq!(l.hit(395.0, 295.0, all), Hit::ResizeEdge(ResizeDir::SE), "grip");
        assert_eq!(l.hit(1.0, 150.0, all), Hit::ResizeEdge(ResizeDir::W));
        assert_eq!(l.hit(399.0, 150.0, all), Hit::ResizeEdge(ResizeDir::E));
        assert_eq!(l.hit(200.0, 299.0, all), Hit::ResizeEdge(ResizeDir::S));
        assert_eq!(l.hit(200.0, 1.0, all), Hit::ResizeEdge(ResizeDir::N), "edge beats titlebar");
        assert_eq!(l.hit(1.0, 1.0, all), Hit::ResizeEdge(ResizeDir::NW));
        assert_eq!(l.hit(398.0, 2.0, all), Hit::ResizeEdge(ResizeDir::NE));
        assert_eq!(l.hit(2.0, 298.0, all), Hit::ResizeEdge(ResizeDir::SW));
        assert_eq!(l.hit(5.0, 100.0, all), Hit::Chrome);
        assert_eq!(l.hit(-1.0, 5.0, all), Hit::Outside);
        assert_eq!(l.hit(400.0, 5.0, all), Hit::Outside);
        // Masked-out pixels are outside, but the viewport never is.
        let none = |_: i32, _: i32| false;
        assert_eq!(l.hit(5.0, 100.0, none), Hit::Outside);
        assert_eq!(l.hit(200.0, 100.0, none), Hit::Viewport);
    }

    #[test]
    fn scale_and_shade() {
        let l = Layout::new(&debug_def(), 800, 600, 2, false, true);
        assert_eq!(l.viewport, PxRect { x: 16, y: 32, w: 800 - 32, h: 600 - 48 });
        assert_eq!(l.border, 8);
        let all = |_: i32, _: i32| true;
        assert_eq!(l.hit(800.0 - 18.0, 16.0, all), Hit::Button(ButtonKind::Close));
        // Shaded: no viewport, no resizing, buttons fit the thin bar.
        let s = Layout::new(&debug_def(), 800, 24, 2, true, true);
        assert_eq!(s.viewport.w, 0);
        assert_eq!(s.border, 0);
        assert!(s.grip.is_none());
        let close = s.button_rect(ButtonKind::Close).unwrap();
        assert!(close.y + close.h <= 24 && close.h > 0);
        assert_eq!(s.hit(400.0, 12.0, all), Hit::Titlebar);
        // Not resizable (maximized): edges do nothing.
        let m = Layout::new(&debug_def(), 800, 600, 1, false, false);
        assert_eq!(m.hit(1.0, 300.0, all), Hit::Chrome);
    }

    #[test]
    fn shape_rects_match_the_mask() {
        // 10x10 texture, caps 2, with the top-left texel transparent.
        let spec = SliceSpec {
            region: [0, 0, 10, 10],
            caps: Insets { left: 2, right: 2, top: 2, bottom: 2 },
            mode: EdgeMode::Tile,
        };
        let alpha = |x: u32, y: u32| if x < 2 && y < 2 && x + y < 2 { 0u8 } else { 255 };
        let rects = shape_rects(&spec, 20, 12, 1, PxRect::default(), alpha);
        let covered = |x: i32, y: i32| rects.iter().any(|r| x >= r[0] && x < r[2] && y >= r[1] && y < r[3]);
        let mut area = 0;
        for r in &rects {
            area += (r[2] - r[0]) * (r[3] - r[1]);
        }
        assert_eq!(area, 20 * 12 - 3, "three transparent pixels (0,0) (1,0) (0,1)");
        assert!(!covered(0, 0) && !covered(1, 0) && !covered(0, 1));
        assert!(covered(1, 1) && covered(2, 0) && covered(19, 11));
        // Rows with identical runs merge into few rectangles.
        assert!(rects.len() <= 4, "{} rects", rects.len());
    }

    /// Bounding box of a rect list.
    fn bbox(rects: &[[i32; 4]]) -> [i32; 4] {
        rects.iter().fold([i32::MAX, i32::MAX, i32::MIN, i32::MIN], |b, r| {
            [b[0].min(r[0]), b[1].min(r[1]), b[2].max(r[2]), b[3].max(r[3])]
        })
    }

    #[test]
    fn shape_rects_reach_the_last_column_and_row() {
        let spec = SliceSpec {
            region: [0, 0, 10, 10],
            caps: Insets { left: 2, right: 2, top: 2, bottom: 2 },
            mode: EdgeMode::Tile,
        };
        // Opaque everywhere, and with a semi-transparent (alpha 180, above the
        // cutoff) last column and row such as a baked drop shadow: the region
        // spans the whole window (right / bottom are exclusive).
        for edge in [255u8, 180] {
            let alpha = move |x: u32, y: u32| if x == 9 || y == 9 { edge } else { 255 };
            for (w, h) in [(20, 12), (1008, 585), (1016, 597)] {
                let rects = shape_rects(&spec, w, h, 1, PxRect::default(), alpha);
                assert_eq!(bbox(&rects), [0, 0, w, h], "edge alpha {edge}, {w}x{h}");
            }
        }
        // A transparent last column / row (alpha below the cutoff) is really
        // outside the shape: the region is one pixel smaller on that side.
        let alpha = |x: u32, y: u32| if x == 9 || y == 9 { 0u8 } else { 255 };
        let rects = shape_rects(&spec, 1008, 585, 1, PxRect::default(), alpha);
        assert_eq!(bbox(&rects), [0, 0, 1007, 584]);
    }
}
