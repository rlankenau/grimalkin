//! wgpu renderer: surface/device setup, instanced quad pipelines, per-row
//! instance caching driven by vt damage.
//!
//! Draw order (one render pass):
//!   1. cell background rects
//!   2. (PASS 2 hook) kitty images with z < 0        -- graphics agent
//!   3. glyphs (mask atlas, then color atlas), then decorations
//!   4. (PASS 4 hook) kitty images with z >= 0       -- graphics agent
//!   5. cursor (rect, then the glyph under a block cursor in inverse color)
//!
//! Instances for the grid are cached per visible row and only regenerated for
//! rows reported by `Terminal::take_damage`. The cursor layer is rebuilt every
//! frame (it is tiny) so blinking never dirties the grid.
//!
//! Color handling: the surface uses the non-sRGB variant of the preferred
//! format and colors are blended in their encoded (sRGB) space, like other
//! terminals, with a luminance-dependent coverage curve so light-on-dark text
//! does not look thin. If the platform only offers an sRGB format the shader
//! converts to linear instead (`Globals::srgb_target`).

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use grimalkin_vt::{CellWidth, Damage, Flags, Terminal, Underline};
use winit::window::Window;

use crate::config::Config;
use crate::glyphs::{AtlasKind, GlyphCache, Gpu, STYLE_BOLD, STYLE_ITALIC};
use crate::selection::Selection;

// ---------------------------------------------------------------------------
// Instance types
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct RectInst {
    pos: [f32; 2],
    size: [f32; 2],
    color: [u8; 4],
    kind: u32,
}

const KIND_SOLID: u32 = 0;
const KIND_CURLY: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct GlyphInst {
    pos: [f32; 2],
    size: [f32; 2],
    uv: [u16; 4],
    color: [u8; 4],
    _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    viewport: [f32; 2],
    mask_size: [f32; 2],
    color_size: [f32; 2],
    text_gamma: f32,
    srgb_target: f32,
}

type Rgb = (u8, u8, u8);

pub(crate) fn rect(x: f32, y: f32, w: f32, h: f32, c: Rgb, a: u8, kind: u32) -> RectInst {
    RectInst {
        pos: [x, y],
        size: [w, h],
        color: [c.0, c.1, c.2, a],
        kind,
    }
}

/// Cached instances for one visible row.
#[derive(Default)]
struct RowInst {
    bg: Vec<RectInst>,
    mask: Vec<GlyphInst>,
    color: Vec<GlyphInst>,
    deco: Vec<RectInst>,
}

impl RowInst {
    fn clear(&mut self) {
        self.bg.clear();
        self.mask.clear();
        self.color.clear();
        self.deco.clear();
    }
}

/// A growable GPU vertex buffer holding instances.
struct InstanceBuf {
    buf: wgpu::Buffer,
    cap: u64,
    len: u32,
    label: &'static str,
}

impl InstanceBuf {
    fn new(device: &wgpu::Device, label: &'static str) -> Self {
        let cap = 4096;
        InstanceBuf {
            buf: Self::make(device, label, cap),
            cap,
            len: 0,
            label,
        }
    }

    fn make(device: &wgpu::Device, label: &'static str, cap: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: cap,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn upload<T: Pod>(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[T]) {
        self.len = data.len() as u32;
        if data.is_empty() {
            return;
        }
        let bytes: &[u8] = bytemuck::cast_slice(data);
        if bytes.len() as u64 > self.cap {
            self.cap = (bytes.len() as u64).next_power_of_two();
            self.buf = Self::make(device, self.label, self.cap);
        }
        queue.write_buffer(&self.buf, 0, bytes);
    }
}

/// Everything the renderer needs to know about the app/UI state for a frame.
pub struct ViewState<'a> {
    pub focused: bool,
    /// Blink phase: false hides the cursor this frame.
    pub cursor_on: bool,
    pub cursor_shape: grimalkin_vt::CursorShape,
    pub selection: &'a Selection,
    /// Link under the pointer while the link modifier is held (underlined).
    pub hover: Option<&'a crate::links::LinkSpan>,
    /// Visual bell strength, 0.0 (off) to 1.0.
    pub flash: f32,
}

