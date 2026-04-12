// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use bytemuck::{Pod, Zeroable};
use iced::Rectangle;
use iced::advanced::graphics::Viewport;
use iced_wgpu::primitive::{self, Primitive};
use iced_wgpu::wgpu;
use std::collections::HashMap;

use crate::util::color::f32_to_u8;

use crate::visuals::render::common::{CacheTracker, create_shader_module};

use super::processor::{FrequencyScale, SpectrogramPoint};

pub const SPECTROGRAM_PALETTE_SIZE: usize = 5;

const fn extent3d(w: u32, h: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: if w > 0 { w } else { 1 },
        height: if h > 0 { h } else { 1 },
        depth_or_array_layers: 1,
    }
}

#[inline]
fn write_texture_region(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    origin: wgpu::Origin3d,
    extent: wgpu::Extent3d,
    bytes_per_row: u32,
    data: &[u8],
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: None,
        },
        extent,
    );
}

// public API

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKind {
    Reassigned,
    Classic,
}

#[derive(Debug, Clone)]
pub enum PendingUpload {
    Reassigned {
        slot: u32,
        points: Vec<SpectrogramPoint>,
    },
    Classic {
        slot: u32,
        mags: Vec<f32>,
    },
}

pub struct SpectrogramParams {
    pub key: u64,
    pub bounds: Rectangle,
    pub ring_capacity: u32,
    pub points_per_column: u32,
    pub col_count: u32,
    pub write_slot: u32,
    pub pending_uploads: Vec<PendingUpload>,
    pub linearize_old_write_slot: Option<u32>,
    pub col_kind: ColumnKind,
    // Bottom of the displayable frequency axis, also the FFT bin spacing
    // (sample_rate / fft_size). Same value, both semantics.
    pub freq_min: f32,
    pub freq_max: f32,
    pub freq_scale: FrequencyScale,
    pub palette: [[f32; 4]; SPECTROGRAM_PALETTE_SIZE],
    pub stop_positions: [f32; SPECTROGRAM_PALETTE_SIZE],
    pub stop_spreads: [f32; SPECTROGRAM_PALETTE_SIZE],
    pub contrast: f32,
    pub floor_db: f32,
    pub ceiling_db: f32,
    pub tilt_db: f32,
    pub uv_y_range: [f32; 2],
    pub rotation: i8,
}

// primitive

#[derive(Debug)]
pub struct SpectrogramPrimitive {
    params: SpectrogramParams,
}

impl SpectrogramPrimitive {
    pub fn new(params: SpectrogramParams) -> Self {
        Self { params }
    }
    fn key(&self) -> u64 {
        self.params.key
    }
}

impl Primitive for SpectrogramPrimitive {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _: &Rectangle,
        vp: &Viewport,
    ) {
        let ls = vp.logical_size();
        pipeline.prepare(
            device,
            queue,
            self.key(),
            &self.params,
            [ls.width, ls.height],
            vp.scale_factor(),
        );
    }

    fn render(
        &self,
        pipeline: &Self::Pipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip: &Rectangle<u32>,
    ) {
        let Some(inst) = pipeline.instances.get(&self.key()) else {
            return;
        };
        let Some(r) = inst.resources.as_ref() else {
            return;
        };
        // Classic skips the trailing bin (no successor to fill into).
        let (pl, segs_per_col) = match r.ring.kind {
            ColumnKind::Reassigned => (&pipeline.splat_pipeline, inst.points_per_col),
            ColumnKind::Classic => (
                &pipeline.strip_pipeline,
                inst.points_per_col.saturating_sub(1),
            ),
        };
        let instance_count = inst.col_count * segs_per_col;
        if instance_count == 0 {
            return;
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Spectrogram pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_scissor_rect(clip.x, clip.y, clip.width.max(1), clip.height.max(1));
        pass.set_pipeline(pl);
        pass.set_bind_group(0, &r.ring.bg, &[]);
        pass.set_vertex_buffer(0, r.quad_buf.slice(..));
        if r.ring.kind == ColumnKind::Reassigned {
            pass.set_vertex_buffer(1, r.ring.buf.slice(..));
        }
        pass.draw(0..6, 0..instance_count);
    }
}

// gpu types

