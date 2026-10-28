//! Font handling: shaping/fallback through cosmic-text, rasterization through
//! swash, and the growable GPU glyph atlases.
//!
//! The terminal grid is shaped one cluster at a time (a base character plus
//! its combining marks). Each distinct (cluster, bold, italic, wide) is shaped
//! and rasterized once, uploaded into an atlas, and afterwards is a hash
//! lookup. Mask glyphs live in an `R8Unorm` atlas and color glyphs (emoji) in
//! an `Rgba8Unorm` atlas with premultiplied alpha.

use std::collections::HashMap;

use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FontSystem, Metrics, Shaping, Style, SwashCache, SwashContent, Weight, Wrap,
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
    fs: FontSystem,
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
}

struct ShapedGlyph {
    key: CacheKey,
    x: i32,
    y: i32,
    adv: f32,
}

impl GlyphCache {
    pub fn new(gpu: &Gpu, max_texture_dim: u32, families: &[String], px_size: f32, line_height: f32) -> GlyphCache {
        let mut fs = FontSystem::new();
        let mut buffer = Buffer::new(&mut fs, Metrics::new(px_size, (px_size * 1.2).ceil()));
        buffer.set_wrap(Wrap::None);
        buffer.set_size(None, None);
        let max_dim = max_texture_dim.min(8192);
        let mask = Atlas::new(gpu.device, wgpu::TextureFormat::R8Unorm, 1, 1024, 512, max_dim);
        let color = Atlas::new(gpu.device, wgpu::TextureFormat::Rgba8Unorm, 4, 1024, 512, max_dim);
        let mut gc = GlyphCache {
            fs,
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
        };
        gc.metrics = gc.compute_metrics();
        gc
    }

    /// Change size and/or families; clears all cached glyphs.
    pub fn reconfigure(&mut self, families: &[String], px_size: f32, line_height: f32) {
        self.families = families.to_vec();
        self.px_size = px_size;
        self.line_height = line_height;
        self.clear();
        self.metrics = self.compute_metrics();
    }

    pub fn set_px_size(&mut self, px_size: f32) {
        let fam = self.families.clone();
        let lh = self.line_height;
        self.reconfigure(&fam, px_size, lh);
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
        let db = self.fs.db();
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
            if let Some(font) = self.fs.get_font(id, Weight::NORMAL) {
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
        self.buffer.shape_until_scroll(&mut self.fs, false);
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

    /// Shape a cluster honoring the family fallback order: the first
    /// configured family that provides every glyph wins; otherwise the
    /// first family plus the system fallback.
    fn shape_cluster(&mut self, text: &str, style: u8) -> Vec<ShapedGlyph> {
        let weight = if style & STYLE_BOLD != 0 { Weight::BOLD } else { Weight::NORMAL };
        let fstyle = if style & STYLE_ITALIC != 0 { Style::Italic } else { Style::Normal };
        let installed = self.installed_families();
        let mut fallback: Option<Vec<ShapedGlyph>> = None;
        for name in &installed {
            let attrs = Attrs::new().family(Family::Name(name)).weight(weight).style(fstyle);
            let Some((glyphs, _)) = self.shape_with(text, &attrs) else { continue };
            if glyphs.is_empty() {
                continue;
            }
            let all_ok = glyphs.iter().all(|g| {
                g.key.glyph_id != 0
                    && self
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
                    let i = self.build(gpu, &s, style);
                    self.complex.insert((s, style), i);
                    i
                }
            }
        };
        &self.entries[idx as usize]
    }

    fn build(&mut self, gpu: &Gpu, text: &str, style: u8) -> u32 {
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
            let Some(img) = self.swash.get_image_uncached(&mut self.fs, g.key) else { continue };
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
                    for px in d.chunks_exact_mut(4) {
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
                        .chunks_exact(4)
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
