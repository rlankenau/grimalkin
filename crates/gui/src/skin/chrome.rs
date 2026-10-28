//! The chrome render pass: 9-sliced frame, viewport fill, button sprites and
//! the window title, drawn before the terminal into the same render pass.
//!
//! Frame and sprites are textured quads sampled with integer math (see
//! `chrome.wgsl`), so they stay pixel-crisp at the integer scale. The title
//! uses its own [`GlyphCache`] at the skin's font size; the renderer draws
//! the glyph quads this module produces with its mask pipeline.

use std::ops::Range;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};

use super::hit::{ButtonKind, Layout, Patch, SliceSpec};
use super::parse::Align;
use super::{Image, Skin, TexId};
use crate::glyphs::{GlyphCache, GlyphQuad, Gpu};

/// Per-frame UI state the chrome needs.
pub struct ChromeView<'a> {
    pub title: &'a str,
    pub shaded: bool,
    pub hover: Option<ButtonKind>,
    pub pressed: Option<ButtonKind>,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct ChromeInst {
    dst: [f32; 4],
    src: [f32; 4],
    /// stretch x, stretch y, texel-to-pixel factor, kind (0 texture, 1 solid)
    params: [u32; 4],
    color: [u8; 4],
}

const ATTRS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
    0 => Float32x4, 1 => Float32x4, 2 => Uint32x4, 3 => Unorm8x4
];

struct GpuTex {
    id: TexId,
    bind: wgpu::BindGroup,
    _texture: wgpu::Texture,
}

/// The window title as positioned glyphs (window pixels).
pub struct TitleDraw {
    pub quads: Vec<(f32, f32, GlyphQuad)>,
    pub color: (u8, u8, u8),
}

pub struct Chrome {
    skin: Arc<Skin>,
    pipeline: wgpu::RenderPipeline,
    texs: Vec<GpuTex>,
    buf: wgpu::Buffer,
    cap: u64,
    insts: Vec<ChromeInst>,
    batches: Vec<(TexId, Range<u32>)>,
    // Window title.
    title: GlyphCache,
    title_bind: wgpu::BindGroup,
    title_version: u32,
    title_px: f32,
    bind_layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    sampler: wgpu::Sampler,
}

fn upload_texture(device: &wgpu::Device, queue: &wgpu::Queue, img: &Image, label: &str) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: img.w,
            height: img.h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // Not the sRGB variant: the bytes are already display-encoded and
        // `textureLoad` must return them untouched.
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &img.rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(img.w * 4),
            rows_per_image: Some(img.h),
        },
        wgpu::Extent3d {
            width: img.w,
            height: img.h,
            depth_or_array_layers: 1,
        },
    );
    texture
}

fn make_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    globals: &wgpu::Buffer,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("chrome bind group"),
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
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

/// The frame patches to draw. The centre patch is the terminal viewport (the
/// grid draws there), so it is left out, except for a shade bar: that has no
/// viewport, and its middle band is real artwork.
fn frame_patches(spec: &SliceSpec, win: (u32, u32), k: i32, shaded: bool) -> Vec<Patch> {
    spec.patches(win.0 as i32, win.1 as i32, k)
        .into_iter()
        .filter(|p| shaded || !p.centre)
        .collect()
}

