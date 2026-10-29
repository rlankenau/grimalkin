//! Font handling: shaping/fallback through cosmic-text, rasterization through
//! swash, and the growable GPU glyph atlases.
//!
//! The terminal grid is shaped one cluster at a time (a base character plus
//! its combining marks). Each distinct (cluster, bold, italic, wide) is shaped
//! and rasterized once, uploaded into an atlas, and afterwards is a hash
//! lookup. Mask glyphs live in an `R8Unorm` atlas and color glyphs (emoji) in
//! an `Rgba8Unorm` atlas with premultiplied alpha.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::fonts::Fonts;
use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, Metrics, Shaping, Style, SwashCache, SwashContent, Weight, Wrap,
};

/// Which atlas a glyph lives in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtlasKind {
    Mask,
    Color,
}

/// One rasterized glyph, positioned relative to the top-left of its cell.
#[derive(Clone, Copy, Debug)]
pub struct GlyphQuad {
    pub kind: AtlasKind,
    /// Rectangle in the atlas, in texels.
    pub ax: u16,
    pub ay: u16,
    pub aw: u16,
    pub ah: u16,
    /// Destination rectangle relative to the cell origin, in pixels.
    pub dx: f32,
    pub dy: f32,
    pub dw: f32,
    pub dh: f32,
}

/// Cell geometry derived from the primary font, in physical pixels.
#[derive(Clone, Copy, Debug)]
pub struct CellMetrics {
    pub cell_w: u32,
    pub cell_h: u32,
    /// Distance from the cell top to the baseline.
    pub baseline: i32,
    /// Underline top, relative to the cell top.
    pub underline_y: i32,
    pub underline_thickness: i32,
    /// Strikethrough top, relative to the cell top.
    pub strike_y: i32,
}

/// Most distinct (cluster, style) entries kept before the cache is emptied
/// and rebuilt from the screen (a stream of distinct clusters would otherwise
/// grow it without bound).
const MAX_ENTRIES: usize = 65536;

/// Colour emoji fonts, in order of preference.
const EMOJI_FAMILIES: [&str; 3] = ["Apple Color Emoji", "Segoe UI Emoji", "Noto Color Emoji"];

pub const STYLE_BOLD: u8 = 1;
pub const STYLE_ITALIC: u8 = 2;
const KEY_WIDE: u8 = 4;

pub struct Gpu<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
}

// ---------------------------------------------------------------------------
// Atlas
// ---------------------------------------------------------------------------

pub struct Atlas {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    format: wgpu::TextureFormat,
    bpp: u32,
    max_dim: u32,
    cur_x: u32,
    cur_y: u32,
    row_h: u32,
    /// Bumped whenever the texture object is replaced (bind groups must be
    /// rebuilt).
    pub version: u32,
}

impl Atlas {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat, bpp: u32, width: u32, height: u32, max_dim: u32) -> Atlas {
        let (texture, view) = Self::make_texture(device, format, width, height);
        Atlas {
            texture,
            view,
            width,
            height,
            format,
            bpp,
            max_dim,
            cur_x: 0,
            cur_y: 0,
            row_h: 0,
            version: 0,
        }
    }

    fn make_texture(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }

    /// Reserve a `w` x `h` rectangle (plus a 1 texel gutter). Grows the
    /// texture when full. Returns `None` once the maximum size is reached.
    fn alloc(&mut self, gpu: &Gpu, w: u32, h: u32) -> Option<(u32, u32)> {
        if w + 1 > self.width {
            return None;
        }
        loop {
            if self.cur_x + w + 1 > self.width {
                self.cur_y += self.row_h + 1;
                self.cur_x = 0;
                self.row_h = 0;
            }
            if self.cur_y + h < self.height {
                let pos = (self.cur_x, self.cur_y);
                self.cur_x += w + 1;
                self.row_h = self.row_h.max(h);
                return Some(pos);
            }
            if !self.grow(gpu) {
                return None;
            }
        }
    }

    fn grow(&mut self, gpu: &Gpu) -> bool {
        let new_h = (self.height * 2).min(self.max_dim);
        if new_h <= self.height {
            return false;
        }
        let (texture, view) = Self::make_texture(gpu.device, self.format, self.width, new_h);
        let mut enc = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("atlas grow") });
        enc.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit([enc.finish()]);
        log::debug!("glyph atlas grown to {}x{}", self.width, new_h);
        self.texture = texture;
        self.view = view;
        self.height = new_h;
        self.version = self.version.wrapping_add(1);
        true
    }

    fn upload(&self, gpu: &Gpu, x: u32, y: u32, w: u32, h: u32, data: &[u8]) {
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * self.bpp),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }

    fn reset(&mut self) {
        self.cur_x = 0;
        self.cur_y = 0;
        self.row_h = 0;
    }
}

