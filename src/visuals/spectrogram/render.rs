// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use bytemuck::{Pod, Zeroable};
use iced::Rectangle;
use iced::advanced::graphics::Viewport;
use iced_wgpu::primitive::{self, Primitive};
use std::collections::{HashMap, VecDeque};

use crate::util::color::f32_to_u8;

use crate::visuals::render::common::{
    CacheTracker, RenderPipelineSpec, begin_load_pass, create_render_pipeline, create_shader_module,
};

use super::processor::SpectrogramPoint;
use crate::util::audio::{FrequencyScale, hz_to_erb_rate};

pub const SPECTROGRAM_PALETTE_SIZE: usize = 5;
const LOG_KNEE_HZ: f32 = 20.0;

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
        mags: Vec<u16>,
    },
}

// (source_capacity, [source_slot, destination_slot] copies) for preserving
// GPU-resident columns when the CPU ring is resized or re-linearized.
pub type RingCopyPlan = (u32, Vec<[u32; 2]>);

pub struct SpectrogramParams {
    pub key: u64,
    pub bounds: Rectangle,
    pub ring_capacity: u32,
    pub points_per_column: u32,
    pub col_count: u32,
    pub write_slot: u32,
    pub pending_uploads: VecDeque<PendingUpload>,
    pub copy_plan: Option<RingCopyPlan>,
    pub col_kind: ColumnKind,
    pub freq_min: f32,
    pub freq_max: f32,
    pub bin_hz: f32,
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

pub struct SpectrogramPrimitive {
    params: SpectrogramParams,
}

impl std::fmt::Debug for SpectrogramPrimitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpectrogramPrimitive")
            .finish_non_exhaustive()
    }
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
        if inst.col_count == 0 {
            return;
        }

        let mut pass = begin_load_pass(encoder, target, clip, "Spectrogram pass");
        pass.set_bind_group(0, &r.ring.bg, &[]);
        pass.set_vertex_buffer(0, r.quad_buf.slice(..));
        match r.ring.kind {
            ColumnKind::Reassigned => {
                let instance_count = inst.col_count * inst.points_per_col;
                if instance_count == 0 {
                    return;
                }
                pass.set_pipeline(&pipeline.splat_pipeline);
                pass.set_vertex_buffer(1, r.ring.buf.slice(..));
                pass.draw(0..6, 0..instance_count);
            }
            ColumnKind::Classic => {
                if inst.points_per_col < 2 {
                    return;
                }
                pass.set_pipeline(&pipeline.classic_pipeline);
                pass.draw(0..6, 0..1);
            }
        }
    }
}

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
    const ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x2];
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<QuadCorner>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &ATTRS,
    }
}

fn point_instance_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![1 => Float32, 2 => Float32, 3 => Float32];
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<SpectrogramPoint>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &ATTRS,
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, PartialEq)]
struct Uniforms {
    freq_axis: [f32; 2], // (scaled_min, inverse scaled display span)
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
    newest_col: u32,
    inv_uv_range: f32,
    col_stride_u16: u32,
    bin_hz: f32,
    // 12 B pad so `stops` lands on a 16-byte boundary (array<vec4> align).
    _pad: [u32; 3],
    // (pos1, pos2, pos3, spread0), (spread1, spread2, spread3, spread4).
    // Stops 0 and 4 are constant 0.0 / 1.0 and live in the shader.
    stops: [[f32; 4]; 2],
    // Quantized to match the old Rgba8Unorm palette texture path.
    palette: [[f32; 4]; SPECTROGRAM_PALETTE_SIZE],
}

// Locks layout to what the WGSL Uniforms struct expects. Stops must land at
// offset 112 (16-aligned for array<vec4>), palette at 144, total 224 bytes.
const _: () = assert!(std::mem::size_of::<Uniforms>() == 224);
const _: () = assert!(std::mem::offset_of!(Uniforms, stops) == 112);
const _: () = assert!(std::mem::offset_of!(Uniforms, palette) == 144);

impl Uniforms {
    fn from_params(p: &SpectrogramParams, viewport: [f32; 2], scale_factor: f32) -> Self {
        let freq_scale = match p.freq_scale {
            FrequencyScale::Linear => 0,
            FrequencyScale::Logarithmic => 1,
            FrequencyScale::Erb => 2,
        };
        let scale_freq = |hz: f32| match p.freq_scale {
            FrequencyScale::Linear => hz,
            FrequencyScale::Logarithmic => (hz / LOG_KNEE_HZ).asinh(),
            FrequencyScale::Erb => hz_to_erb_rate(hz),
        };
        let freq_lo = scale_freq(p.freq_min);
        let freq_hi = scale_freq(p.freq_max);
        let palette = p
            .palette
            .map(|rgba| rgba.map(|c| f32_to_u8(c) as f32 / 255.0));
        let rotation = p.rotation.rem_euclid(4) as u32;
        let sf = scale_factor.max(1.0);
        let hl = p.ring_capacity.max(1);
        let newest_col = (p.write_slot + hl - 1) % hl;
        let inv_uv_range = 1.0 / (p.uv_y_range[1] - p.uv_y_range[0]).max(1e-12);
        let col_stride_u16 = p.points_per_column.div_ceil(2) * 2;
        Self {
            freq_axis: [freq_lo, 1.0 / (freq_hi - freq_lo).max(1e-12)],
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
            bin_hz: p.bin_hz,
            _pad: [0; 3],
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
            palette,
        }
    }
}

