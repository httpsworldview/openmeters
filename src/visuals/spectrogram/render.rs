// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use bytemuck::{Pod, Zeroable};
use iced::Rectangle;
use iced::advanced::graphics::Viewport;
use iced_wgpu::primitive::{self, Primitive};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use wgpu::util::DeviceExt as _;

use crate::visuals::render::common::{
    CacheTracker, RenderPipelineSpec, begin_load_pass, create_render_pipeline, create_shader_module,
};

use super::processor::{ColumnKind, SpectrogramPoint, col_byte_stride};
use crate::util::audio::FrequencyScale;

pub const SPECTROGRAM_PALETTE_SIZE: usize = crate::visuals::palettes::spectrogram::SIZE;

const ACCUM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;

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

// preserve GPU columns when the CPU ring is resized or re-linearized.
pub type RingCopyPlan = (u32, Vec<[u32; 2]>);

pub struct SpectrogramParams {
    pub key: u64,
    pub bounds: Rectangle,
    pub ring_capacity: u32,
    pub points_per_column: u32,
    pub reassigned_points_per_slot: u32,
    pub col_count: u32,
    pub write_slot: u32,
    pub pending_uploads: VecDeque<PendingUpload>,
    pub copy_plan: Option<RingCopyPlan>,
    pub slot_counts: Arc<[u32]>,
    pub(super) col_kind: ColumnKind,
    pub freq_min: f32,
    pub freq_max: f32,
    pub bin_hz: f32,
    pub reassigned_power_scale: f32,
    pub freq_scale: FrequencyScale,
    pub palette: [[f32; 4]; SPECTROGRAM_PALETTE_SIZE],
    pub stop_positions: [f32; SPECTROGRAM_PALETTE_SIZE],
    pub stop_spreads: [f32; SPECTROGRAM_PALETTE_SIZE],
    pub floor_db: f32,
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
            self.params.key,
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
        let Some(inst) = pipeline.instances.get(&self.params.key) else {
            return;
        };
        let Some(r) = inst.resources.as_ref() else {
            return;
        };
        let visible_slots = r.uniform_cache.col_count.min(r.ring.layout.slots as u32);
        if visible_slots == 0
            || (r.ring.layout.kind == ColumnKind::Reassigned
                && !(0..visible_slots).any(|slot| inst.slot_count(slot) > 0))
        {
            return;
        }

        match r.ring.layout.kind {
            ColumnKind::Reassigned => {
                let Some(accum) = r.accum.as_ref() else {
                    return;
                };
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("Spectrogram accumulation pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &accum.view,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                    let stride = (r.ring.layout.stride
                        / std::mem::size_of::<SpectrogramPoint>() as u64)
                        as u32;
                    pass.set_pipeline(&pipeline.accum_pipeline);
                    pass.set_bind_group(0, &r.ring.bg, &[]);
                    pass.set_vertex_buffer(0, r.quad_buf.slice(..));
                    pass.set_vertex_buffer(1, r.ring.buf.slice(..));
                    let mut slot = 0;
                    while slot < visible_slots {
                        let count = inst.slot_count(slot).min(stride);
                        if count == 0 {
                            slot += 1;
                            continue;
                        }
                        let first = slot * stride;
                        if count == stride {
                            slot += 1;
                            while slot < visible_slots && inst.slot_count(slot).min(stride) == stride {
                                slot += 1;
                            }
                            pass.draw(0..4, first..slot * stride);
                        } else {
                            pass.draw(0..4, first..first + count);
                            slot += 1;
                        }
                    }
                }

                let mut pass = begin_load_pass(encoder, target, clip, "Spectrogram resolve pass");
                pass.set_pipeline(&pipeline.resolve_pipeline);
                pass.set_bind_group(0, &accum.bg, &[]);
                pass.set_vertex_buffer(0, r.quad_buf.slice(..));
                pass.draw(0..4, 0..1);
            }
            ColumnKind::Classic => {
                if r.uniform_cache.points_per_col < 2 {
                    return;
                }
                let mut pass = begin_load_pass(encoder, target, clip, "Spectrogram pass");
                pass.set_pipeline(&pipeline.classic_pipeline);
                pass.set_bind_group(0, &r.ring.bg, &[]);
                pass.set_vertex_buffer(0, r.quad_buf.slice(..));
                pass.draw(0..4, 0..1);
            }
        }
    }
}