// ---------------------------------------------------------------------------
// Glyph cache
// ---------------------------------------------------------------------------

pub struct GlyphCache {
    fonts: Fonts,
    swash: SwashCache,
    buffer: Buffer,
    families: Vec<String>,
    line_height: f32,
    px_size: f32,
    pub metrics: CellMetrics,
    simple: HashMap<(char, u8), u32>,
    complex: HashMap<(String, u8), u32>,
    entries: Vec<Vec<GlyphQuad>>,
    pub mask: Atlas,
    pub color: Atlas,
    /// Set when an atlas is full at its maximum size. The renderer reacts by
    /// calling [`GlyphCache::clear`] and regenerating all rows.
    pub overflowed: bool,
    /// Configured families that exist in the font database (cached: scanning
    /// every face for each shaped cluster dominated the cost of new glyphs).
    installed: Arc<Vec<String>>,
    /// Colour emoji family: `None` = not looked up yet, `Some(None)` = none installed.
    emoji: Option<Option<String>>,
    /// Per-frame limit on new glyph work, see [`GlyphCache::begin_frame`].
    deadline: Option<Instant>,
    built_this_frame: u32,
    /// Some glyph was left blank this frame because the budget ran out; the
    /// renderer must draw another frame to fill it in.
    pub deferred: bool,
    /// Scale glyphs wider than their cells down to fit (the skin title, where
    /// an over-wide glyph such as U+FDFD would overwrite its neighbours).
    pub fit_to_cells: bool,
}

struct ShapedGlyph {
    key: CacheKey,
    x: i32,
    y: i32,
    adv: f32,
}

impl GlyphCache {
    pub fn new(gpu: &Gpu, max_texture_dim: u32, families: &[String], px_size: f32, line_height: f32) -> GlyphCache {
        let mut fonts = Fonts::load(families);
        let mut buffer = Buffer::new(&mut fonts.fs, Metrics::new(px_size, (px_size * 1.2).ceil()));
        buffer.set_wrap(Wrap::None);
        buffer.set_size(None, None);
        let max_dim = max_texture_dim.min(8192);
        let mask = Atlas::new(gpu.device, wgpu::TextureFormat::R8Unorm, 1, 1024, 512, max_dim);
        let color = Atlas::new(gpu.device, wgpu::TextureFormat::Rgba8Unorm, 4, 1024, 512, max_dim);
        let mut gc = GlyphCache {
            fonts,
            swash: SwashCache::new(),
            buffer,
            families: families.to_vec(),
            line_height,
            px_size,
            metrics: CellMetrics {
                cell_w: 8,
                cell_h: 16,
                baseline: 12,
                underline_y: 14,
                underline_thickness: 1,
                strike_y: 8,
            },
            simple: HashMap::new(),
            complex: HashMap::new(),
            entries: Vec::new(),
            mask,
            color,
            overflowed: false,
            installed: Arc::new(Vec::new()),
            emoji: None,
            deadline: None,
            built_this_frame: 0,
            deferred: false,
            fit_to_cells: false,
        };
        gc.refresh_installed();
        gc.metrics = gc.compute_metrics();
        gc
    }

    /// Change size and/or families; clears all cached glyphs.
    pub fn reconfigure(&mut self, families: &[String], px_size: f32, line_height: f32) {
        self.families = families.to_vec();
        self.px_size = px_size;
        self.line_height = line_height;
        self.clear();
        self.refresh_installed();
        self.metrics = self.compute_metrics();
    }

    pub fn set_px_size(&mut self, px_size: f32) {
        let fam = self.families.clone();
        let lh = self.line_height;
        self.reconfigure(&fam, px_size, lh);
    }