impl Chrome {
    /// `bind_layout`, `globals` and `sampler` are the renderer's shared
    /// (uniform, texture, sampler) layout, uniform buffer and nearest sampler.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        skin: Arc<Skin>,
        scale: u32,
        bind_layout: &wgpu::BindGroupLayout,
        globals: &wgpu::Buffer,
        sampler: &wgpu::Sampler,
        format: wgpu::TextureFormat,
        max_texture_dim: u32,
        families: &[String],
    ) -> Chrome {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chrome shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("chrome.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chrome pipeline layout"),
            bind_group_layouts: &[Some(bind_layout)],
            immediate_size: 0,
        });
        let premul = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("chrome"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_chrome"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<ChromeInst>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &ATTRS,
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
                entry_point: Some("fs_chrome"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState { color: premul, alpha: premul }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let mut texs = Vec::new();
        for id in [TexId::Frame1, TexId::Frame2, TexId::Shade, TexId::Sheet] {
            if let Some(img) = skin.image(id) {
                let texture = upload_texture(device, queue, img, "skin texture");
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                texs.push(GpuTex {
                    id,
                    bind: make_bind(device, bind_layout, globals, &view, sampler),
                    _texture: texture,
                });
            }
        }

        let font_px = skin
            .def
            .titlebar
            .as_ref()
            .and_then(|t| t.text.as_ref())
            .map(|t| t.font_px)
            .unwrap_or(11.0);
        let title_px = font_px * scale as f32;
        let gpu = Gpu { device, queue };
        let title = GlyphCache::new(&gpu, max_texture_dim, families, title_px, 1.0);
        let title_bind = make_bind(device, bind_layout, globals, &title.mask.view, sampler);
        let title_version = title.mask.version;
        let cap = 4096;
        Chrome {
            skin,
            pipeline,
            texs,
            buf: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("chrome instances"),
                size: cap,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            cap,
            insts: Vec::new(),
            batches: Vec::new(),
            title,
            title_bind,
            title_version,
            title_px,
            bind_layout: bind_layout.clone(),
            globals: globals.clone(),
            sampler: sampler.clone(),
        }
    }

    pub fn skin(&self) -> &Arc<Skin> {
        &self.skin
    }

    /// Bind group of the title glyph atlas (for the renderer's mask pipeline).
    pub fn title_bind(&self) -> &wgpu::BindGroup {
        &self.title_bind
    }

    fn push_batch(&mut self, tex: TexId, start: usize) {
        let end = self.insts.len();
        if end > start {
            self.batches.push((tex, start as u32..end as u32));
        }
    }

    /// Build this frame's instances for a window of `win` physical pixels at
    /// integer `scale`, upload them, and lay out the title. `fill` is the
    /// viewport background (straight RGBA).
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &ChromeView,
        win: (u32, u32),
        scale: u32,
        fill: [u8; 4],
    ) -> Option<TitleDraw> {
        let scale = scale.max(1);
        let skin = self.skin.clone();
        let layout = Layout::new(&skin.def, win.0, win.1, scale, view.shaded, true);
        self.insts.clear();
        self.batches.clear();

        // Frame.
        let (spec, tex, k) = skin.frame_spec(view.shaded, scale);
        let start = self.insts.len();
        for p in frame_patches(&spec, win, k, view.shaded) {
            self.insts.push(ChromeInst {
                dst: [p.dst[0] as f32, p.dst[1] as f32, p.dst[2] as f32, p.dst[3] as f32],
                src: [p.src[0] as f32, p.src[1] as f32, p.src[2] as f32, p.src[3] as f32],
                params: [p.stretch[0] as u32, p.stretch[1] as u32, k as u32, 0],
                color: [255; 4],
            });
        }
        self.push_batch(tex, start);

        // Viewport fill (under the terminal grid).
        let vp = layout.viewport;
        if !view.shaded && vp.w > 0 && vp.h > 0 && fill[3] > 0 {
            let start = self.insts.len();
            self.insts.push(ChromeInst {
                dst: [vp.x as f32, vp.y as f32, vp.w as f32, vp.h as f32],
                src: [0.0; 4],
                params: [0, 0, 1, 1],
                color: fill,
            });
            self.push_batch(tex, start);
        }

        // Button sprites: normal / hover / pressed stacked vertically.
        if let Some(sheet) = &skin.sheet {
            let start = self.insts.len();
            let b = &skin.def.buttons;
            for (kind, def) in [
                (ButtonKind::Close, b.close),
                (ButtonKind::Minimize, b.minimize),
                (ButtonKind::Shade, b.shade),
                (ButtonKind::Maximize, b.maximize),
            ] {
                let (Some(def), Some(r)) = (def, layout.button_rect(kind)) else { continue };
                let (w, h) = (def.rect.w.max(0) as u32, def.rect.h.max(0) as u32);
                if w == 0 || h == 0 {
                    continue;
                }
                let state = if view.pressed == Some(kind) && view.hover == Some(kind) {
                    2
                } else if view.hover == Some(kind) && view.pressed.is_none() {
                    1
                } else {
                    0
                };
                let mut state = state;
                // Sheets without hover/pressed rows fall back to normal.
                if def.sprite[1] + (state + 1) * h > sheet.h {
                    state = 0;
                }
                if def.sprite[0] + w > sheet.w || def.sprite[1] + h > sheet.h {
                    continue;
                }
                self.insts.push(ChromeInst {
                    dst: [r.x as f32, r.y as f32, r.w as f32, r.h as f32],
                    src: [def.sprite[0] as f32, (def.sprite[1] + state * h) as f32, w as f32, h as f32],
                    params: [1, 1, scale, 0],
                    color: [255; 4],
                });
            }
            self.push_batch(TexId::Sheet, start);
        }

        // Upload.
        if !self.insts.is_empty() {
            let bytes: &[u8] = bytemuck::cast_slice(&self.insts);
            if bytes.len() as u64 > self.cap {
                self.cap = (bytes.len() as u64).next_power_of_two();
                self.buf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("chrome instances"),
                    size: self.cap,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
            queue.write_buffer(&self.buf, 0, bytes);
        }

        self.layout_title(device, queue, view, &layout, scale)
    }

    fn layout_title(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &ChromeView,
        layout: &Layout,
        scale: u32,
    ) -> Option<TitleDraw> {
        let text = self.skin.def.titlebar.as_ref()?.text.clone()?;
        let rect = layout.title_text?;
        let want_px = text.font_px * scale as f32;
        if (want_px - self.title_px).abs() > f32::EPSILON {
            self.title.set_px_size(want_px);
            self.title_px = want_px;
        }
        let m = self.title.metrics;
        let cell_w = m.cell_w.max(1) as i32;
        let chars: Vec<char> = view.title.chars().filter(|c| !c.is_control()).collect();
        let max = (rect.w / cell_w).max(0) as usize;
        let shown: Vec<char> = if chars.len() <= max {
            chars
        } else if max > 3 {
            chars[..max - 3].iter().copied().chain("...".chars()).collect()
        } else {
            chars[..max].to_vec()
        };
        let tw = shown.len() as i32 * cell_w;
        let x0 = rect.x
            + match text.align {
                Align::Left => 0,
                Align::Center => ((rect.w - tw) / 2).max(0),
                Align::Right => (rect.w - tw).max(0),
            };
        let y0 = rect.y + ((rect.h - m.cell_h as i32) / 2).max(0);
        let gpu = Gpu { device, queue };
        let mut quads = Vec::new();
        for (i, ch) in shown.iter().enumerate() {
            if *ch == ' ' {
                continue;
            }
            let x = (x0 + i as i32 * cell_w) as f32;
            for q in self.title.lookup(&gpu, *ch, &[], 0, false) {
                quads.push((x, y0 as f32, *q));
            }
        }
        if self.title.overflowed {
            self.title.clear();
        }
        if self.title.mask.version != self.title_version {
            self.title_bind = make_bind(device, &self.bind_layout, &self.globals, &self.title.mask.view, &self.sampler);
            self.title_version = self.title.mask.version;
        }
        Some(TitleDraw {
            quads,
            color: text.color.tuple(),
        })
    }

    /// Draw the frame, viewport fill and button sprites.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.insts.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.buf.slice(..));
        for (id, range) in &self.batches {
            if let Some(t) = self.texs.iter().find(|t| t.id == *id) {
                pass.set_bind_group(0, &t.bind, &[]);
                pass.draw(0..4, range.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shade_bar_is_fully_covered_and_the_frame_keeps_a_hole() {
        for name in super::super::builtin_names() {
            let skin = Skin::load_from(name, None).unwrap();
            for scale in [1u32, 2] {
                let (spec, _, k) = skin.frame_spec(true, scale);
                let (w, h) = (700u32, skin.shade_height() * scale);
                let area: i64 = frame_patches(&spec, (w, h), k, true).iter().map(|p| p.dst[2] as i64 * p.dst[3] as i64).sum();
                assert_eq!(area, (w * h) as i64, "{name} @{scale}x: the shade bar must be drawn edge to edge");
                // The normal frame leaves the viewport to the terminal.
                let (spec, _, k) = skin.frame_spec(false, scale);
                let (w, h) = (900u32, 600u32);
                let frame: i64 = frame_patches(&spec, (w, h), k, false).iter().map(|p| p.dst[2] as i64 * p.dst[3] as i64).sum();
                let vp = Layout::new(&skin.def, w, h, scale, false, true).viewport;
                assert_eq!(frame, (w * h) as i64 - (vp.w * vp.h) as i64, "{name} @{scale}x");
            }
        }
    }

    #[test]
    fn instance_layout_matches_the_vertex_attributes() {
        assert_eq!(std::mem::size_of::<ChromeInst>(), 52);
        let sum: u64 = ATTRS.iter().map(|a| a.format.size()).sum();
        assert_eq!(sum, 52);
    }
}