type QuadCorner = [f32; 2];

const UNIT_QUAD: [QuadCorner; 4] = [
    [-0.5, 0.5],
    [-0.5, -0.5],
    [0.5, 0.5],
    [0.5, -0.5],
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

fn accum_size(bounds: Rectangle, rotation: i8, scale_factor: f32) -> [u32; 2] {
    let sf = scale_factor.max(1.0);
    let swapped = matches!((rotation as i32).rem_euclid(4), 1 | 3);
    let (w, h) = if swapped {
        (bounds.height, bounds.width)
    } else {
        (bounds.width, bounds.height)
    };
    [
        (w.max(1.0) * sf).ceil() as u32,
        (h.max(1.0) * sf).ceil() as u32,
    ]
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, PartialEq)]
struct Uniforms {
    freq_axis: [f32; 2], // (scaled_min, inverse scaled display span)
    freq_scale: u32,
    points_per_col: u32, // reassigned slot stride, or classic FFT bins
    history_length: u32,
    col_count: u32,
    rotation: u32,
    _header_padding: u32,
    bounds: [f32; 4],
    clip_scale: [f32; 2],
    uv_y_range: [f32; 2],
    scale_factor: f32,
    floor_db: f32,
    tilt_db: f32,
    newest_col: u32,
    inv_uv_range: f32,
    col_stride_u16: u32,
    bin_hz: f32,
    accum_size: [f32; 2],
    reassigned_power_scale: f32,
    // Match WGSL's 16-byte array alignment.
    _padding: [f32; 2],
    // (pos1, pos2, pos3, spread0), (spread1, spread2, spread3, spread4).
    // Stops 0 and 4 are constant 0.0 / 1.0 and live in the shader.
    stops: [[f32; 4]; 2],
    palette: [[f32; 4]; SPECTROGRAM_PALETTE_SIZE],
}

// Locks layout to what the WGSL Uniforms struct expects. Stops must land at
// offset 112 (16-aligned for array<vec4>), palette at 144, total 224 bytes.
const _: () = assert!(std::mem::size_of::<Uniforms>() == 224);
const _: () = assert!(std::mem::offset_of!(Uniforms, accum_size) == 92);
const _: () = assert!(std::mem::offset_of!(Uniforms, reassigned_power_scale) == 100);
const _: () = assert!(std::mem::offset_of!(Uniforms, stops) == 112);
const _: () = assert!(std::mem::offset_of!(Uniforms, palette) == 144);

impl Uniforms {
    fn from_params(p: &SpectrogramParams, viewport: [f32; 2], scale_factor: f32) -> Self {
        let freq_scale = match p.freq_scale {
            FrequencyScale::Linear => 0,
            FrequencyScale::Logarithmic => 1,
            FrequencyScale::Erb => 2,
        };
        let freq_lo = p.freq_scale.scale(p.freq_min);
        let freq_hi = p.freq_scale.scale(p.freq_max);
        let palette = p.palette;
        let rotation = p.rotation.rem_euclid(4) as u32;
        let sf = scale_factor.max(1.0);
        let hl = p.ring_capacity.max(1);
        let newest_col = (p.write_slot + hl - 1) % hl;
        let inv_uv_range = 1.0 / (p.uv_y_range[1] - p.uv_y_range[0]).max(1e-12);
        let col_stride_u16 = p.points_per_column.div_ceil(2) * 2;
        let acc_sz = accum_size(p.bounds, p.rotation, sf);
        Self {
            freq_axis: [freq_lo, 1.0 / (freq_hi - freq_lo).max(1e-12)],
            freq_scale,
            points_per_col: match p.col_kind {
                ColumnKind::Reassigned => p.reassigned_points_per_slot.max(1),
                ColumnKind::Classic => p.points_per_column,
            },
            history_length: p.ring_capacity,
            col_count: p.col_count,
            rotation,
            _header_padding: 0,
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
            tilt_db: p.tilt_db,
            newest_col,
            inv_uv_range,
            col_stride_u16,
            bin_hz: p.bin_hz,
            accum_size: [acc_sz[0] as f32, acc_sz[1] as f32],
            reassigned_power_scale: p.reassigned_power_scale,
            _padding: [0.0; 2],
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
    accum_pipeline: wgpu::RenderPipeline,
    resolve_pipeline: wgpu::RenderPipeline,
    classic_pipeline: wgpu::RenderPipeline,
    splat_bgl: wgpu::BindGroupLayout,
    classic_bgl: wgpu::BindGroupLayout,
    resolve_bgl: wgpu::BindGroupLayout,
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
        let accum_entry = bgl_entry(
            1,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
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
            entries: &[uniform_entry],
        });
        let classic_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Spectrogram classic BGL"),
            entries: &[uniform_entry, mag_entry],
        });
        let resolve_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Spectrogram resolve BGL"),
            entries: &[uniform_entry, accum_entry],
        });

        let accum_pipeline = create_render_pipeline(
            device,
            ACCUM_FORMAT,
            RenderPipelineSpec {
                label: "Spectrogram accumulation pipeline",
                shader: &shader,
                vertex_entry: "vs_accum_splat",
                fragment_entry: "fs_accum",
                buffers: &[quad_corner_layout(), point_instance_layout()],
                bind_group_layouts: &[&splat_bgl],
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                }),
                write_mask: wgpu::ColorWrites::RED | wgpu::ColorWrites::GREEN,
            },
        );
        let resolve_pipeline = create_render_pipeline(
            device,
            format,
            RenderPipelineSpec {
                label: "Spectrogram resolve pipeline",
                shader: &shader,
                vertex_entry: "vs_resolve",
                fragment_entry: "fs_resolve",
                buffers: &[quad_corner_layout()],
                bind_group_layouts: &[&resolve_bgl],
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
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
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            },
        );

        Self {
            accum_pipeline,
            resolve_pipeline,
            classic_pipeline,
            splat_bgl,
            classic_bgl,
            resolve_bgl,
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
            resolve: &self.resolve_bgl,
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
    resolve: &'a wgpu::BindGroupLayout,
}