    /// Start a frame. With a `budget`, glyphs not yet in the cache are only
    /// built until it is used up (always at least one, so every frame makes
    /// progress); the rest are left blank and [`GlyphCache::deferred`] is set.
    /// Shaping and rasterizing thousands of new glyphs (a screen of distinct
    /// CJK, or hostile combining sequences) would otherwise take seconds.
    pub fn begin_frame(&mut self, budget: Option<Duration>) {
        self.deadline = budget.map(|b| Instant::now() + b);
        self.built_this_frame = 0;
        self.deferred = false;
    }

    fn over_budget(&self) -> bool {
        self.built_this_frame > 0 && self.deadline.is_some_and(|d| Instant::now() >= d)
    }

    fn refresh_installed(&mut self) {
        self.installed = Arc::new(self.installed_families());
    }

    /// Drop every cached glyph and empty the atlases (their textures keep
    /// their current size).
    pub fn clear(&mut self) {
        self.simple.clear();
        self.complex.clear();
        self.entries.clear();
        self.mask.reset();
        self.color.reset();
        self.overflowed = false;
    }

    /// The family names that exist in the font database, in config order.
    fn installed_families(&self) -> Vec<String> {
        let db = self.fonts.fs.db();
        self.families
            .iter()
            .filter(|want| {
                db.faces()
                    .any(|f| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(want)))
            })
            .cloned()
            .collect()
    }

    fn compute_metrics(&mut self) -> CellMetrics {
        let installed = self.installed_families();
        if installed.is_empty() {
            log::warn!("none of the configured fonts {:?} is installed, using the system monospace font", self.families);
        }
        let fam = installed.first().cloned();
        let family = match &fam {
            Some(n) => Family::Name(n),
            None => Family::Monospace,
        };
        let shaped = self.shape_with("M", &Attrs::new().family(family));
        let (adv, font_id) = match shaped {
            Some((g, id)) if !g.is_empty() => (g[0].adv, id),
            _ => (self.px_size * 0.6, None),
        };
        let mut ascent = self.px_size * 0.8;
        let mut descent = self.px_size * 0.2;
        let mut underline_off = -self.px_size * 0.1;
        let mut strike_off = self.px_size * 0.3;
        let mut stroke = (self.px_size / 14.0).max(1.0);
        if let Some(id) = font_id {
            if let Some(font) = self.fonts.fs.get_font(id, Weight::NORMAL) {
                let m = font.as_swash().metrics(&[]).scale(self.px_size);
                ascent = m.ascent;
                descent = m.descent;
                underline_off = m.underline_offset;
                strike_off = m.strikeout_offset;
                if m.stroke_size > 0.0 {
                    stroke = m.stroke_size;
                }
            }
        }
        let natural = ascent + descent;
        let cell_h = ((natural * self.line_height).round() as i32).max(1);
        let cell_w = (adv.round() as i32).max(1);
        // Center the glyph box vertically when line_height != 1.
        let baseline = (ascent + (cell_h as f32 - natural) / 2.0).round() as i32;
        let thickness = (stroke.round() as i32).max(1);
        let mut underline_y = baseline - underline_off.round() as i32;
        underline_y = underline_y.clamp(baseline + 1, (cell_h - thickness).max(baseline + 1));
        if underline_y + thickness > cell_h {
            underline_y = (cell_h - thickness).max(0);
        }
        let strike_y = (baseline - strike_off.round() as i32 - thickness / 2).clamp(0, cell_h - 1);
        CellMetrics {
            cell_w: cell_w as u32,
            cell_h: cell_h as u32,
            baseline,
            underline_y,
            underline_thickness: thickness,
            strike_y,
        }
    }

    /// Shape `text` with `attrs`; returns glyphs plus the font id of the
    /// first glyph.
    fn shape_with(&mut self, text: &str, attrs: &Attrs) -> Option<(Vec<ShapedGlyph>, Option<cosmic_text::fontdb::ID>)> {
        self.buffer.set_metrics(Metrics::new(self.px_size, (self.px_size * 1.2).ceil()));
        self.buffer.set_text(text, attrs, Shaping::Advanced, None);
        self.buffer.shape_until_scroll(&mut self.fonts.fs, false);
        let mut out = Vec::new();
        let mut first = None;
        for run in self.buffer.layout_runs() {
            for g in run.glyphs {
                let p = g.physical((0.0, 0.0), 1.0);
                if first.is_none() {
                    first = Some(g.font_id);
                }
                out.push(ShapedGlyph {
                    key: p.cache_key,
                    x: p.x,
                    y: p.y,
                    adv: g.w,
                });
            }
        }
        Some((out, first))
    }

    /// The installed colour emoji family, if any (loads the full system font
    /// set the first time, since the quick start-up set may lack it).
    fn emoji_family(&mut self) -> Option<String> {
        if self.fonts.is_partial() {
            self.fonts.upgrade();
            self.refresh_installed();
            self.emoji = None;
        }
        if self.emoji.is_none() {
            let db = self.fonts.fs.db();
            let found = EMOJI_FAMILIES
                .iter()
                .find(|want| db.faces().any(|f| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(want))))
                .map(|s| (*s).to_string());
            self.emoji = Some(found);
        }
        self.emoji.clone().flatten()
    }

    /// Shape a cluster honoring the family fallback order: the first
    /// configured family that provides every glyph wins; otherwise the
    /// first family plus the system fallback.
    fn shape_cluster(&mut self, text: &str, style: u8) -> Vec<ShapedGlyph> {
        let weight = if style & STYLE_BOLD != 0 { Weight::BOLD } else { Weight::NORMAL };
        let fstyle = if style & STYLE_ITALIC != 0 { Style::Italic } else { Style::Normal };
        // U+FE0F asks for the emoji presentation. The primary (text) font often
        // has a plain glyph for the base character and would win, drawing a
        // monochrome symbol; use the colour emoji font instead. (U+FE0E, the
        // text selector, takes the normal path below.)
        if text.contains('\u{fe0f}') {
            if let Some(name) = self.emoji_family() {
                let attrs = Attrs::new().family(Family::Name(&name)).weight(weight).style(fstyle);
                if let Some((glyphs, _)) = self.shape_with(text, &attrs) {
                    if !glyphs.is_empty() && glyphs.iter().any(|g| g.key.glyph_id != 0) {
                        return glyphs;
                    }
                }
            }
        }
        let installed = self.installed.clone();
        let mut fallback: Option<Vec<ShapedGlyph>> = None;
        let partial = self.fonts.is_partial();
        for name in installed.iter() {
            let attrs = Attrs::new().family(Family::Name(name)).weight(weight).style(fstyle);
            let Some((glyphs, _)) = self.shape_with(text, &attrs) else { continue };
            if glyphs.is_empty() {
                continue;
            }
            let all_ok = glyphs.iter().all(|g| {
                g.key.glyph_id != 0
                    && self
                        .fonts
                        .fs
                        .db()
                        .face(g.key.font_id)
                        .map(|f| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)))
                        .unwrap_or(false)
            });
            if all_ok {
                return glyphs;
            }
            if fallback.is_none() {
                fallback = Some(glyphs);
            }
        }
        if partial {
            // A glyph is missing from every configured family: load the
            // complete system font set (emoji / CJK fallback) and retry.
            self.fonts.upgrade();
            self.refresh_installed();
            return self.shape_cluster(text, style);
        }
        if let Some(f) = fallback {
            return f;
        }
        let attrs = Attrs::new().family(Family::Monospace).weight(weight).style(fstyle);
        self.shape_with(text, &attrs).map(|(g, _)| g).unwrap_or_default()
    }

    /// Look up (rasterizing on first use) the glyphs for a cell's text.
    /// `wide` is true when the cell spans two columns.
    pub fn lookup(&mut self, gpu: &Gpu, ch: char, combining: &[char], style: u8, wide: bool) -> &[GlyphQuad] {
        let style = (style & 3) | if wide { KEY_WIDE } else { 0 };
        let idx = if combining.is_empty() {
            match self.simple.get(&(ch, style)) {
                Some(&i) => i,
                None => {
                    if self.over_budget() {
                        self.deferred = true;
                        return &[];
                    }
                    let mut s = String::new();
                    s.push(ch);
                    let i = self.build(gpu, &s, style);
                    self.simple.insert((ch, style), i);
                    i
                }
            }
        } else {
            let mut s = String::new();
            s.push(ch);
            s.extend(combining.iter());
            match self.complex.get(&(s.clone(), style)) {
                Some(&i) => i,
                None => {
                    if self.over_budget() {
                        self.deferred = true;
                        return &[];
                    }
                    let i = self.build(gpu, &s, style);
                    self.complex.insert((s, style), i);
                    i
                }
            }
        };
        &self.entries[idx as usize]
    }

    fn build(&mut self, gpu: &Gpu, text: &str, style: u8) -> u32 {
        self.built_this_frame += 1;
        if self.entries.len() >= MAX_ENTRIES {
            // Same recovery as a full atlas: the renderer clears the cache
            // and regenerates the visible rows.
            self.overflowed = true;
        }
        let wide = style & KEY_WIDE != 0;
        let ncells = if wide { 2.0 } else { 1.0 };
        let cell_w = self.metrics.cell_w as f32;
        let cell_h = self.metrics.cell_h as f32;
        let baseline = self.metrics.baseline as f32;
        let shaped = self.shape_cluster(text, style & 3);
        // Total advance of the cluster, used to center wide glyphs.
        let total_adv: f32 = shaped.iter().map(|g| g.adv).sum();
        let mut quads = Vec::new();
        for g in &shaped {
            if g.key.glyph_id == 0 && shaped.len() > 1 {
                continue;
            }
            let Some(img) = self.swash.get_image_uncached(&mut self.fonts.fs, g.key) else { continue };
            let pw = img.placement.width;
            let ph = img.placement.height;
            if pw == 0 || ph == 0 {
                continue;
            }
            let mut pw = pw;
            let mut ph = ph;
            let (kind, mut data): (AtlasKind, Vec<u8>) = match img.content {
                SwashContent::Mask => (AtlasKind::Mask, img.data.clone()),
                SwashContent::Color => {
                    let mut d = img.data.clone();
                    for px in d.as_chunks_mut::<4>().0 {
                        let a = px[3] as u32;
                        px[0] = ((px[0] as u32 * a + 127) / 255) as u8;
                        px[1] = ((px[1] as u32 * a + 127) / 255) as u8;
                        px[2] = ((px[2] as u32 * a + 127) / 255) as u8;
                    }
                    (AtlasKind::Color, d)
                }
                SwashContent::SubpixelMask => {
                    // Not requested (we ask swash for Format::Alpha); reduce
                    // to coverage if it ever shows up.
                    let d = img
                        .data
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|p| ((p[0] as u32 + p[1] as u32 + p[2] as u32) / 3) as u8)
                        .collect();
                    (AtlasKind::Mask, d)
                }
            };
            if kind == AtlasKind::Color {
                // Color bitmaps (emoji) are much larger than a cell. Shrink
                // them on the CPU with an area-aware filter; letting the GPU
                // sample the full-size bitmap with plain bilinear filtering
                // aliases badly. The data is premultiplied, which is the
                // right space to filter in.
                let box_w = self.metrics.cell_w as f32 * ncells;
                let s = (box_w / pw as f32).min(cell_h / ph as f32);
                if s < 1.0 && data.len() == pw as usize * ph as usize * 4 {
                    let (nw, nh) = (((pw as f32 * s).floor() as u32).max(1), ((ph as f32 * s).floor() as u32).max(1));
                    if let Some(src) = image::RgbaImage::from_raw(pw, ph, std::mem::take(&mut data)) {
                        data = image::imageops::resize(&src, nw, nh, image::imageops::FilterType::Triangle).into_raw();
                        pw = nw;
                        ph = nh;
                    }
                }
            }
            let atlas = match kind {
                AtlasKind::Mask => &mut self.mask,
                AtlasKind::Color => &mut self.color,
            };
            let Some((ax, ay)) = atlas.alloc(gpu, pw, ph) else {
                self.overflowed = true;
                continue;
            };
            let atlas = match kind {
                AtlasKind::Mask => &self.mask,
                AtlasKind::Color => &self.color,
            };
            atlas.upload(gpu, ax, ay, pw, ph, &data);

            let mut dx = (g.x + img.placement.left) as f32;
            let mut dy = baseline + (g.y - img.placement.top) as f32;
            let mut dw = pw as f32;
            let mut dh = ph as f32;
            match kind {
                AtlasKind::Color => {
                    // Fit color bitmaps (emoji) into the cell box, centered.
                    let box_w = cell_w * ncells;
                    let s = (box_w / dw).min(cell_h / dh).min(1.0);
                    dw *= s;
                    dh *= s;
                    dx = (box_w - dw) / 2.0;
                    dy = (cell_h - dh) / 2.0;
                }
                AtlasKind::Mask => {
                    if self.fit_to_cells && dw > cell_w * ncells {
                        let s = cell_w * ncells / dw;
                        dw *= s;
                        dh *= s;
                        dx = 0.0;
                        dy = baseline - (baseline - dy) * s;
                    }
                    if wide && total_adv > 0.0 && total_adv < cell_w * ncells {
                        dx += ((cell_w * ncells - total_adv) / 2.0).floor();
                    }
                }
            }
            quads.push(GlyphQuad {
                kind,
                ax: ax as u16,
                ay: ay as u16,
                aw: pw as u16,
                ah: ph as u16,
                dx,
                dy,
                dw,
                dh,
            });
        }
        self.entries.push(quads);
        (self.entries.len() - 1) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device without a window; `None` where there is no adapter at all.
    fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))
        .ok()?;
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("glyph test"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .ok()
    }

    #[test]
    fn new_glyph_work_per_frame_is_bounded() {
        // F16: thousands of new clusters in one frame took 0.5-0.8 s (release).
        let Some((device, queue)) = device() else { return };
        let gpu = Gpu { device: &device, queue: &queue };
        let mut gc = GlyphCache::new(&gpu, 4096, &["Menlo".to_string()], 16.0, 1.0);
        let marks = ['\u{300}', '\u{301}', '\u{302}', '\u{303}', '\u{304}', '\u{305}', '\u{306}', '\u{307}'];
        gc.begin_frame(Some(Duration::ZERO));
        for i in 0..200u32 {
            let ch = char::from_u32(0x4e00 + i).unwrap();
            let _ = gc.lookup(&gpu, ch, &marks, 0, true);
        }
        assert!(gc.deferred, "an exhausted budget defers new glyphs");
        assert_eq!(gc.entries.len(), 1, "exactly one glyph per frame once over budget");
        // Without a budget everything is built, and cached results are free.
        gc.begin_frame(None);
        for i in 0..200u32 {
            let ch = char::from_u32(0x4e00 + i).unwrap();
            let _ = gc.lookup(&gpu, ch, &marks, 0, true);
        }
        assert!(!gc.deferred);
        assert_eq!(gc.entries.len(), 200);
        gc.begin_frame(Some(Duration::ZERO));
        let _ = gc.lookup(&gpu, '\u{4e00}', &marks, 0, true);
        assert!(!gc.deferred, "cache hits never count against the budget");
    }

    #[test]
    fn vs16_selects_the_colour_emoji_and_vs15_stays_text() {
        // F41: U+2764 U+FE0F drew the text glyph, same as U+FE0E.
        let Some((device, queue)) = device() else { return };
        let gpu = Gpu { device: &device, queue: &queue };
        let mut gc = GlyphCache::new(&gpu, 4096, &["Menlo".to_string()], 16.0, 1.0);
        if gc.emoji_family().is_none() {
            eprintln!("vs16 test: no colour emoji font, skipped");
            return; // no colour emoji font on this machine
        }
        gc.begin_frame(None);
        let vs16 = gc.lookup(&gpu, '\u{2764}', &['\u{fe0f}'], 0, true).to_vec();
        assert!(vs16.iter().any(|q| q.kind == AtlasKind::Color), "VS16 must draw the colour emoji: {vs16:?}");
        let vs15 = gc.lookup(&gpu, '\u{2764}', &['\u{fe0e}'], 0, true).to_vec();
        assert!(vs15.iter().all(|q| q.kind == AtlasKind::Mask), "VS15 must stay a text glyph: {vs15:?}");
    }

    #[test]
    fn entries_are_capped() {
        let Some((device, queue)) = device() else { return };
        let gpu = Gpu { device: &device, queue: &queue };
        let mut gc = GlyphCache::new(&gpu, 4096, &["Menlo".to_string()], 16.0, 1.0);
        gc.begin_frame(None);
        gc.entries.resize(MAX_ENTRIES, Vec::new());
        let _ = gc.lookup(&gpu, 'x', &[], 0, false);
        assert!(gc.overflowed, "the renderer clears the cache when it flags overflow");
        gc.clear();
        assert!(gc.entries.is_empty());
    }
}