type QuadCorner = [f32; 2];

const UNIT_QUAD: [QuadCorner; 6] = [
    [-0.5, -0.5],
    [0.5, -0.5],
    [0.5, 0.5],
    [-0.5, -0.5],
    [0.5, 0.5],
    [-0.5, 0.5],
];

fn quad_corner_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<QuadCorner>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[wgpu::VertexAttribute {
            offset: 0,
            shader_location: 0,
            format: wgpu::VertexFormat::Float32x2,
        }],
    }
}

fn point_instance_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<SpectrogramPoint>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &[
            wgpu::VertexAttribute {
                offset: 0,
                shader_location: 1,
                format: wgpu::VertexFormat::Float32,
            },
            wgpu::VertexAttribute {
                offset: 4,
                shader_location: 2,
                format: wgpu::VertexFormat::Float32,
            },
            wgpu::VertexAttribute {
                offset: 8,
                shader_location: 3,
                format: wgpu::VertexFormat::Float32,
            },
        ],
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, PartialEq)]
struct Uniforms {
    freq_min_max: [f32; 2], // (bin_hz, max_hz)
    freq_scale: u32,
    points_per_col: u32,
    history_length: u32,
    col_count: u32,
    write_slot: u32,
    rotation: u32,
    bounds: [f32; 4],
    clip_scale: [f32; 2],
    uv_y_range: [f32; 2],
    scale_factor: f32,
    floor_db: f32,
    ceiling_db: f32,
    contrast: f32,
    tilt_db: f32,
    // Precomputed scalars; also fill the 12 B of pad before the stops block.
    newest_col: u32,
    inv_uv_range: f32,
    col_stride_u16: u32,
    // (pos1, pos2, pos3, spread0), (spread1, spread2, spread3, spread4).
    // Stops 0 and 4 are constant 0.0 / 1.0 and live in the shader.
    stops: [[f32; 4]; 2],
}

impl Uniforms {
    fn from_params(p: &SpectrogramParams, viewport: [f32; 2], scale_factor: f32) -> Self {
        let freq_scale = match p.freq_scale {
            FrequencyScale::Linear => 0u32,
            FrequencyScale::Logarithmic => 1u32,
            FrequencyScale::Erb => 2u32,
        };
        let rotation = p.rotation.rem_euclid(4) as u32;
        let sf = scale_factor.max(1.0);
        let hl = p.ring_capacity.max(1);
        let newest_col = (p.write_slot + hl - 1) % hl;
        let inv_uv_range = 1.0 / (p.uv_y_range[1] - p.uv_y_range[0]).max(1e-12);
        let col_stride_u16 = p.points_per_column.div_ceil(2) * 2;
        Self {
            freq_min_max: [p.freq_min, p.freq_max],
            freq_scale,
            points_per_col: p.points_per_column,
            history_length: p.ring_capacity,
            col_count: p.col_count,
            write_slot: p.write_slot,
            rotation,
            bounds: [
                p.bounds.x * sf,
                p.bounds.y * sf,
                p.bounds.width.max(1.0) * sf,
                p.bounds.height.max(1.0) * sf,
            ],
            clip_scale: [
                2.0 / (viewport[0] * sf).max(1.0),
                2.0 / (viewport[1] * sf).max(1.0),
            ],
            uv_y_range: p.uv_y_range,
            scale_factor: sf,
            floor_db: p.floor_db,
            ceiling_db: p.ceiling_db,
            contrast: p.contrast.max(0.01),
            tilt_db: p.tilt_db,
            newest_col,
            inv_uv_range,
            col_stride_u16,
            stops: [
                [
                    p.stop_positions[1],
                    p.stop_positions[2],
                    p.stop_positions[3],
                    p.stop_spreads[0],
                ],
                [
                    p.stop_spreads[1],
                    p.stop_spreads[2],
                    p.stop_spreads[3],
                    p.stop_spreads[4],
                ],
            ],
        }
    }
}

// pipeline

pub struct Pipeline {
    splat_pipeline: wgpu::RenderPipeline,
    strip_pipeline: wgpu::RenderPipeline,
    splat_bgl: wgpu::BindGroupLayout,
    strip_bgl: wgpu::BindGroupLayout,
    instances: HashMap<u64, Instance>,
    cache: CacheTracker,
}