#[derive(Default)]
struct Instance {
    resources: Option<Resources>,
    slot_counts: Arc<[u32]>,
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
            Some(r) if r.ring.layout.kind == p.col_kind => r,
            slot => slot.insert(Resources::new(device, bgls, p)),
        };
        res.sync(device, queue, bgls, p, viewport, scale_factor);
        self.slot_counts = Arc::clone(&p.slot_counts);
    }

    fn slot_count(&self, slot: u32) -> u32 {
        self.slot_counts.get(slot as usize).copied().unwrap_or(0)
    }
}

fn stored_points_per_col(p: &SpectrogramParams) -> u32 {
    match p.col_kind {
        ColumnKind::Reassigned => p.reassigned_points_per_slot.max(1),
        ColumnKind::Classic => p.points_per_column,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RingLayout {
    kind: ColumnKind,
    stride: u64,
    slots: u64,
}

impl RingLayout {
    fn bytes(self) -> u64 {
        self.stride * self.slots
    }
}

fn ring_layout(p: &SpectrogramParams) -> RingLayout {
    RingLayout {
        kind: p.col_kind,
        stride: col_byte_stride(p.col_kind, stored_points_per_col(p)),
        slots: u64::from(p.ring_capacity),
    }
}

fn can_reuse_ring(current: RingLayout, requested: RingLayout, copy_pending: bool) -> bool {
    current == requested && !copy_pending
}

struct ColumnRing {
    layout: RingLayout,
    buf: wgpu::Buffer,
    bg: wgpu::BindGroup,
}

struct AccumTarget {
    size: [u32; 2],
    _tex: wgpu::Texture,
    view: wgpu::TextureView,
    bg: wgpu::BindGroup,
}

struct Resources {
    uniform_buf: wgpu::Buffer,
    quad_buf: wgpu::Buffer,
    uniform_cache: Uniforms,
    ring: ColumnRing,
    accum: Option<AccumTarget>,
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
        let quad_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Spectrogram quad VB"),
            contents: bytemuck::cast_slice(&UNIT_QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ring = create_ring(device, bgls, &uniform_buf, p);

        Self {
            uniform_buf,
            quad_buf,
            uniform_cache: Uniforms::zeroed(),
            ring,
            accum: None,
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
        self.resize_accum(device, bgls.resolve, p, scale_factor);
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
        let layout = ring_layout(p);
        let copy_plan = p
            .copy_plan
            .as_ref()
            .filter(|(_, copies)| p.col_count > 0 && !copies.is_empty());
        if can_reuse_ring(self.ring.layout, layout, copy_plan.is_some()) {
            return;
        }

        let old_layout = self.ring.layout;
        let new_ring = create_ring(device, bgls, &self.uniform_buf, p);
        if let Some((source_cap, copies)) = copy_plan {
            let source_cap = u64::from(*source_cap).min(old_layout.slots);
            if source_cap > 0 {
                let mut encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                for &[src, dst] in copies {
                    if u64::from(src) < source_cap && dst < p.ring_capacity {
                        let bytes = match layout.kind {
                            ColumnKind::Reassigned => p
                                .slot_counts
                                .get(dst as usize)
                                .copied()
                                .unwrap_or(0) as u64
                                * std::mem::size_of::<SpectrogramPoint>() as u64,
                            ColumnKind::Classic => layout.stride,
                        }
                        .min(old_layout.stride)
                        .min(layout.stride);
                        if bytes > 0 {
                            encoder.copy_buffer_to_buffer(
                                &self.ring.buf,
                                u64::from(src) * old_layout.stride,
                                &new_ring.buf,
                                u64::from(dst) * layout.stride,
                                bytes,
                            );
                        }
                    }
                }
                queue.submit(std::iter::once(encoder.finish()));
            }
        }
        self.ring = new_ring;
    }

    fn resize_accum(
        &mut self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        p: &SpectrogramParams,
        scale_factor: f32,
    ) {
        if p.col_kind != ColumnKind::Reassigned {
            self.accum = None;
            return;
        }
        let size = accum_size(p.bounds, p.rotation, scale_factor);
        if self.accum.as_ref().is_some_and(|a| a.size == size) {
            return;
        }
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Spectrogram power accumulation texture"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: ACCUM_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        let bg = make_bind_group(device, layout, &self.uniform_buf, None, Some(&view));
        self.accum = Some(AccumTarget {
            size,
            _tex: tex,
            view,
            bg,
        });
    }

    fn upload_pending(&mut self, queue: &wgpu::Queue, p: &SpectrogramParams) {
        let stride = ring_layout(p).stride;
        let ring_buf = &self.ring.buf;
        let write = |slot: u32, data: &[u8]| {
            queue.write_buffer(ring_buf, slot as u64 * stride, data);
        };
        match p.col_kind {
            ColumnKind::Reassigned => {
                let point_stride =
                    (stride / std::mem::size_of::<SpectrogramPoint>() as u64) as usize;
                for upload in &p.pending_uploads {
                    if let PendingUpload::Reassigned { slot, points } = upload
                        && !points.is_empty()
                    {
                        let written = points.len().min(point_stride);
                        write(*slot, bytemuck::cast_slice(&points[..written]));
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
    let ring_layout = ring_layout(p);
    let (label, usage, bgl) = match ring_layout.kind {
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
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: ring_layout.bytes(),
        usage,
        mapped_at_creation: false,
    });
    let mag = (ring_layout.kind == ColumnKind::Classic).then_some(&buf);
    let bg = make_bind_group(device, bgl, uniform_buf, mag, None);
    ColumnRing {
        layout: ring_layout,
        buf,
        bg,
    }
}

fn make_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    ub: &wgpu::Buffer,
    mag: Option<&wgpu::Buffer>,
    accum: Option<&wgpu::TextureView>,
) -> wgpu::BindGroup {
    let entry = |binding, resource| wgpu::BindGroupEntry { binding, resource };
    let mut entries = vec![entry(0, ub.as_entire_binding())];
    if let Some(view) = accum {
        entries.push(entry(1, wgpu::BindingResource::TextureView(view)));
    }
    if let Some(buf) = mag {
        entries.push(entry(2, buf.as_entire_binding()));
    }
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Spectrogram BG"),
        layout,
        entries: &entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_byte_capacity_does_not_reuse_a_different_ring_layout() {
        let current = RingLayout {
            kind: ColumnKind::Classic,
            stride: col_byte_stride(ColumnKind::Classic, 513),
            slots: 513,
        };
        let requested = RingLayout {
            kind: ColumnKind::Classic,
            stride: col_byte_stride(ColumnKind::Classic, 1025),
            slots: 257,
        };

        assert_eq!(current.bytes(), requested.bytes());
        assert!(!can_reuse_ring(current, requested, false));
    }
}
