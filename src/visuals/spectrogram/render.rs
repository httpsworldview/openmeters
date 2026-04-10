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

#[derive(Debug, Clone)]
pub struct PendingUpload {
    pub slot: u32,
    pub points: Vec<SpectrogramPoint>,
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
        let instance_count = inst.col_count * inst.points_per_col;
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
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &r.bind_group, &[]);
        pass.set_vertex_buffer(0, r.quad_buf.slice(..));
        pass.set_vertex_buffer(1, r.point_buf.slice(..));
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
    freq_min_max: [f32; 2],
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
    _pad: [f32; 3],
    stop_positions: [[f32; 4]; 2],
    stop_spreads: [[f32; 4]; 2],
}

impl Uniforms {
    fn pack_stops(stops: &[f32; SPECTROGRAM_PALETTE_SIZE]) -> [[f32; 4]; 2] {
        let mut out = [[0.0f32; 4]; 2];
        for (i, &v) in stops.iter().enumerate() {
            out[i / 4][i % 4] = v;
        }
        out
    }

    fn from_params(p: &SpectrogramParams, viewport: [f32; 2], scale_factor: f32) -> Self {
        let freq_scale = match p.freq_scale {
            FrequencyScale::Linear => 0u32,
            FrequencyScale::Logarithmic => 1u32,
            FrequencyScale::Erb => 2u32,
        };
        let rotation = ((p.rotation as i32 % 4) + 4) as u32 % 4;
        let sf = scale_factor.max(1.0);
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
            _pad: [0.0; 3],
            stop_positions: Self::pack_stops(&p.stop_positions),
            stop_spreads: Self::pack_stops(&p.stop_spreads),
        }
    }
}

// pipeline

pub struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
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

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Spectrogram BGL"),
            entries: &[
                bgl_entry(
                    0,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                bgl_entry(
                    1,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D1,
                        multisampled: false,
                    },
                ),
            ],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Spectrogram pipeline"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: None,
                    bind_group_layouts: &[&layout],
                    push_constant_ranges: &[],
                }),
            ),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[quad_corner_layout(), point_instance_layout()],
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
        });

        Self {
            pipeline,
            layout,
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
        let inst = self.instances.entry(key).or_insert_with(Instance::new);
        inst.last_used = frame;
        inst.sync(device, queue, &self.layout, params, viewport, scale_factor);
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

// instance

struct Instance {
    resources: Option<Resources>,
    col_count: u32,
    points_per_col: u32,
    last_used: u64,
}

impl Instance {
    fn new() -> Self {
        Self {
            resources: None,
            col_count: 0,
            points_per_col: 0,
            last_used: 0,
        }
    }

    fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        p: &SpectrogramParams,
        viewport: [f32; 2],
        scale_factor: f32,
    ) {
        if p.ring_capacity == 0 || p.points_per_column == 0 {
            self.resources = None;
            self.col_count = 0;
            self.points_per_col = 0;
            return;
        }

        let res = self
            .resources
            .get_or_insert_with(|| Resources::new(device, layout, p));
        res.sync(device, queue, p, viewport, scale_factor);
        self.col_count = p.col_count;
        self.points_per_col = p.points_per_column;
    }
}

// gpu resources

struct Resources {
    uniform_buf: wgpu::Buffer,
    quad_buf: wgpu::Buffer,
    point_buf: wgpu::Buffer,
    point_capacity: u64,
    palette_tex: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    uniform_cache: Uniforms,
    palette_cache: [[f32; 4]; SPECTROGRAM_PALETTE_SIZE],
    quad_written: bool,
}