impl primitive::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = create_shader_module(
            device,
            "Spectrogram shader",
            include_str!("../render/shaders/spectrogram.wgsl"),
        );

        let uniform_entry = bgl_entry(
            0,
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
        );
        let palette_entry = bgl_entry(
            1,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D1,
                multisampled: false,
            },
        );
        let mag_entry = bgl_entry(
            2,
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
        );

        let splat_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Spectrogram splat BGL"),
            entries: &[uniform_entry, palette_entry],
        });
        let strip_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Spectrogram strip BGL"),
            entries: &[uniform_entry, palette_entry, mag_entry],
        });

        let build = |label: &'static str,
                     entry: &'static str,
                     bgl: &wgpu::BindGroupLayout,
                     buffers: &[wgpu::VertexBufferLayout]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(
                    &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some(label),
                        bind_group_layouts: &[bgl],
                        push_constant_ranges: &[],
                    }),
                ),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(entry),
                    buffers,
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };

        let splat_pipeline = build(
            "Spectrogram splat pipeline",
            "vs_splat",
            &splat_bgl,
            &[quad_corner_layout(), point_instance_layout()],
        );
        let strip_pipeline = build(
            "Spectrogram strip pipeline",
            "vs_strip",
            &strip_bgl,
            &[quad_corner_layout()],
        );

        Self {
            splat_pipeline,
            strip_pipeline,
            splat_bgl,
            strip_bgl,
            instances: HashMap::new(),
            cache: CacheTracker::default(),
        }
    }
}

impl Pipeline {
    fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: u64,
        params: &SpectrogramParams,
        viewport: [f32; 2],
        scale_factor: f32,
    ) {
        let (frame, prune) = self.cache.advance();
        let inst = self.instances.entry(key).or_default();
        inst.last_used = frame;
        let bgls = Bgls {
            splat: &self.splat_bgl,
            strip: &self.strip_bgl,
        };
        inst.sync(device, queue, bgls, params, viewport, scale_factor);
        if let Some(t) = prune {
            self.instances.retain(|_, i| i.last_used >= t);
        }
    }
}

fn bgl_entry(binding: u32, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty,
        count: None,
    }
}

#[derive(Clone, Copy)]
struct Bgls<'a> {
    splat: &'a wgpu::BindGroupLayout,
    strip: &'a wgpu::BindGroupLayout,
}

// instance

#[derive(Default)]
struct Instance {
    resources: Option<Resources>,
    col_count: u32,
    points_per_col: u32,
    last_used: u64,
}

impl Instance {
    fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bgls: Bgls<'_>,
        p: &SpectrogramParams,
        viewport: [f32; 2],
        scale_factor: f32,
    ) {
        if p.ring_capacity == 0 || p.points_per_column == 0 {
            self.resources = None;
            return;
        }
        let res = match &mut self.resources {
            Some(r) if r.ring.kind == p.col_kind => r,
            slot => slot.insert(Resources::new(device, bgls, p)),
        };
        res.sync(device, queue, bgls, p, viewport, scale_factor);
        self.col_count = p.col_count;
        self.points_per_col = p.points_per_column;
    }
}

// gpu resources

// Fixed [dB] storage domain — must match the shader constants in spectrogram.wgsl.
// u16 unorm over this range gives ~0.0024 dB/step, decoupled from the live
// floor/ceiling window so history recolors cleanly on slider drags.
const DB_STORE_LO: f32 = -144.0;
const DB_STORE_HI: f32 = 12.0;
const DB_STORE_RANGE: f32 = DB_STORE_HI - DB_STORE_LO;

// Column stride in bytes for the active storage kind. Packed rounds u16 pairs
// up to a full u32 so pack/unpack2x16 never straddles a word boundary.
fn col_byte_stride(kind: ColumnKind, points_per_col: u32) -> u64 {
    match kind {
        ColumnKind::Reassigned => {
            points_per_col as u64 * std::mem::size_of::<SpectrogramPoint>() as u64
        }
        ColumnKind::Classic => (points_per_col as u64).div_ceil(2) * 4,
    }
}

