// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use bytemuck::{Pod, Zeroable};
use iced::Rectangle;
use iced::advanced::graphics::Viewport;
use iced_wgpu::primitive::{self, Primitive};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::visuals::render::common::{
    CacheTracker, RenderPipelineSpec, begin_load_pass, create_buffer, create_render_pipeline,
    create_shader_module,
};

use super::processor::{ColumnKind, SpectrogramColumn, SpectrogramPoint, col_byte_stride};
use crate::util::audio::FrequencyScale;

pub const SPECTROGRAM_PALETTE_SIZE: usize = crate::visuals::palettes::spectrogram::SIZE;

const ACCUM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;

// preserve GPU columns when the CPU ring is resized or re-linearized.
pub type RingCopyPlan = Vec<u32>;

#[derive(Debug)]
pub struct SpectrogramParams {
    pub key: u64,
    pub bounds: Rectangle,
    pub ring_capacity: u32,
    pub points_per_column: u32,
    pub reassigned_points_per_slot: u32,
    pub col_count: u32,
    pub write_slot: u32,
    pub pending_uploads: VecDeque<SpectrogramColumn>,
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

impl Primitive for SpectrogramParams {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _: &Rectangle,
        vp: &Viewport,
    ) {
        let params = self;
        let scale_factor = vp.scale_factor();
        let size = vp.logical_size();
        let viewport = [size.width, size.height];
        let (frame, prune) = pipeline.cache.advance();
        let inst = pipeline.instances.entry(params.key).or_default();
        inst.last_used = frame;
        let bgls = pipeline.bgls.each_ref();
        let res = match &mut inst.resources {
            Some(res) if res.ring.layout.kind == params.col_kind => res,
            slot => slot.insert(Resources::new(device, bgls, params)),
        };
        res.resize_ring(device, queue, bgls, params);
        res.resize_accum(device, bgls[1], params, scale_factor);
        res.upload_pending(queue, params);
        let uniforms = Uniforms::from_params(params, viewport, scale_factor);
        if uniforms != res.uniform_cache {
            queue.write_buffer(&res.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
            res.uniform_cache = uniforms;
        }
        if let Some(threshold) = prune {
            pipeline.instances.retain(|_, instance| instance.last_used >= threshold);
        }
    }

    fn render(
        &self,
        pipeline: &Self::Pipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip: &Rectangle<u32>,
    ) {
        let Some(inst) = pipeline.instances.get(&self.key) else {
            return;
        };
        let Some(r) = inst.resources.as_ref() else {
            return;
        };
        let visible_slots = self.col_count.min(r.ring.layout.slots as u32);
        let slot_count = |slot: u32| self.slot_counts.get(slot as usize).copied().unwrap_or(0);
        if r.ring.layout.kind == ColumnKind::Reassigned
            && !(0..visible_slots).any(|slot| slot_count(slot) > 0)
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
                        ..Default::default()
                    });
                    let stride = (r.ring.layout.stride
                        / std::mem::size_of::<SpectrogramPoint>() as u64)
                        as u32;
                    pass.set_pipeline(&pipeline.pipelines[0]);
                    pass.set_bind_group(0, &r.ring.bg, &[]);
                    pass.set_vertex_buffer(0, r.ring.buf.slice(..));
                    let mut slot = 0;
                    while slot < visible_slots {
                        let count = slot_count(slot).min(stride);
                        if count == 0 {
                            slot += 1;
                            continue;
                        }
                        let first = slot * stride;
                        if count == stride {
                            slot += 1;
                            while slot < visible_slots && slot_count(slot).min(stride) == stride {
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
                pass.set_pipeline(&pipeline.pipelines[1]);
                pass.set_bind_group(0, &accum.bg, &[]);
                pass.draw(0..4, 0..1);
            }
            ColumnKind::Classic => {
                if self.points_per_column < 2 {
                    return;
                }
                let mut pass = begin_load_pass(encoder, target, clip, "Spectrogram pass");
                pass.set_pipeline(&pipeline.pipelines[2]);
                pass.set_bind_group(0, &r.ring.bg, &[]);
                pass.draw(0..4, 0..1);
            }
        }
    }
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
    bin_hz: f32,
    reassigned_power_scale: f32,
    // Match WGSL's 16-byte array alignment.
    _padding: f32,
    // (pos1, pos2, pos3, spread0), (spread1, spread2, spread3, spread4).
    // Stops 0 and 4 are constant 0.0 / 1.0 and live in the shader.
    stops: [[f32; 4]; 2],
    palette: [[f32; 4]; SPECTROGRAM_PALETTE_SIZE],
}