pub struct Pipeline {
    splat_pipeline: wgpu::RenderPipeline,
    classic_pipeline: wgpu::RenderPipeline,
    splat_bgl: wgpu::BindGroupLayout,
    classic_bgl: wgpu::BindGroupLayout,
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
            entries: &[uniform_entry],
        });
        let classic_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Spectrogram classic BGL"),
            entries: &[uniform_entry, mag_entry],
        });

        let splat_pipeline = create_render_pipeline(
            device,
            format,
            RenderPipelineSpec {
                label: "Spectrogram splat pipeline",
                shader: &shader,
                vertex_entry: "vs_splat",
                fragment_entry: "fs_splat",
                buffers: &[quad_corner_layout(), point_instance_layout()],
                bind_group_layouts: &[&splat_bgl],
                topology: wgpu::PrimitiveTopology::TriangleList,
            },
        );
        let classic_pipeline = create_render_pipeline(
            device,
            format,
            RenderPipelineSpec {
                label: "Spectrogram classic pipeline",
                shader: &shader,
                vertex_entry: "vs_classic",
                fragment_entry: "fs_classic",
                buffers: &[quad_corner_layout()],
                bind_group_layouts: &[&classic_bgl],
                topology: wgpu::PrimitiveTopology::TriangleList,
            },
        );

        Self {
            splat_pipeline,
            classic_pipeline,
            splat_bgl,
            classic_bgl,
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
            classic: &self.classic_bgl,
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
    classic: &'a wgpu::BindGroupLayout,
}

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
            Some(r) if r.ring.kind == p.col_kind && self.points_per_col == p.points_per_column => r,
            slot => slot.insert(Resources::new(device, bgls, p)),
        };
        res.sync(device, queue, bgls, p, viewport, scale_factor);
        self.col_count = p.col_count;
        self.points_per_col = p.points_per_column;
    }
}

// Column stride in bytes for the active storage kind. Packed rounds u16 pairs
// up to a full u32 so pack/unpack2x16 never straddles a word boundary.
pub(super) fn col_byte_stride(kind: ColumnKind, points_per_col: u32) -> u64 {
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
    uniform_cache: Uniforms,
    ring: ColumnRing,
    classic_upload_scratch: Vec<u16>,
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
        let ring = create_ring(device, bgls, &uniform_buf, p);

        Self {
            uniform_buf,
            quad_buf,
            uniform_cache: Uniforms::zeroed(),
            ring,
            classic_upload_scratch: Vec::new(),
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
        self.resize_ring(device, queue, bgls, p);
        self.upload_pending(queue, p);
        self.write_uniforms(queue, p, viewport, scale_factor);
    }

    fn resize_ring(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bgls: Bgls<'_>,
        p: &SpectrogramParams,
    ) {
        let stride = col_byte_stride(p.col_kind, p.points_per_column);
        let needed = p.ring_capacity as u64 * stride;
        let copy_plan = p
            .copy_plan
            .as_ref()
            .filter(|(_, copies)| p.col_count > 0 && !copies.is_empty());
        if needed == self.ring.capacity && copy_plan.is_none() {
            return;
        }

        let new_ring = create_ring(device, bgls, &self.uniform_buf, p);
        if let Some((source_cap, copies)) = copy_plan {
            let source_cap = u64::from(*source_cap).min(self.ring.capacity / stride);
            if source_cap > 0 {
                let mut encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                for &[src, dst] in copies {
                    if u64::from(src) < source_cap && dst < p.ring_capacity {
                        encoder.copy_buffer_to_buffer(
                            &self.ring.buf,
                            u64::from(src) * stride,
                            &new_ring.buf,
                            u64::from(dst) * stride,
                            stride,
                        );
                    }
                }
                queue.submit(std::iter::once(encoder.finish()));
            }
        }
        self.ring = new_ring;
    }

    fn upload_pending(&mut self, queue: &wgpu::Queue, p: &SpectrogramParams) {
        let stride = col_byte_stride(p.col_kind, p.points_per_column);
        let ring_buf = &self.ring.buf;
        let write = |slot: u32, data: &[u8]| {
            queue.write_buffer(ring_buf, slot as u64 * stride, data);
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
                let u16_stride = (stride / 2) as usize;
                self.classic_upload_scratch.resize(u16_stride, 0);
                let packed = &mut self.classic_upload_scratch;
                for upload in &p.pending_uploads {
                    if let PendingUpload::Classic { slot, mags } = upload
                        && !mags.is_empty()
                    {
                        let written = mags.len().min(u16_stride);
                        packed[..written].copy_from_slice(&mags[..written]);
                        if written < u16_stride {
                            packed[written..].fill(0);
                        }
                        write(*slot, bytemuck::cast_slice(packed));
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

}

fn create_ring(
    device: &wgpu::Device,
    bgls: Bgls<'_>,
    uniform_buf: &wgpu::Buffer,
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
            bgls.classic,
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
    let bg = make_bind_group(device, layout, uniform_buf, mag);
    ColumnRing {
        kind: p.col_kind,
        buf,
        capacity,
        bg,
    }
}

fn make_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    ub: &wgpu::Buffer,
    mag: Option<&wgpu::Buffer>,
) -> wgpu::BindGroup {
    let entry = |binding, resource| wgpu::BindGroupEntry { binding, resource };
    let mut entries = vec![entry(0, ub.as_entire_binding())];
    if let Some(buf) = mag {
        entries.push(entry(2, buf.as_entire_binding()));
    }
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Spectrogram BG"),
        layout,
        entries: &entries,
    })
}
