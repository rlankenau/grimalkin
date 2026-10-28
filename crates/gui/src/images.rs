//! Kitty graphics rendering: GPU textures for the protocol's images and the
//! textured quads for the placements that are currently on screen.
//!
//! Per frame [`ImageRenderer::prepare`]:
//! 1. drops textures of removed images (`take_removed_images`) and of images
//!    whose pixels were replaced (`take_dirty_images`; the new texture is
//!    uploaded lazily, only if a visible placement needs it),
//! 2. asks the protocol for the placements intersecting the visible absolute
//!    rows, in draw order,
//! 3. lays them out in physical pixels and uploads one instance per placement.
//!
//! [`ImageRenderer::draw`] is then called for three layers (see [`Layer`]):
//! below cell backgrounds, between backgrounds and text, and above text.
//!
//! Layout follows the kitty spec: the image's top left is the top left of the
//! cell it was placed at plus the `X`/`Y` pixel offset; the source rectangle is
//! stretched to `Placement::dest_px` (native size without `c`/`r`, otherwise
//! `c` cells by `r` cells, deriving the missing axis from the aspect ratio).
//! Everything is clipped to the cell grid, so images never spill into the
//! padding or over rows that scrolled away.

use std::collections::HashMap;
use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use grimalkin_graphics::{Protocol, Screen as GScreen};
use grimalkin_vt::{Screen, Terminal};

/// Draw layers, in z order. The boundaries follow kitty: `z < -2^30` goes
/// under cells with a non-default background, other negatives under the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    BelowBackground = 0,
    BelowText = 1,
    AboveText = 2,
}

const BELOW_BG_Z: i32 = -(1 << 30);
/// Soft cap for GPU memory held by image textures (LRU-evicted above this).
const TEXTURE_BUDGET: u64 = 512 * 1024 * 1024;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct ImageInst {
    pos: [f32; 2],
    size: [f32; 2],
    uv0: [f32; 2],
    uv1: [f32; 2],
}

const IMAGE_ATTRS: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x2];

/// Where the cell grid sits in the surface, in physical pixels.
#[derive(Clone, Copy, Debug)]
pub struct GridGeometry {
    pub pad: u32,
    pub cell_w: u32,
    pub cell_h: u32,
    pub surface_w: u32,
    pub surface_h: u32,
}

struct GpuImage {
    version: u64,
    bind: wgpu::BindGroup,
    _texture: wgpu::Texture,
    bytes: u64,
    last_used: u64,
}

pub struct ImageRenderer {
    pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    bind_layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    sampler: wgpu::Sampler,
    max_dim: u32,

    textures: HashMap<u32, GpuImage>,
    texture_bytes: u64,
    frame: u64,

    buf: wgpu::Buffer,
    cap: u64,
    /// Image id of each uploaded instance.
    items: Vec<u32>,
    ranges: [Range<u32>; 3],
    /// Scissor rectangle (x, y, w, h) clipping images to the grid.
    clip: [u32; 4],
    surface: [u32; 2],
}

/// Pixel rectangle and texture coordinates for a placement, given the
/// absolute row shown at the top of the grid.
fn layout(p: &grimalkin_graphics::Placement, img_w: u32, img_h: u32, top: u64, geo: GridGeometry) -> ImageInst {
    let (iw, ih) = (img_w as f32, img_h as f32);
    let row = p.abs_row as i64 - top as i64;
    let x = geo.pad as f32 + p.col as f32 * geo.cell_w as f32 + p.pixel_offset.0 as f32;
    let y = geo.pad as f32 + row as f32 * geo.cell_h as f32 + p.pixel_offset.1 as f32;
    let r = p.src_rect;
    ImageInst {
        pos: [x, y],
        size: [p.dest_px.0 as f32, p.dest_px.1 as f32],
        uv0: [r.x as f32 / iw, r.y as f32 / ih],
        uv1: [(r.x + r.w) as f32 / iw, (r.y + r.h) as f32 / ih],
    }
}