// Locks layout to what the WGSL Uniforms struct expects.
const _: () = assert!(std::mem::size_of::<Uniforms>() == 208);
const _: () = assert!(std::mem::offset_of!(Uniforms, reassigned_power_scale) == 88);
const _: () = assert!(std::mem::offset_of!(Uniforms, stops) == 96);
const _: () = assert!(std::mem::offset_of!(Uniforms, palette) == 128);

impl Uniforms {
    fn from_params(p: &SpectrogramParams, viewport: [f32; 2], scale_factor: f32) -> Self {
        let freq_scale = p.freq_scale as u32;
        let freq_lo = p.freq_scale.scale(p.freq_min);
        let freq_hi = p.freq_scale.scale(p.freq_max);
        let rotation = p.rotation.rem_euclid(4) as u32;
        let sf = scale_factor.max(1.0);
        let hl = p.ring_capacity;
        let newest_col = (p.write_slot + hl - 1) % hl;
        let inv_uv_range = 1.0 / (p.uv_y_range[1] - p.uv_y_range[0]).max(1e-12);
        Self {
            freq_axis: [freq_lo, 1.0 / (freq_hi - freq_lo).max(1e-12)],
            freq_scale,
            points_per_col: stored_points_per_col(p),
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
            bin_hz: p.bin_hz,
            reassigned_power_scale: p.reassigned_power_scale,
            _padding: 0.0,
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
            palette: p.palette,
        }
    }
}

pub struct Pipeline {
    pipelines: [wgpu::RenderPipeline; 3],
    bgls: [wgpu::BindGroupLayout; 3],
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
        const POINT_ATTRS: [wgpu::VertexAttribute; 3] =
            wgpu::vertex_attr_array![1 => Float32, 2 => Float32, 3 => Float32];

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

        let additive = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let accum_pipeline = create_render_pipeline(
            device,
            ACCUM_FORMAT,
            RenderPipelineSpec {
                label: "Spectrogram accumulation pipeline",
                shader: &shader,
                vertex_entry: "vs_accum_splat",
                fragment_entry: "fs_accum",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<SpectrogramPoint>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &POINT_ATTRS,
                }],
                bind_group_layouts: &[&splat_bgl],
                blend: Some(wgpu::BlendState {
                    color: additive,
                    alpha: additive,
                }),
                write_mask: wgpu::ColorWrites::RED | wgpu::ColorWrites::GREEN,
            },
        );
        let pipeline = |label, vertex_entry, fragment_entry, bgl| {
            create_render_pipeline(
                device,
                format,
                RenderPipelineSpec {
                    label,
                    shader: &shader,
                    vertex_entry,
                    fragment_entry,
                    buffers: &[],
                    bind_group_layouts: &[bgl],
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                },
            )
        };
        let (resolve_label, classic_label) =
            ("Spectrogram resolve pipeline", "Spectrogram classic pipeline");
        let resolve_pipeline = pipeline(resolve_label, "vs_resolve", "fs_resolve", &resolve_bgl);
        let classic_pipeline = pipeline(classic_label, "vs_classic", "fs_classic", &classic_bgl);

        Self {
            pipelines: [accum_pipeline, resolve_pipeline, classic_pipeline],
            bgls: [splat_bgl, resolve_bgl, classic_bgl],
            instances: HashMap::new(),
            cache: CacheTracker::default(),
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

type Bgls<'a> = [&'a wgpu::BindGroupLayout; 3];

#[derive(Default)]
struct Instance {
    resources: Option<Resources>,
    last_used: u64,
}