#[derive(Debug)]
pub enum FrameStatus {
    Presented,
    /// The window is occluded or hidden; nothing was drawn. Redraw when the
    /// platform reports it visible again (`WindowEvent::Occluded(false)`).
    Occluded,
    /// The swapchain had no frame ready; retry shortly.
    Timeout,
    /// The surface was reconfigured; the caller should request another redraw.
    Retry,
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    /// Debug aid: when `GRIMALKIN_DUMP_FRAME=/path/file.png` is set, every
    /// presented frame is also written to that file (used for headless
    /// verification where screen capture is unavailable).
    dump_path: Option<std::path::PathBuf>,
    srgb_target: bool,
    premultiplied_clear: bool,

    rect_pipeline: wgpu::RenderPipeline,
    mask_pipeline: wgpu::RenderPipeline,
    color_pipeline: wgpu::RenderPipeline,
    globals_buf: wgpu::Buffer,
    bind_layout: wgpu::BindGroupLayout,
    nearest: wgpu::Sampler,
    linear: wgpu::Sampler,
    rect_bind: wgpu::BindGroup,
    mask_bind: wgpu::BindGroup,
    color_bind: wgpu::BindGroup,
    bound_versions: (u32, u32),

    glyphs: GlyphCache,

    // Layout (physical pixels).
    scale: f64,
    font_pt: f32,
    families: Vec<String>,
    line_height: f32,
    pad: u32,
    cols: usize,
    rows: usize,

    // Theme.
    sel_bg: Rgb,
    sel_fg: Rgb,
    opacity: f32,

    // Instance caches.
    row_cache: Vec<RowInst>,
    force_full: bool,
    all_bg: Vec<RectInst>,
    all_mask: Vec<GlyphInst>,
    all_color: Vec<GlyphInst>,
    all_deco: Vec<RectInst>,
    bg_buf: InstanceBuf,
    mask_buf: InstanceBuf,
    color_buf: InstanceBuf,
    deco_buf: InstanceBuf,
    cur_rect: Vec<RectInst>,
    cur_mask: Vec<GlyphInst>,
    cur_color: Vec<GlyphInst>,
    cur_rect_buf: InstanceBuf,
    cur_mask_buf: InstanceBuf,
    cur_color_buf: InstanceBuf,
}

const RECT_ATTRS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
    0 => Float32x2, 1 => Float32x2, 2 => Unorm8x4, 3 => Uint32
];
const GLYPH_ATTRS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
    0 => Float32x2, 1 => Float32x2, 2 => Uint16x4, 3 => Unorm8x4
];

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