impl ImageRenderer {
    /// `bind_layout` is the renderer's (uniform, texture, sampler) layout and
    /// `globals` its uniform buffer.
    pub fn new(
        device: &wgpu::Device,
        bind_layout: &wgpu::BindGroupLayout,
        globals: &wgpu::Buffer,
        format: wgpu::TextureFormat,
    ) -> ImageRenderer {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("image shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("image.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("image pipeline layout"),
            bind_group_layouts: &[Some(bind_layout)],
            immediate_size: 0,
        });
        let premul = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("images"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_image"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<ImageInst>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &IMAGE_ATTRS,
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
                entry_point: Some("fs_image"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState {
                        color: premul,
                        alpha: premul,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mip blit layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let blit_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mip blit pipeline layout"),
            bind_group_layouts: &[Some(&blit_layout)],
            immediate_size: 0,
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mip blit"),
            layout: Some(&blit_pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_blit"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_blit"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("image sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let cap = 4096;
        ImageRenderer {
            pipeline,
            blit_pipeline,
            blit_layout,
            bind_layout: bind_layout.clone(),
            globals: globals.clone(),
            sampler,
            max_dim: device.limits().max_texture_dimension_2d,
            textures: HashMap::new(),
            texture_bytes: 0,
            frame: 0,
            buf: Self::make_buf(device, cap),
            cap,
            items: Vec::new(),
            ranges: [0..0, 0..0, 0..0],
            clip: [0, 0, 1, 1],
            surface: [1, 1],
        }
    }

    fn make_buf(device: &wgpu::Device, cap: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("image instances"),
            size: cap,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Number of image textures currently resident on the GPU.
    #[allow(dead_code)]
    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    /// Update textures and instances for the rows visible in `term`.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        protocol: &mut Protocol,
        term: &Terminal,
        geo: GridGeometry,
    ) {
        self.frame += 1;
        for id in protocol.take_removed_images() {
            if let Some(t) = self.textures.remove(&id) {
                self.texture_bytes -= t.bytes;
            }
        }
        // Replaced pixels: forget the old texture, upload lazily when needed.
        for id in protocol.take_dirty_images() {
            if let Some(t) = self.textures.remove(&id) {
                self.texture_bytes -= t.bytes;
            }
        }

        let (rows, cols) = (term.rows() as u32, term.cols() as u32);
        self.surface = [geo.surface_w.max(1), geo.surface_h.max(1)];
        let cx = geo.pad.min(self.surface[0] - 1);
        let cy = geo.pad.min(self.surface[1] - 1);
        self.clip = [
            cx,
            cy,
            (cols * geo.cell_w).min(self.surface[0] - cx).max(1),
            (rows * geo.cell_h).min(self.surface[1] - cy).max(1),
        ];

        let top = term.abs_row_of_visible_row(0);
        let bottom = top + rows.saturating_sub(1) as u64;
        let screen = match term.screen() {
            Screen::Main => GScreen::Main,
            Screen::Alt => GScreen::Alt,
        };
        // (layer, image id, instance) in draw order.
        let mut staged: Vec<(Layer, u32, ImageInst)> = Vec::new();
        let placements: Vec<_> = protocol
            .placements_in_draw_order(screen, top..=bottom)
            .into_iter()
            .cloned()
            .collect();
        for p in &placements {
            let Some(img) = protocol.image(p.image_id) else { continue };
            let Some(version) = self.ensure_texture(device, queue, protocol, p.image_id) else {
                continue;
            };
            debug_assert!(version == img.texture_version);
            let inst = layout(p, img.width, img.height, top, geo);
            let layer = if p.z < BELOW_BG_Z {
                Layer::BelowBackground
            } else if p.z < 0 {
                Layer::BelowText
            } else {
                Layer::AboveText
            };
            if let Some(t) = self.textures.get_mut(&p.image_id) {
                t.last_used = self.frame;
            }
            staged.push((layer, p.image_id, inst));
        }
        staged.sort_by_key(|(l, _, _)| *l as u8); // stable: keeps z order within a layer
        self.items.clear();
        let mut insts: Vec<ImageInst> = Vec::with_capacity(staged.len());
        let mut starts = [0u32; 4];
        for (l, id, inst) in &staged {
            starts[*l as usize + 1] += 1;
            self.items.push(*id);
            insts.push(*inst);
        }
        for i in 1..4 {
            starts[i] += starts[i - 1];
        }
        self.ranges = [starts[0]..starts[1], starts[1]..starts[2], starts[2]..starts[3]];

        if !insts.is_empty() {
            let bytes: &[u8] = bytemuck::cast_slice(&insts);
            if bytes.len() as u64 > self.cap {
                self.cap = (bytes.len() as u64).next_power_of_two();
                self.buf = Self::make_buf(device, self.cap);
            }
            queue.write_buffer(&self.buf, 0, bytes);
        }
        self.evict();
    }

    /// Make sure `id` has an up-to-date texture. Returns its version.
    fn ensure_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        protocol: &Protocol,
        id: u32,
    ) -> Option<u64> {
        let img = protocol.image(id)?;
        if let Some(t) = self.textures.get(&id) {
            if t.version == img.texture_version {
                return Some(t.version);
            }
            let old = self.textures.remove(&id)?;
            self.texture_bytes -= old.bytes;
        }
        let (w, h) = (img.width, img.height);
        if w == 0 || h == 0 || w > self.max_dim || h > self.max_dim || img.rgba.len() < w as usize * h as usize * 4 {
            log::warn!("image {id} ({w}x{h}) cannot be displayed (GPU limit {})", self.max_dim);
            return None;
        }
        let started = std::time::Instant::now();
        let mips = 32 - w.max(h).leading_zeros();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("kitty image"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &img.rgba[..w as usize * h as usize * 4],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        if mips > 1 {
            self.generate_mips(device, queue, &texture, mips);
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("image bind group"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        log::debug!("uploaded image {id} ({w}x{h}, {mips} mips) in {:?}", started.elapsed());
        let bytes = (w as u64 * h as u64 * 4) * 4 / 3;
        self.texture_bytes += bytes;
        self.textures.insert(
            id,
            GpuImage {
                version: img.texture_version,
                bind,
                _texture: texture,
                bytes,
                last_used: self.frame,
            },
        );
        Some(img.texture_version)
    }

    /// Fill mip levels 1.. by repeatedly halving the previous level on the GPU.
    fn generate_mips(&self, device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, mips: u32) {
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("image mips"),
        });
        for level in 1..mips {
            let view_of = |l: u32| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    base_mip_level: l,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            };
            let src = view_of(level - 1);
            let dst = view_of(level);
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mip blit bind"),
                layout: &self.blit_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&src),
                }],
            });
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mip blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &dst,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.blit_pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([enc.finish()]);
    }

    /// Drop least recently used textures when over budget (never ones used by
    /// the frame being prepared).
    fn evict(&mut self) {
        if self.texture_bytes <= TEXTURE_BUDGET {
            return;
        }
        let mut idle: Vec<(u64, u32)> = self
            .textures
            .iter()
            .filter(|(_, t)| t.last_used != self.frame)
            .map(|(&id, t)| (t.last_used, id))
            .collect();
        idle.sort_unstable();
        for (_, id) in idle {
            if self.texture_bytes <= TEXTURE_BUDGET {
                break;
            }
            if let Some(t) = self.textures.remove(&id) {
                self.texture_bytes -= t.bytes;
            }
        }
    }

    /// Draw one layer into an open render pass.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, layer: Layer) {
        let range = self.ranges[layer as usize].clone();
        if range.is_empty() {
            return;
        }
        let [x, y, w, h] = self.clip;
        pass.set_scissor_rect(x, y, w, h);
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.buf.slice(..));
        let mut i = range.start;
        while i < range.end {
            // Batch consecutive instances of the same image.
            let id = self.items[i as usize];
            let mut j = i + 1;
            while j < range.end && self.items[j as usize] == id {
                j += 1;
            }
            if let Some(t) = self.textures.get(&id) {
                pass.set_bind_group(0, &t.bind, &[]);
                pass.draw(0..4, i..j);
            }
            i = j;
        }
        pass.set_scissor_rect(0, 0, self.surface[0], self.surface[1]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grimalkin_graphics::{Placement, Rect};

    fn placement() -> Placement {
        Placement {
            image_id: 1,
            placement_id: 0,
            screen: GScreen::Main,
            col: 3,
            abs_row: 12,
            z: 0,
            cols: 4,
            rows: 3,
            src_rect: Rect { x: 10, y: 20, w: 50, h: 40 },
            pixel_offset: (2, 5),
            dest_px: (40, 60),
            is_virtual: false,
        }
    }

    const GEO: GridGeometry = GridGeometry {
        pad: 4,
        cell_w: 10,
        cell_h: 20,
        surface_w: 800,
        surface_h: 600,
    };

    #[test]
    fn anchors_to_cell_and_offset() {
        let i = layout(&placement(), 100, 100, 10, GEO);
        assert_eq!(i.pos, [4.0 + 30.0 + 2.0, 4.0 + 40.0 + 5.0]);
        assert_eq!(i.size, [40.0, 60.0]);
        assert_eq!(i.uv0, [0.1, 0.2]);
        assert_eq!(i.uv1, [0.6, 0.6]);
    }

    #[test]
    fn moves_with_the_viewport_and_may_start_above_it() {
        let a = layout(&placement(), 100, 100, 10, GEO);
        let b = layout(&placement(), 100, 100, 11, GEO); // scrolled by one row
        assert_eq!(a.pos[1] - b.pos[1], 20.0);
        let c = layout(&placement(), 100, 100, 14, GEO); // top row already above the grid
        assert!(c.pos[1] < 0.0);
    }
}