fn stored_points_per_col(p: &SpectrogramParams) -> u32 {
    match p.col_kind {
        ColumnKind::Reassigned => p.reassigned_points_per_slot,
        ColumnKind::Classic => p.points_per_column,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RingLayout {
    kind: ColumnKind,
    stride: u64,
    slots: u64,
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
    tex: wgpu::Texture,
    view: wgpu::TextureView,
    bg: wgpu::BindGroup,
}

struct Resources {
    uniform_buf: wgpu::Buffer,
    uniform_cache: Uniforms,
    ring: ColumnRing,
    accum: Option<AccumTarget>,
    classic_upload_scratch: Vec<u16>,
}

impl Resources {
    fn new(device: &wgpu::Device, bgls: Bgls<'_>, p: &SpectrogramParams) -> Self {
        let uniform_buf = create_buffer(
            device,
            "Spectrogram UB",
            std::mem::size_of::<Uniforms>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let ring = create_ring(device, bgls, &uniform_buf, p);

        Self {
            uniform_buf,
            uniform_cache: Uniforms::zeroed(),
            ring,
            accum: None,
            classic_upload_scratch: Vec::new(),
        }
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
            .filter(|copies| copies.iter().any(|&dst| dst < p.ring_capacity));
        if can_reuse_ring(self.ring.layout, layout, copy_plan.is_some()) {
            return;
        }

        let old_layout = self.ring.layout;
        let new_ring = create_ring(device, bgls, &self.uniform_buf, p);
        if let Some(copies) = copy_plan {
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            for (src, &dst) in copies.iter().enumerate().filter(|(_, dst)| **dst < p.ring_capacity) {
                let bytes = match layout.kind {
                    ColumnKind::Reassigned => u64::from(p.slot_counts[dst as usize])
                        * std::mem::size_of::<SpectrogramPoint>() as u64,
                    ColumnKind::Classic => layout.stride,
                }
                .min(old_layout.stride)
                .min(layout.stride);
                if bytes > 0 {
                    encoder.copy_buffer_to_buffer(
                        &self.ring.buf,
                        src as u64 * old_layout.stride,
                        &new_ring.buf,
                        u64::from(dst) * layout.stride,
                        bytes,
                    );
                }
            }
            queue.submit(std::iter::once(encoder.finish()));
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
        let scale = scale_factor.max(1.0);
        let [width, height] = if matches!(p.rotation.rem_euclid(4), 1 | 3) {
            [p.bounds.height, p.bounds.width]
        } else {
            [p.bounds.width, p.bounds.height]
        };
        let size = [width, height].map(|value| (value.max(1.0) * scale).ceil() as u32);
        if self
            .accum
            .as_ref()
            .is_some_and(|a| [a.tex.width(), a.tex.height()] == size)
        {
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
        self.accum = Some(AccumTarget { tex, view, bg });
    }

    fn upload_pending(&mut self, queue: &wgpu::Queue, p: &SpectrogramParams) {
        let stride = ring_layout(p).stride;
        let ring_buf = &self.ring.buf;
        let write = |slot: u32, data: &[u8]| {
            queue.write_buffer(ring_buf, slot as u64 * stride, data);
        };
        let first = (p.write_slot + p.ring_capacity - p.pending_uploads.len() as u32)
            % p.ring_capacity;
        let slot = |offset: usize| (first + offset as u32) % p.ring_capacity;
        match p.col_kind {
            ColumnKind::Reassigned => {
                let point_stride =
                    (stride / std::mem::size_of::<SpectrogramPoint>() as u64) as usize;
                for (offset, column) in p.pending_uploads.iter().enumerate() {
                    if let SpectrogramColumn::Reassigned(points) = column
                        && !points.is_empty()
                    {
                        let written = points.len().min(point_stride);
                        write(slot(offset), bytemuck::cast_slice(&points[..written]));
                    }
                }
            }
            ColumnKind::Classic => {
                let u16_stride = (stride / 2) as usize;
                self.classic_upload_scratch.resize(u16_stride, 0);
                let packed = &mut self.classic_upload_scratch;
                for (offset, column) in p.pending_uploads.iter().enumerate() {
                    if let SpectrogramColumn::Classic(mags) = column
                        && !mags.is_empty()
                    {
                        let written = mags.len().min(u16_stride);
                        packed[..written].copy_from_slice(&mags[..written]);
                        if written < u16_stride {
                            packed[written..].fill(0);
                        }
                        write(slot(offset), bytemuck::cast_slice(packed));
                    }
                }
            }
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
            bgls[0],
        ),
        ColumnKind::Classic => (
            "Spectrogram mag ring",
            copy | wgpu::BufferUsages::STORAGE,
            bgls[2],
        ),
    };
    let buf = create_buffer(device, label, ring_layout.stride * ring_layout.slots, usage);
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

        assert_eq!(current.stride * current.slots, requested.stride * requested.slots);
        assert!(!can_reuse_ring(current, requested, false));
    }
}