struct ColumnRing {
    kind: ColumnKind,
    buf: wgpu::Buffer,
    capacity: u64,
    bg: wgpu::BindGroup,
}

struct Resources {
    uniform_buf: wgpu::Buffer,
    quad_buf: wgpu::Buffer,
    palette_tex: wgpu::Texture,
    palette_view: wgpu::TextureView,
    uniform_cache: Uniforms,
    palette_cache: [[f32; 4]; SPECTROGRAM_PALETTE_SIZE],
    ring: ColumnRing,
}

impl Resources {
    fn new(device: &wgpu::Device, bgls: Bgls<'_>, p: &SpectrogramParams) -> Self {
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Spectrogram UB"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let quad_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Spectrogram quad VB"),
            size: std::mem::size_of_val(&UNIT_QUAD) as u64,
            usage: wgpu::BufferUsages::VERTEX,
            mapped_at_creation: true,
        });
        quad_buf
            .slice(..)
            .get_mapped_range_mut()
            .copy_from_slice(bytemuck::cast_slice(&UNIT_QUAD));
        quad_buf.unmap();
        // Rgba8Unorm palette: raw sRGB stops, mix in sRGB space (web-colors pipeline).
        let (palette_tex, palette_view) = create_1d_texture(
            device,
            "Spectrogram palette",
            SPECTROGRAM_PALETTE_SIZE as u32,
            wgpu::TextureFormat::Rgba8Unorm,
        );
        let ring = create_ring(device, bgls, &uniform_buf, &palette_view, p);

        Self {
            uniform_buf,
            quad_buf,
            palette_tex,
            palette_view,
            uniform_cache: Uniforms::zeroed(),
            palette_cache: [[0.0; 4]; SPECTROGRAM_PALETTE_SIZE],
            ring,
        }
    }

    fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bgls: Bgls<'_>,
        p: &SpectrogramParams,
        viewport: [f32; 2],
        scale_factor: f32,
    ) {
        self.grow_ring(device, queue, bgls, p);
        self.upload_pending(queue, p);
        self.write_uniforms(queue, p, viewport, scale_factor);
        self.write_palette(queue, p);
    }

    // Grow the ring in place, preserving history via copy_buffer_to_buffer.
    // On a ring-full wrap the caller signals `linearize_old_write_slot`; we
    // reorder so slot 0 becomes the oldest column in the new, bigger ring.
    fn grow_ring(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bgls: Bgls<'_>,
        p: &SpectrogramParams,
    ) {
        let stride = col_byte_stride(p.col_kind, p.points_per_column);
        let needed = p.ring_capacity as u64 * stride;
        if needed <= self.ring.capacity {
            return;
        }
        let new_ring = create_ring(device, bgls, &self.uniform_buf, &self.palette_view, p);
        if p.col_count > 0 {
            let old_cap_cols = self.ring.capacity / stride;
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            if let Some(old_ws) = p.linearize_old_write_slot {
                let ws = old_ws as u64;
                let tail = (old_cap_cols - ws) * stride;
                encoder.copy_buffer_to_buffer(&self.ring.buf, ws * stride, &new_ring.buf, 0, tail);
                if ws > 0 {
                    encoder.copy_buffer_to_buffer(
                        &self.ring.buf,
                        0,
                        &new_ring.buf,
                        tail,
                        ws * stride,
                    );
                }
            } else {
                let copy = (p.col_count as u64 * stride).min(self.ring.capacity);
                encoder.copy_buffer_to_buffer(&self.ring.buf, 0, &new_ring.buf, 0, copy);
            }
            queue.submit(std::iter::once(encoder.finish()));
        }
        self.ring = new_ring;
    }

    fn upload_pending(&self, queue: &wgpu::Queue, p: &SpectrogramParams) {
        let stride = col_byte_stride(p.col_kind, p.points_per_column);
        let write = |slot: u32, data: &[u8]| {
            queue.write_buffer(&self.ring.buf, slot as u64 * stride, data);
        };
        match p.col_kind {
            ColumnKind::Reassigned => {
                for upload in &p.pending_uploads {
                    if let PendingUpload::Reassigned { slot, points } = upload
                        && !points.is_empty()
                    {
                        write(*slot, bytemuck::cast_slice(points));
                    }
                }
            }
            ColumnKind::Classic => {
                // Trailing u16 (for odd bin counts) stays zero: vec![0; _] inits it
                // once and nothing ever writes there.
                let u16_stride = (stride / 2) as usize;
                let mut packed = vec![0u16; u16_stride];
                let inv_range = 65535.0 / DB_STORE_RANGE;
                for upload in &p.pending_uploads {
                    if let PendingUpload::Classic { slot, mags } = upload
                        && !mags.is_empty()
                    {
                        for (i, &db) in mags.iter().enumerate().take(u16_stride) {
                            packed[i] = ((db - DB_STORE_LO) * inv_range).clamp(0.0, 65535.0) as u16;
                        }
                        write(*slot, bytemuck::cast_slice(&packed));
                    }
                }
            }
        }
    }

    fn write_uniforms(
        &mut self,
        queue: &wgpu::Queue,
        p: &SpectrogramParams,
        viewport: [f32; 2],
        scale_factor: f32,
    ) {
        let u = Uniforms::from_params(p, viewport, scale_factor);
        if u != self.uniform_cache {
            queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&u));
            self.uniform_cache = u;
        }
    }

    fn write_palette(&mut self, queue: &wgpu::Queue, p: &SpectrogramParams) {
        if p.palette == self.palette_cache {
            return;
        }
        let mut bytes = [0u8; SPECTROGRAM_PALETTE_SIZE * 4];
        for (dst, rgba) in bytes.chunks_exact_mut(4).zip(p.palette.iter()) {
            dst.copy_from_slice(&rgba.map(f32_to_u8));
        }
        write_texture_region(
            queue,
            &self.palette_tex,
            wgpu::Origin3d::ZERO,
            extent3d(SPECTROGRAM_PALETTE_SIZE as u32, 1),
            (SPECTROGRAM_PALETTE_SIZE * 4) as u32,
            &bytes,
        );
        self.palette_cache = p.palette;
    }
}