impl Resources {
    fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, p: &SpectrogramParams) -> Self {
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Spectrogram UB"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let quad_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Spectrogram quad VB"),
            size: (6 * std::mem::size_of::<QuadCorner>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let point_capacity = p.ring_capacity as u64
            * p.points_per_column as u64
            * std::mem::size_of::<SpectrogramPoint>() as u64;
        let point_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Spectrogram point ring"),
            size: point_capacity.max(std::mem::size_of::<SpectrogramPoint>() as u64),
            usage: wgpu::BufferUsages::VERTEX
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let (palette_tex, palette_view) = create_1d_texture(
            device,
            "Spectrogram palette",
            SPECTROGRAM_PALETTE_SIZE as u32,
            // Raw sRGB bytes pass through unchanged under web-colors mode.
            wgpu::TextureFormat::Rgba8Unorm,
        );
        let bind_group = make_bind_group(device, layout, &uniform_buf, &palette_view);

        Self {
            uniform_buf,
            quad_buf,
            point_buf,
            point_capacity: point_capacity.max(std::mem::size_of::<SpectrogramPoint>() as u64),
            palette_tex,
            bind_group,
            uniform_cache: Uniforms::zeroed(),
            palette_cache: [[0.0; 4]; SPECTROGRAM_PALETTE_SIZE],
            quad_written: false,
        }
    }

    fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        p: &SpectrogramParams,
        viewport: [f32; 2],
        scale_factor: f32,
    ) {
        if !self.quad_written {
            queue.write_buffer(&self.quad_buf, 0, bytemuck::cast_slice(&UNIT_QUAD));
            self.quad_written = true;
        }

        self.resize_point_buf(device, queue, p);
        self.upload_points(queue, p);
        self.write_uniforms(queue, p, viewport, scale_factor);
        self.write_palette(queue, p);
    }

    fn resize_point_buf(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        p: &SpectrogramParams,
    ) {
        let point_size = std::mem::size_of::<SpectrogramPoint>() as u64;
        let stride = p.points_per_column as u64 * point_size;
        let needed = (p.ring_capacity as u64 * stride).max(point_size);
        if needed <= self.point_capacity {
            return;
        }
        let new_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Spectrogram point ring"),
            size: needed,
            usage: wgpu::BufferUsages::VERTEX
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        if self.point_capacity > 0 && stride > 0 && p.col_count > 0 {
            let old_cap = self.point_capacity / stride;
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            if let Some(old_ws) = p.linearize_old_write_slot {
                // Ring was full and wrapped; linearize: reorder so oldest
                // data is at slot 0 and newest at slot col_count-1.
                let ws = old_ws as u64;
                let oldest_offset = ws * stride;
                let oldest_size = (old_cap - ws) * stride;
                let newest_size = ws * stride;
                encoder.copy_buffer_to_buffer(
                    &self.point_buf,
                    oldest_offset,
                    &new_buf,
                    0,
                    oldest_size,
                );
                if newest_size > 0 {
                    encoder.copy_buffer_to_buffer(
                        &self.point_buf,
                        0,
                        &new_buf,
                        oldest_size,
                        newest_size,
                    );
                }
            } else {
                let copy_size = (p.col_count as u64 * stride).min(self.point_capacity);
                encoder.copy_buffer_to_buffer(&self.point_buf, 0, &new_buf, 0, copy_size);
            }
            queue.submit(std::iter::once(encoder.finish()));
        }
        self.point_buf = new_buf;
        self.point_capacity = needed;
    }

    fn upload_points(&self, queue: &wgpu::Queue, p: &SpectrogramParams) {
        let stride = p.points_per_column as u64 * std::mem::size_of::<SpectrogramPoint>() as u64;
        for upload in &p.pending_uploads {
            if upload.points.is_empty() {
                continue;
            }
            let offset = upload.slot as u64 * stride;
            queue.write_buffer(
                &self.point_buf,
                offset,
                bytemuck::cast_slice(&upload.points),
            );
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
        for (i, rgba) in p.palette.iter().enumerate() {
            bytes[i * 4] = f32_to_u8(rgba[0]);
            bytes[i * 4 + 1] = f32_to_u8(rgba[1]);
            bytes[i * 4 + 2] = f32_to_u8(rgba[2]);
            bytes[i * 4 + 3] = f32_to_u8(rgba[3]);
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
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Spectrogram BG"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: ub.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(pal),
            },
        ],
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