impl Renderer {
    pub fn new(window: Arc<Window>, cfg: &Config) -> Result<Renderer, String> {
        let size = window.inner_size();
        let scale = window.scale_factor();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("cannot create surface: {e}"))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
            apply_limit_buckets: false,
        }))
        .map_err(|e| format!("no suitable GPU adapter: {e}"))?;
        let info = adapter.get_info();
        log::info!("GPU adapter: {} ({:?}, {:?})", info.name, info.device_type, info.backend);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("grimalkin"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| format!("cannot create device: {e}"))?;
        device.on_uncaptured_error(Arc::new(|e| log::error!("wgpu error: {e}")));

        let caps = surface.get_capabilities(&adapter);
        log::debug!("surface formats {:?} alpha {:?} present {:?}", caps.formats, caps.alpha_modes, caps.present_modes);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or("surface has no supported formats")?;
        let srgb_target = format.is_srgb();
        let transparent = cfg.window.opacity < 1.0;
        let (alpha_mode, premultiplied_clear) = if transparent {
            if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
                (wgpu::CompositeAlphaMode::PreMultiplied, true)
            } else if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::PostMultiplied) {
                (wgpu::CompositeAlphaMode::PostMultiplied, false)
            } else {
                log::warn!("surface does not support transparency");
                (wgpu::CompositeAlphaMode::Auto, true)
            }
        } else if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
            (wgpu::CompositeAlphaMode::Opaque, true)
        } else {
            (wgpu::CompositeAlphaMode::Auto, true)
        };
        let surface_config = wgpu::SurfaceConfiguration {
            usage: if std::env::var_os("GRIMALKIN_DUMP_FRAME").is_some()
                && caps.usages.contains(wgpu::TextureUsages::COPY_SRC)
            {
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC
            } else {
                wgpu::TextureUsages::RENDER_ATTACHMENT
            },
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);

        let font_pt = cfg.font.size;
        let px_size = font_pt * scale as f32;
        let glyphs = {
            let gpu = Gpu {
                device: &device,
                queue: &queue,
            };
            GlyphCache::new(
                &gpu,
                device.limits().max_texture_dimension_2d,
                &cfg.font.family,
                px_size,
                cfg.font.line_height,
            )
        };

        // Pipelines.
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("grimalkin shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bind layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let make_pipeline = |label: &str,
                             vs: &str,
                             fs: &str,
                             stride: u64,
                             attrs: &'static [wgpu::VertexAttribute]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: stride,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: attrs,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let rect_pipeline = make_pipeline(
            "rects",
            "vs_rect",
            "fs_rect",
            std::mem::size_of::<RectInst>() as u64,
            &RECT_ATTRS,
        );
        let mask_pipeline = make_pipeline(
            "mask glyphs",
            "vs_mask",
            "fs_mask",
            std::mem::size_of::<GlyphInst>() as u64,
            &GLYPH_ATTRS,
        );
        let color_pipeline = make_pipeline(
            "color glyphs",
            "vs_color",
            "fs_color",
            std::mem::size_of::<GlyphInst>() as u64,
            &GLYPH_ATTRS,
        );

        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = |filter| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("atlas sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            })
        };
        let nearest = sampler(wgpu::FilterMode::Nearest);
        let linear = sampler(wgpu::FilterMode::Linear);
        let (rect_bind, mask_bind, color_bind) = Self::make_binds(&device, &bind_layout, &globals_buf, &glyphs, &nearest, &linear);
        let bound_versions = (glyphs.mask.version, glyphs.color.version);

        let mut r = Renderer {
            surface,
            queue,
            surface_config,
            dump_path: std::env::var_os("GRIMALKIN_DUMP_FRAME").map(std::path::PathBuf::from),
            srgb_target,
            premultiplied_clear,
            rect_pipeline,
            mask_pipeline,
            color_pipeline,
            globals_buf,
            bind_layout,
            nearest,
            linear,
            rect_bind,
            mask_bind,
            color_bind,
            bound_versions,
            glyphs,
            scale,
            font_pt,
            families: cfg.font.family.clone(),
            line_height: cfg.font.line_height,
            pad: 0,
            cols: 1,
            rows: 1,
            sel_bg: cfg.colors.selection_bg.tuple(),
            sel_fg: cfg.colors.selection_fg.tuple(),
            opacity: cfg.window.opacity,
            row_cache: Vec::new(),
            force_full: true,
            all_bg: Vec::new(),
            all_mask: Vec::new(),
            all_color: Vec::new(),
            all_deco: Vec::new(),
            bg_buf: InstanceBuf::new(&device, "bg"),
            mask_buf: InstanceBuf::new(&device, "mask glyphs"),
            color_buf: InstanceBuf::new(&device, "color glyphs"),
            deco_buf: InstanceBuf::new(&device, "decorations"),
            cur_rect: Vec::new(),
            cur_mask: Vec::new(),
            cur_color: Vec::new(),
            cur_rect_buf: InstanceBuf::new(&device, "cursor rects"),
            cur_mask_buf: InstanceBuf::new(&device, "cursor mask"),
            cur_color_buf: InstanceBuf::new(&device, "cursor color"),
            device,
        };
        r.pad = (cfg.window.padding as f64 * scale).round() as u32;
        r.update_grid_dims();
        Ok(r)
    }

    fn make_binds(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        globals: &wgpu::Buffer,
        glyphs: &GlyphCache,
        nearest: &wgpu::Sampler,
        linear: &wgpu::Sampler,
    ) -> (wgpu::BindGroup, wgpu::BindGroup, wgpu::BindGroup) {
        let mk = |view: &wgpu::TextureView, samp: &wgpu::Sampler| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bind group"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: globals.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(samp),
                    },
                ],
            })
        };
        (
            mk(&glyphs.mask.view, nearest),
            mk(&glyphs.mask.view, nearest),
            mk(&glyphs.color.view, linear),
        )
    }

    // -- geometry ---------------------------------------------------------

    /// Cell size in physical pixels.
    pub fn cell_size_px(&self) -> (u32, u32) {
        (self.glyphs.metrics.cell_w, self.glyphs.metrics.cell_h)
    }

    pub fn padding_px(&self) -> u32 {
        self.pad
    }

    /// Grid dimensions that fit the current surface size.
    pub fn grid_size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    /// Grid dimensions that fit a surface of `w` x `h` physical pixels.
    pub fn grid_size_for(&self, w: u32, h: u32) -> (usize, usize) {
        let (cw, ch) = self.cell_size_px();
        let avail_w = w.saturating_sub(2 * self.pad);
        let avail_h = h.saturating_sub(2 * self.pad);
        (((avail_w / cw) as usize).max(1), ((avail_h / ch) as usize).max(1))
    }

    /// Surface size (physical pixels) needed for a `cols` x `rows` grid.
    pub fn surface_size_for(&self, cols: usize, rows: usize) -> (u32, u32) {
        let (cw, ch) = self.cell_size_px();
        (cols as u32 * cw + 2 * self.pad, rows as u32 * ch + 2 * self.pad)
    }

    fn update_grid_dims(&mut self) {
        let (c, r) = self.grid_size_for(self.surface_config.width, self.surface_config.height);
        self.cols = c;
        self.rows = r;
    }

    /// Window resized (physical pixels).
    pub fn resize(&mut self, w: u32, h: u32) {
        let (w, h) = (w.max(1), h.max(1));
        if w == self.surface_config.width && h == self.surface_config.height {
            return;
        }
        self.surface_config.width = w;
        self.surface_config.height = h;
        self.surface.configure(&self.device, &self.surface_config);
        self.update_grid_dims();
        self.force_full = true;
    }

    /// The display scale factor changed: re-rasterize at the new size.
    pub fn set_scale_factor(&mut self, scale: f64, padding_logical: f32) {
        self.scale = scale;
        self.pad = (padding_logical as f64 * scale).round() as u32;
        self.glyphs.set_px_size(self.font_pt * scale as f32);
        self.update_grid_dims();
        self.force_full = true;
    }

    /// Change the font size (points) and rebuild the atlas.
    pub fn set_font_size(&mut self, pt: f32) {
        self.font_pt = pt;
        let fam = self.families.clone();
        self.glyphs
            .reconfigure(&fam, pt * self.scale as f32, self.line_height);
        self.update_grid_dims();
        self.force_full = true;
    }

    /// Force every row to be regenerated on the next frame (selection or
    /// theme changed, etc.).
    pub fn invalidate(&mut self) {
        self.force_full = true;
    }

    // -- instance generation ---------------------------------------------

    fn update_rows(&mut self, term: &Terminal, damage: &Damage, view: &ViewState) {
        let rows = term.rows();
        if self.row_cache.len() != rows {
            self.row_cache.resize_with(rows, RowInst::default);
            self.force_full = true;
        }
        let full = self.force_full || matches!(damage, Damage::Full);
        let dirty: Vec<usize> = match damage {
            _ if full => (0..rows).collect(),
            Damage::Rows(v) => v.iter().copied().filter(|&r| r < rows).collect(),
            _ => Vec::new(),
        };
        if dirty.is_empty() && !full {
            return;
        }
        self.force_full = false;
        for &r in &dirty {
            self.build_row(term, r, view);
        }
        if self.glyphs.overflowed {
            // Atlas full at max size: start over with an empty atlas and
            // rebuild everything once.
            log::debug!("glyph atlas overflow, clearing");
            self.glyphs.clear();
            for r in 0..rows {
                self.row_cache[r].clear();
                self.build_row(term, r, view);
            }
            self.glyphs.overflowed = false;
        }
        // Concatenate the per-row caches and upload.
        self.all_bg.clear();
        self.all_mask.clear();
        self.all_color.clear();
        self.all_deco.clear();
        for row in &self.row_cache {
            self.all_bg.extend_from_slice(&row.bg);
            self.all_mask.extend_from_slice(&row.mask);
            self.all_color.extend_from_slice(&row.color);
            self.all_deco.extend_from_slice(&row.deco);
        }
        self.bg_buf.upload(&self.device, &self.queue, &self.all_bg);
        self.mask_buf.upload(&self.device, &self.queue, &self.all_mask);
        self.color_buf.upload(&self.device, &self.queue, &self.all_color);
        self.deco_buf.upload(&self.device, &self.queue, &self.all_deco);
    }

    fn build_row(&mut self, term: &Terminal, r: usize, view: &ViewState) {
        let mut out = std::mem::take(&mut self.row_cache[r]);
        out.clear();
        let m = self.glyphs.metrics;
        let (cw, chh) = (m.cell_w as f32, m.cell_h as f32);
        let y = self.pad as f32 + r as f32 * chh;
        let x0 = self.pad as f32;
        let colors = term.colors();
        let def_fg = colors.foreground;
        let def_bg = colors.background;
        let row = term.visible_row(r);
        let sel_active = view.selection.is_active();
        let abs_row = if sel_active { term.abs_row_of_visible_row(r) } else { 0 };
        let t = m.underline_thickness as f32;

        // Background run merging.
        let mut run: Option<(usize, Rgb)> = None;
        let cols = row.cells.len().min(term.cols());

        for c in 0..cols {
            let cell = &row.cells[c];
            let selected = sel_active && view.selection.contains(abs_row, c);
            let mut fg = colors.resolve(cell.fg, def_fg);
            let mut bg = colors.resolve(cell.bg, def_bg);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if selected {
                fg = self.sel_fg;
                bg = self.sel_bg;
            }
            if cell.flags.contains(Flags::DIM) && !selected {
                fg = mix(fg, bg, 1.0 / 3.0);
            }

            // Background.
            let draw_bg = bg != def_bg || selected;
            match (&mut run, draw_bg) {
                (Some((_, col)), true) if *col == bg => {}
                _ => {
                    if let Some((start, col)) = run.take() {
                        out.bg.push(rect(
                            x0 + start as f32 * cw,
                            y,
                            (c - start) as f32 * cw,
                            chh,
                            col,
                            255,
                            KIND_SOLID,
                        ));
                    }
                    if draw_bg {
                        run = Some((c, bg));
                    }
                }
            }

            let x = x0 + c as f32 * cw;

            // Glyph.
            if cell.width != CellWidth::WideSpacer
                && !cell.flags.contains(Flags::HIDDEN)
                && (cell.ch != ' ' && cell.ch != '\0' || !cell.combining.is_empty())
                && !crate::boxdraw::draw(cell.ch, x, y, cw, chh, fg, &mut out.deco)
            {
                let mut style = 0u8;
                if cell.flags.contains(Flags::BOLD) {
                    style |= STYLE_BOLD;
                }
                if cell.flags.contains(Flags::ITALIC) {
                    style |= STYLE_ITALIC;
                }
                let wide = cell.width == CellWidth::Wide;
                let gpu = Gpu {
                    device: &self.device,
                    queue: &self.queue,
                };
                let quads = self.glyphs.lookup(&gpu, cell.ch, &cell.combining, style, wide);
                push_glyphs(quads, x, y, fg, &mut out.mask, &mut out.color);
            }

            // Decorations.
            if cell.underline != Underline::None {
                let ul = match cell.underline_color {
                    grimalkin_vt::Color::Default => fg,
                    c => colors.resolve(c, fg),
                };
                emit_underline(&mut out.deco, cell.underline, x, y, cw, chh, &m, t, ul);
            }
            if cell.flags.contains(Flags::STRIKETHROUGH) {
                out.deco
                    .push(rect(x, y + m.strike_y as f32, cw, t, fg, 255, KIND_SOLID));
            }
        }
        if let Some((start, col)) = run.take() {
            out.bg.push(rect(
                x0 + start as f32 * cw,
                y,
                (cols - start) as f32 * cw,
                chh,
                col,
                255,
                KIND_SOLID,
            ));
        }
        self.row_cache[r] = out;
    }

    /// Per-frame overlays drawn on top of the cached rows: the visual bell
    /// flash, the hovered link underline and the "scrolled back" indicator.
    /// Appends to the cursor instance lists (rebuilt every frame).
    fn build_overlay(&mut self, term: &Terminal, view: &ViewState) {
        let m = self.glyphs.metrics;
        let (cw, chh) = (m.cell_w as f32, m.cell_h as f32);
        let colors = term.colors();
        let (fg, bg) = (colors.foreground, colors.background);
        let (vw, vh) = (self.surface_config.width as f32, self.surface_config.height as f32);

        if view.flash > 0.0 {
            let a = (view.flash.clamp(0.0, 1.0) * 70.0) as u8;
            self.cur_rect.push(rect(0.0, 0.0, vw, vh, fg, a, KIND_SOLID));
        }

        if let Some(link) = view.hover {
            let t = (m.underline_thickness as f32).max(1.0);
            for &(r, c0, c1) in &link.segments {
                if r >= term.rows() || c0 >= term.cols() {
                    continue;
                }
                let cell = &term.visible_row(r).cells[c0];
                let c = if cell.flags.contains(Flags::INVERSE) {
                    colors.resolve(cell.bg, bg)
                } else {
                    colors.resolve(cell.fg, fg)
                };
                let x = self.pad as f32 + c0 as f32 * cw;
                let y = self.pad as f32 + r as f32 * chh + m.underline_y as f32;
                self.cur_rect
                    .push(rect(x, y, (c1 - c0 + 1) as f32 * cw, t, c, 255, KIND_SOLID));
            }
        }

        let off = term.display_offset();
        if off > 0 {
            let text = format!("scrolled back {off} lines");
            let n = text.chars().count() as f32;
            let pad_x = (cw * 0.6).round();
            let w = n * cw + 2.0 * pad_x;
            let x = (vw - self.pad as f32 - w).max(0.0);
            let y = self.pad as f32;
            self.cur_rect
                .push(rect(x, y, w, chh, mix(bg, fg, 0.22), 235, KIND_SOLID));
            let gpu = Gpu {
                device: &self.device,
                queue: &self.queue,
            };
            for (i, ch) in text.chars().enumerate() {
                if ch == ' ' {
                    continue;
                }
                let quads = self.glyphs.lookup(&gpu, ch, &[], 0, false);
                push_glyphs(quads, x + pad_x + i as f32 * cw, y, fg, &mut self.cur_mask, &mut self.cur_color);
            }
        }
    }

    fn build_cursor(&mut self, term: &Terminal, view: &ViewState) {
        self.cur_rect.clear();
        self.cur_mask.clear();
        self.cur_color.clear();
        let cur = term.cursor();
        if !term.cursor_visible()
            || term.display_offset() > 0
            || cur.row >= term.rows()
            || cur.col >= term.cols()
            || !view.cursor_on && view.focused
        {
            return;
        }
        let m = self.glyphs.metrics;
        let (cw, chh) = (m.cell_w as f32, m.cell_h as f32);
        let colors = term.colors();
        let cursor_rgb = colors.cursor.unwrap_or(colors.foreground);
        let row = term.visible_row(cur.row);
        let cell = &row.cells[cur.col];
        let ncells = if cell.width == CellWidth::Wide { 2.0 } else { 1.0 };
        let x = self.pad as f32 + cur.col as f32 * cw;
        let y = self.pad as f32 + cur.row as f32 * chh;
        let w = cw * ncells;
        let thick = ((self.scale * 1.5).round() as f32).max(1.0);
        use grimalkin_vt::CursorShape as CS;
        if !view.focused {
            // Hollow block regardless of shape.
            let t = (self.scale.round() as f32).max(1.0);
            let c = cursor_rgb;
            self.cur_rect.push(rect(x, y, w, t, c, 255, KIND_SOLID));
            self.cur_rect.push(rect(x, y + chh - t, w, t, c, 255, KIND_SOLID));
            self.cur_rect.push(rect(x, y + t, t, chh - 2.0 * t, c, 255, KIND_SOLID));
            self.cur_rect.push(rect(x + w - t, y + t, t, chh - 2.0 * t, c, 255, KIND_SOLID));
            return;
        }
        match view.cursor_shape {
            CS::Block => {
                self.cur_rect.push(rect(x, y, w, chh, cursor_rgb, 255, KIND_SOLID));
                if cell.width != CellWidth::WideSpacer
                    && (cell.ch != ' ' && cell.ch != '\0' || !cell.combining.is_empty())
                    && !cell.flags.contains(Flags::HIDDEN)
                {
                    let mut style = 0u8;
                    if cell.flags.contains(Flags::BOLD) {
                        style |= STYLE_BOLD;
                    }
                    if cell.flags.contains(Flags::ITALIC) {
                        style |= STYLE_ITALIC;
                    }
                    let text_rgb = colors.background;
                    // Box-drawing glyphs go through the rect path.
                    if !crate::boxdraw::draw(cell.ch, x, y, cw, chh, text_rgb, &mut self.cur_rect) {
                        let gpu = Gpu {
                            device: &self.device,
                            queue: &self.queue,
                        };
                        let quads = self.glyphs.lookup(
                            &gpu,
                            cell.ch,
                            &cell.combining,
                            style,
                            cell.width == CellWidth::Wide,
                        );
                        push_glyphs(quads, x, y, text_rgb, &mut self.cur_mask, &mut self.cur_color);
                    }
                }
            }
            CS::Underline => {
                self.cur_rect
                    .push(rect(x, y + chh - thick, w, thick, cursor_rgb, 255, KIND_SOLID));
            }
            CS::Bar => {
                self.cur_rect.push(rect(x, y, thick, chh, cursor_rgb, 255, KIND_SOLID));
            }
        }
    }

    // -- frame ------------------------------------------------------------

    /// Update instances from the terminal's damage and draw one frame.
    pub fn render(&mut self, term: &mut Terminal, view: &ViewState) -> FrameStatus {
        let damage = term.take_damage();
        self.update_rows(term, &damage, view);
        self.build_cursor(term, view);
        self.build_overlay(term, view);
        {
            let dev = &self.device;
            let q = &self.queue;
            self.cur_rect_buf.upload(dev, q, &self.cur_rect);
            self.cur_mask_buf.upload(dev, q, &self.cur_mask);
            self.cur_color_buf.upload(dev, q, &self.cur_color);
        }

        // Rebuild bind groups if an atlas texture was replaced.
        let versions = (self.glyphs.mask.version, self.glyphs.color.version);
        if versions != self.bound_versions {
            let (a, b, c) = Self::make_binds(
                &self.device,
                &self.bind_layout,
                &self.globals_buf,
                &self.glyphs,
                &self.nearest,
                &self.linear,
            );
            self.rect_bind = a;
            self.mask_bind = b;
            self.color_bind = c;
            self.bound_versions = versions;
        }
        let g = Globals {
            viewport: [self.surface_config.width as f32, self.surface_config.height as f32],
            mask_size: [self.glyphs.mask.width as f32, self.glyphs.mask.height as f32],
            color_size: [self.glyphs.color.width as f32, self.glyphs.color.height as f32],
            text_gamma: 0.72,
            srgb_target: if self.srgb_target { 1.0 } else { 0.0 },
        };
        self.queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&g));

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => f,
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => {
                // Draw this frame, then reconfigure.
                self.draw(&f.texture, term);
                self.dump_frame(&f.texture);
                self.queue.present(f);
                self.surface.configure(&self.device, &self.surface_config);
                return FrameStatus::Presented;
            }
            wgpu::CurrentSurfaceTexture::Timeout => return FrameStatus::Timeout,
            wgpu::CurrentSurfaceTexture::Occluded => {
                if self.dump_path.is_some() {
                    // Headless verification: render offscreen so the frame
                    // dump works even when the window cannot be shown.
                    let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("offscreen frame"),
                        size: wgpu::Extent3d {
                            width: self.surface_config.width,
                            height: self.surface_config.height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: self.surface_config.format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                        view_formats: &[],
                    });
                    self.draw(&tex, term);
                    self.dump_frame(&tex);
                }
                return FrameStatus::Occluded;
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.surface_config);
                return FrameStatus::Retry;
            }
            other => {
                log::error!("cannot acquire frame: {other:?}");
                return FrameStatus::Timeout;
            }
        };
        self.draw(&frame.texture, term);
        self.dump_frame(&frame.texture);
        self.queue.present(frame);
        FrameStatus::Presented
    }

    /// Write the frame to `GRIMALKIN_DUMP_FRAME` as a PNG (debug aid).
    fn dump_frame(&self, frame: &wgpu::Texture) {
        let Some(path) = &self.dump_path else { return };
        let (w, h) = (self.surface_config.width, self.surface_config.height);
        let bpr = (w * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame dump"),
            size: (bpr * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame dump") });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: frame,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bpr),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([enc.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        if !matches!(rx.recv(), Ok(Ok(()))) {
            log::warn!("frame dump: map failed");
            return;
        }
        let Ok(data) = buf.slice(..).get_mapped_range() else { return };
        let bgra = matches!(
            self.surface_config.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let mut img = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h as usize {
            let row = &data[y * bpr as usize..y * bpr as usize + (w * 4) as usize];
            for px in row.chunks_exact(4) {
                if bgra {
                    img.extend_from_slice(&[px[2], px[1], px[0], 255]);
                } else {
                    img.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
            }
        }
        drop(data);
        buf.unmap();
        if let Err(e) = image::save_buffer(path, &img, w, h, image::ColorType::Rgba8) {
            log::warn!("frame dump failed: {e}");
        }
    }

    fn clear_color(&self, term: &Terminal) -> wgpu::Color {
        let (r, g, b) = term.colors().background;
        let a = self.opacity as f64;
        let conv = |v: u8| {
            let f = v as f64 / 255.0;
            if self.srgb_target {
                srgb_to_linear(f)
            } else {
                f
            }
        };
        let k = if self.premultiplied_clear { a } else { 1.0 };
        wgpu::Color {
            r: conv(r) * k,
            g: conv(g) * k,
            b: conv(b) * k,
            a,
        }
    }

    fn draw(&self, target: &wgpu::Texture, term: &Terminal) {
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terminal"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear_color(term)),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            // 1. backgrounds
            self.draw_rects(&mut pass, &self.bg_buf);
            // 2. PASS 2 hook: kitty images with z < 0 (graphics agent)
            // 3. glyphs then decorations
            self.draw_glyphs(&mut pass, &self.mask_pipeline, &self.mask_bind, &self.mask_buf);
            self.draw_glyphs(&mut pass, &self.color_pipeline, &self.color_bind, &self.color_buf);
            self.draw_rects(&mut pass, &self.deco_buf);
            // 4. PASS 4 hook: kitty images with z >= 0 (graphics agent)
            // 5. cursor
            self.draw_rects(&mut pass, &self.cur_rect_buf);
            self.draw_glyphs(&mut pass, &self.mask_pipeline, &self.mask_bind, &self.cur_mask_buf);
            self.draw_glyphs(&mut pass, &self.color_pipeline, &self.color_bind, &self.cur_color_buf);
        }
        self.queue.submit([enc.finish()]);
    }

    fn draw_rects(&self, pass: &mut wgpu::RenderPass<'_>, buf: &InstanceBuf) {
        if buf.len == 0 {
            return;
        }
        pass.set_pipeline(&self.rect_pipeline);
        pass.set_bind_group(0, &self.rect_bind, &[]);
        pass.set_vertex_buffer(0, buf.buf.slice(..));
        pass.draw(0..4, 0..buf.len);
    }

    fn draw_glyphs(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        pipeline: &wgpu::RenderPipeline,
        bind: &wgpu::BindGroup,
        buf: &InstanceBuf,
    ) {
        if buf.len == 0 {
            return;
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.set_vertex_buffer(0, buf.buf.slice(..));
        pass.draw(0..4, 0..buf.len);
    }
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (f(a.0, b.0), f(a.1, b.1), f(a.2, b.2))
}

fn push_glyphs(
    quads: &[crate::glyphs::GlyphQuad],
    x: f32,
    y: f32,
    fg: Rgb,
    mask: &mut Vec<GlyphInst>,
    color: &mut Vec<GlyphInst>,
) {
    for q in quads {
        let inst = GlyphInst {
            pos: [x + q.dx, y + q.dy],
            size: [q.dw, q.dh],
            uv: [q.ax, q.ay, q.aw, q.ah],
            color: [fg.0, fg.1, fg.2, 255],
            _pad: 0,
        };
        match q.kind {
            AtlasKind::Mask => mask.push(inst),
            AtlasKind::Color => color.push(inst),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_underline(
    out: &mut Vec<RectInst>,
    style: Underline,
    x: f32,
    y: f32,
    cw: f32,
    chh: f32,
    m: &crate::glyphs::CellMetrics,
    t: f32,
    c: Rgb,
) {
    let uy = m.underline_y as f32;
    match style {
        Underline::None => {}
        Underline::Single => out.push(rect(x, y + uy, cw, t, c, 255, KIND_SOLID)),
        Underline::Double => {
            // Keep both lines inside the cell.
            let mut top = uy - t;
            if top + 3.0 * t > chh {
                top = chh - 3.0 * t;
            }
            top = top.max(0.0);
            out.push(rect(x, y + top, cw, t, c, 255, KIND_SOLID));
            out.push(rect(x, y + top + 2.0 * t, cw, t, c, 255, KIND_SOLID));
        }
        Underline::Curly => {
            let h = 3.0 * t;
            let mut top = uy - t;
            if top + h > chh {
                top = chh - h;
            }
            top = top.max(0.0);
            out.push(rect(x, y + top, cw, h, c, 255, KIND_CURLY));
        }
        Underline::Dotted => {
            let step = (2.0 * t).max(2.0);
            let mut dx = 0.0;
            while dx + t <= cw + 0.01 {
                out.push(rect(x + dx, y + uy, t, t, c, 255, KIND_SOLID));
                dx += step;
            }
        }
        Underline::Dashed => {
            let dash = (cw * 0.75).floor().max(1.0);
            out.push(rect(x, y + uy, dash, t, c, 255, KIND_SOLID));
        }
    }
}