fn create_ring(
    device: &wgpu::Device,
    bgls: Bgls<'_>,
    uniform_buf: &wgpu::Buffer,
    palette_view: &wgpu::TextureView,
    p: &SpectrogramParams,
) -> ColumnRing {
    let copy = wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
    let (label, usage, layout) = match p.col_kind {
        ColumnKind::Reassigned => (
            "Spectrogram point ring",
            copy | wgpu::BufferUsages::VERTEX,
            bgls.splat,
        ),
        ColumnKind::Classic => (
            "Spectrogram mag ring",
            copy | wgpu::BufferUsages::STORAGE,
            bgls.strip,
        ),
    };
    let capacity = p.ring_capacity as u64 * col_byte_stride(p.col_kind, p.points_per_column);
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: capacity,
        usage,
        mapped_at_creation: false,
    });
    let mag = (p.col_kind == ColumnKind::Classic).then_some(&buf);
    let bg = make_bind_group(device, layout, uniform_buf, palette_view, mag);
    ColumnRing {
        kind: p.col_kind,
        buf,
        capacity,
        bg,
    }
}

fn create_1d_texture(
    device: &wgpu::Device,
    label: &'static str,
    width: u32,
    format: wgpu::TextureFormat,
) -> (wgpu::Texture, wgpu::TextureView) {
    let view_fmt = [format];
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: extent3d(width.max(1), 1),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D1,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &view_fmt,
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D1),
        ..Default::default()
    });
    (tex, view)
}

fn make_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    ub: &wgpu::Buffer,
    pal: &wgpu::TextureView,
    mag: Option<&wgpu::Buffer>,
) -> wgpu::BindGroup {
    let entry = |binding, resource| wgpu::BindGroupEntry { binding, resource };
    let mut entries = vec![
        entry(0, ub.as_entire_binding()),
        entry(1, wgpu::BindingResource::TextureView(pal)),
    ];
    if let Some(buf) = mag {
        entries.push(entry(2, buf.as_entire_binding()));
    }
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Spectrogram BG"),
        layout,
        entries: &entries,
    })
}

impl std::fmt::Debug for SpectrogramParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpectrogramParams")
            .field("key", &self.key)
            .field("bounds", &self.bounds)
            .field("col_count", &self.col_count)
            .finish_non_exhaustive()
    }
}
