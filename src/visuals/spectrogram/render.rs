// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use bytemuck::{Pod, Zeroable};
use iced::Rectangle;
use iced::advanced::graphics::Viewport;
use iced_wgpu::primitive::{self, Primitive};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use wgpu::util::DeviceExt as _;

use crate::visuals::render::common::{
    CacheTracker, RenderPipelineSpec, begin_pass, create_buffer, create_render_pipeline,
    create_shader_module,
};

use super::processor::{ColumnKind, SpectrogramColumn, SpectrogramPoint, col_byte_stride};
use crate::util::audio::FrequencyScale;

pub const SPECTROGRAM_PALETTE_SIZE: usize = crate::visuals::palettes::spectrogram::SIZE;

const ACCUM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;
const PAGE_COLUMNS: usize = 64;

// preserve GPU columns when the CPU ring is resized or re-linearized.
pub type RingCopyPlan = Vec<u32>;

#[derive(Debug)]
pub struct SpectrogramParams {
    pub key: u64,
    pub bounds: Rectangle,
    pub ring_capacity: u32,
    pub points_per_column: u32,
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
        let bgls = pipeline.bgls.each_ref();
        let res = pipeline.instances.entry(params.key)
            .or_insert_with(|| Resources::new(device, bgls, params));
        if res.ring.layout.kind != params.col_kind {
            *res = Resources::new(device, bgls, params);
        }
        res.last_used = frame;
        res.resize_ring(device, queue, bgls, params);
        res.resize_accum(device, bgls[1], params, scale_factor);
        res.upload_pending(queue, params);
        let mut uniforms = Uniforms::from_params(params, viewport, scale_factor);
        uniforms.page_slot_mask = (res.ring.layout.page_columns as u32).next_power_of_two() - 1;
        let changed = uniforms != res.uniform_cache;
        if let Some(accum) = &mut res.accum {
            *accum.dirty.get_mut() |= !params.pending_uploads.is_empty()
                || params.copy_plan.is_some()
                || (changed && uniforms.accumulation_key() != res.uniform_cache.accumulation_key());
        }
        if changed {
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
        let Some(r) = pipeline.instances.get(&self.key) else {
            return;
        };
        let visible_slots = self.col_count.min(r.ring.layout.slots as u32);
        if r.ring.layout.kind == ColumnKind::Reassigned
            && !self.slot_counts.iter().take(visible_slots as usize).any(|&count| count > 0)
        {
            return;
        }

        let (index, bg) = match r.ring.layout.kind {
            ColumnKind::Reassigned => {
                let Some(accum) = r.accum.as_ref() else {
                    return;
                };
                if accum.dirty.swap(false, Ordering::Relaxed) {
                    let mut pass = begin_pass(
                        encoder, &accum.view, None, "Spectrogram accumulation pass",
                        wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    );
                    let RingStorage::Reassigned { pages, bg } = &r.ring.storage else {
                        return;
                    };
                    let mut indexed = r.indices.is_some();
                    pass.set_pipeline(&pipeline.pipelines[if indexed { 3 } else { 0 }]);
                    if let Some((_, indices)) = &r.indices {
                        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                    } else {
                        pass.set_bind_group(0, bg, &[]);
                    }
                    let page_columns = r.ring.layout.page_columns;
                    for (index, page) in pages.iter().enumerate() {
                        let Some(page) = page else { continue };
                        let first = (index * page_columns) as u32;
                        let columns = visible_slots.saturating_sub(first).min(page_columns as u32);
                        if columns > 0 {
                            let counts = &self.slot_counts[first as usize..][..columns as usize];
                            if let (Some((capacity, _)), Some(bg)) = (&r.indices, &page.indexed_bg) {
                                if !indexed {
                                    pass.set_pipeline(&pipeline.pipelines[3]);
                                    indexed = true;
                                }
                                pass.set_bind_group(0, bg, &[]);
                                let tag = page_vertex(first, page.stride, r.uniform_cache.points_per_col) / 4;
                                for (columns, count) in equal_count_batches(counts, (*capacity).min(page.stride)) {
                                    pass.draw_indexed(0..count * 6, 0, tag + columns.start..tag + columns.end);
                                }
                            } else {
                                if indexed {
                                    pass.set_pipeline(&pipeline.pipelines[0]);
                                    pass.set_bind_group(0, bg, &[]);
                                    indexed = false;
                                }
                                pass.set_vertex_buffer(0, page.buf.slice(..));
                                let vertex = page_vertex(first, page.stride, r.uniform_cache.points_per_col);
                                for points in point_draws(counts, page.stride) {
                                    pass.draw(vertex..vertex + 4, points);
                                }
                            }
                        }
                    }
                }

                (1, &accum.bg)
            }
            ColumnKind::Classic => {
                if self.points_per_column < 2 {
                    return;
                }
                let RingStorage::Classic { bg, .. } = &r.ring.storage else { return };
                (2, bg)
            }
        };
        let mut pass = begin_pass(encoder, target, Some(clip), "Spectrogram pass", wgpu::LoadOp::Load);
        pass.set_pipeline(&pipeline.pipelines[index]);
        pass.set_bind_group(0, bg, &[]);
        pass.draw(0..4, 0..1);
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, PartialEq)]
struct Uniforms {
    freq_axis: [f32; 2], // (scaled_min, inverse scaled display span)
    freq_scale: u32,
    points_per_col: u32, // reassigned page-tag shift, or classic FFT bins
    history_length: u32,
    col_count: u32,
    rotation: u32,
    page_mask: u32,
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
    page_slot_mask: u32,
    // (pos1, pos2, pos3, spread0), (spread1, spread2, spread3, spread4).
    // Stops 0 and 4 are constant 0.0 / 1.0 and live in the shader.
    stops: [[f32; 4]; 2],
    palette: [[f32; 4]; SPECTROGRAM_PALETTE_SIZE],
}

// Locks layout to what the WGSL Uniforms struct expects.
const _: () = assert!(std::mem::size_of::<Uniforms>() == 208);
const _: () = assert!(std::mem::offset_of!(Uniforms, page_mask) == 28);
const _: () = assert!(std::mem::offset_of!(Uniforms, reassigned_power_scale) == 88);
const _: () = assert!(std::mem::offset_of!(Uniforms, page_slot_mask) == 92);
const _: () = assert!(std::mem::offset_of!(Uniforms, stops) == 96);
const _: () = assert!(std::mem::offset_of!(Uniforms, palette) == 128);

impl Uniforms {
    fn accumulation_key(&self) -> impl PartialEq {
        let size = [self.bounds[2], self.bounds[3]];
        let extents = if self.rotation.is_multiple_of(2) { size } else { [size[1], size[0]] };
        (self.freq_axis, self.freq_scale, self.history_length, self.col_count, self.newest_col,
            extents, self.uv_y_range, self.scale_factor, self.tilt_db)
    }

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
            points_per_col: if p.col_kind == ColumnKind::Reassigned { hl.next_power_of_two().trailing_zeros() } else { p.points_per_column },
            history_length: p.ring_capacity,
            col_count: p.col_count,
            rotation,
            page_mask: hl.next_power_of_two() - 1,
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
            page_slot_mask: 0,
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
    pipelines: [wgpu::RenderPipeline; 4],
    bgls: [wgpu::BindGroupLayout; 4],
    instances: HashMap<u64, Resources>,
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

        let points_entry = wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::VERTEX, ..mag_entry };
        let [splat_bgl, resolve_bgl, classic_bgl, indexed_bgl] = [
            &[uniform_entry][..],
            &[uniform_entry, accum_entry][..],
            &[uniform_entry, mag_entry][..],
            &[uniform_entry, points_entry][..],
        ].map(|entries| device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Spectrogram BGL"),
            entries,
        }));

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
                topology: wgpu::PrimitiveTopology::TriangleStrip,
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
        let indexed_pipeline = create_render_pipeline(device, ACCUM_FORMAT, RenderPipelineSpec {
            label: "Spectrogram indexed accumulation", shader: &shader, vertex_entry: "vs_accum_indexed",
            fragment_entry: "fs_accum", buffers: &[], bind_group_layouts: &[&indexed_bgl],
            topology: wgpu::PrimitiveTopology::TriangleList,
            blend: Some(wgpu::BlendState { color: additive, alpha: additive }),
            write_mask: wgpu::ColorWrites::RED | wgpu::ColorWrites::GREEN,
        });
        let pipeline = |label, vertex_entry, fragment_entry, bgl| {
            create_render_pipeline(
                device,
                format,
                RenderPipelineSpec {
                    label,
                    shader: &shader,
                    vertex_entry,
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    fragment_entry,
                    buffers: &[],
                    bind_group_layouts: &[bgl],
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                },
            )
        };
        Self {
            pipelines: [
                accum_pipeline,
                pipeline("Spectrogram resolve pipeline", "vs_resolve", "fs_resolve", &resolve_bgl),
                pipeline("Spectrogram classic pipeline", "vs_classic", "fs_classic", &classic_bgl),
                indexed_pipeline,
            ],
            bgls: [splat_bgl, resolve_bgl, classic_bgl, indexed_bgl],
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

type Bgls<'a> = [&'a wgpu::BindGroupLayout; 4];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RingLayout {
    kind: ColumnKind,
    stride: u64,
    slots: u64,
    page_columns: usize,
}

fn point_page_columns(maxima: &[u32], slots: usize, was_paged: bool) -> usize {
    let (mut maximum, mut paged) = (0, 0_u64);
    for (index, &max) in maxima.iter().enumerate() {
        maximum = maximum.max(max);
        paged += u64::from(max) * PAGE_COLUMNS.min(slots - index * PAGE_COLUMNS) as u64;
    }
    let flat = u64::from(maximum) * slots as u64;
    // Avoid extra bindings for dense data and repeated repacking near the threshold.
    let saves_enough = if was_paged { paged * 8 < flat * 7 } else { paged * 4 < flat * 3 };
    if saves_enough { PAGE_COLUMNS.min(slots).max(1) } else { slots.max(1) }
}

fn pending_pages(p: &SpectrogramParams, columns: usize) -> impl Iterator<Item = usize> + '_ {
    let first = (p.write_slot + p.ring_capacity - p.pending_uploads.len() as u32) % p.ring_capacity;
    (0..p.pending_uploads.len()).filter_map(move |offset| {
        let slot = (first as usize + offset) % p.ring_capacity as usize;
        (offset == 0 || slot.is_multiple_of(columns)).then_some(slot / columns)
    })
}

fn ring_layout(p: &SpectrogramParams, maxima: &[u32], was_paged: bool) -> RingLayout {
    RingLayout {
        kind: p.col_kind,
        stride: col_byte_stride(p.col_kind, p.points_per_column),
        slots: u64::from(p.ring_capacity),
        page_columns: point_page_columns(maxima, p.slot_counts.len(), was_paged),
    }
}

fn can_reuse_ring(current: RingLayout, requested: RingLayout, copy_pending: bool) -> bool {
    current == requested && !copy_pending
}

// Carry page addressing in vertex_index without per-page uniform bindings.
fn page_vertex(first_slot: u32, stride: u32, shift: u32) -> u32 {
    let tag = (u64::from(stride) << shift) | u64::from(first_slot);
    u32::try_from(tag * 4).expect("history budget bounds the page tag")
}

fn point_draws(counts: &[u32], stride: u32) -> impl Iterator<Item = std::ops::Range<u32>> + '_ {
    let mut slot = 0;
    std::iter::from_fn(move || {
        while slot < counts.len() && counts[slot] == 0 { slot += 1; }
        if slot == counts.len() || stride == 0 { return None }
        let count = counts[slot].min(stride);
        let first = slot as u32 * stride;
        slot += 1;
        let end = if count == stride {
            while slot < counts.len() && counts[slot] >= stride { slot += 1; }
            slot as u32 * stride
        } else { first + count };
        Some(first..end)
    })
}

fn equal_count_batches(counts: &[u32], limit: u32) -> impl Iterator<Item = (std::ops::Range<u32>, u32)> + '_ {
    let mut slot = 0;
    std::iter::from_fn(move || {
        while slot < counts.len() && counts[slot] == 0 { slot += 1; }
        if slot == counts.len() || limit == 0 { return None }
        let first = slot;
        let count = counts[slot].min(limit);
        slot += 1;
        while slot < counts.len() && counts[slot].min(limit) == count { slot += 1; }
        Some((first as u32..slot as u32, count))
    })
}

struct PointPage {
    stride: u32,
    buf: wgpu::Buffer,
    indexed_bg: Option<wgpu::BindGroup>,
}

impl PointPage {
    fn new(device: &wgpu::Device, bgl: &wgpu::BindGroupLayout, ub: &wgpu::Buffer, counts: &[u32]) -> Option<Self> {
        let stride = counts.iter().copied().max().filter(|&n| n > 0)?;
        let buf = create_buffer(
            device, "Spectrogram point page",
            col_byte_stride(ColumnKind::Reassigned, stride) * counts.len() as u64,
            wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        );
        let indexed_bg = (buf.size() <= u64::from(device.limits().max_storage_buffer_binding_size)).then(||
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Spectrogram indexed page"), layout: bgl, entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: buf.as_entire_binding() },
                ],
            })
        );
        Some(Self { stride, buf, indexed_bg })
    }
}

enum RingStorage {
    Reassigned { pages: Vec<Option<PointPage>>, bg: wgpu::BindGroup },
    Classic { buf: wgpu::Buffer, bg: wgpu::BindGroup },
}

struct ColumnRing {
    layout: RingLayout,
    storage: RingStorage,
}

impl ColumnRing {
    fn column(&self, slot: usize) -> Option<(&wgpu::Buffer, u64, u64)> {
        match &self.storage {
            RingStorage::Classic { buf, .. } => Some((buf, slot as u64 * self.layout.stride, self.layout.stride)),
            RingStorage::Reassigned { pages, .. } => {
                let page = pages[slot / self.layout.page_columns].as_ref()?;
                let stride = col_byte_stride(ColumnKind::Reassigned, page.stride);
                Some((&page.buf, (slot % self.layout.page_columns) as u64 * stride, stride))
            }
        }
    }
}

struct AccumTarget {
    dirty: AtomicBool,
    tex: wgpu::Texture,
    view: wgpu::TextureView,
    bg: wgpu::BindGroup,
}

struct Resources {
    last_used: u64,
    uniform_buf: wgpu::Buffer,
    uniform_cache: Uniforms,
    ring: ColumnRing,
    accum: Option<AccumTarget>,
    classic_upload_scratch: Vec<u16>,
    page_maxima: Vec<u32>,
    indices: Option<(u32, wgpu::Buffer)>,
}

impl Resources {
    fn new(device: &wgpu::Device, bgls: Bgls<'_>, p: &SpectrogramParams) -> Self {
        let uniform_buf = create_buffer(
            device,
            "Spectrogram UB",
            std::mem::size_of::<Uniforms>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let page_maxima: Vec<_> = p.slot_counts.chunks(PAGE_COLUMNS)
            .map(|counts| counts.iter().copied().max().unwrap_or(0)).collect();
        let ring = create_ring(device, bgls, &uniform_buf, p, ring_layout(p, &page_maxima, false));

        let mut res = Self {
            last_used: 0,
            uniform_buf,
            uniform_cache: Uniforms::zeroed(),
            ring,
            accum: None,
            classic_upload_scratch: Vec::new(),
            page_maxima,
            indices: None,
        };
        res.fit_indices(device, p);
        res
    }

    fn fit_indices(&mut self, device: &wgpu::Device, p: &SpectrogramParams) {
        let RingStorage::Reassigned { pages, .. } = &self.ring.storage else { self.indices = None; return };
        let needed = self.page_maxima.iter().copied().max().unwrap_or(0);
        let capacity = needed.max(1).next_power_of_two().min(p.points_per_column);
        let bytes: u64 = pages.iter().flatten().map(|page| page.buf.size()).sum();
        // Bound index overhead to 1/32 of point storage, including after shrinkage.
        if needed == 0 || u64::from(capacity) * 24 * 32 > bytes
            || !pages.iter().flatten().any(|page| page.indexed_bg.is_some())
        {
            self.indices = None;
            return;
        }
        if self.indices.as_ref().is_some_and(|(capacity, indices)| needed <= *capacity
            && *capacity <= needed.saturating_mul(4) && indices.size() * 32 <= bytes)
        {
            return;
        }
        let indices: Vec<u32> = (0..capacity)
            .flat_map(|point| [0, 1, 2, 2, 1, 3].map(|corner| point * 4 + corner)).collect();
        self.indices = Some((capacity, device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Spectrogram quad indices"), contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        })));
    }

    fn resize_ring(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bgls: Bgls<'_>,
        p: &SpectrogramParams,
    ) {
        let old = self.ring.layout;
        let same_shape = old.kind == p.col_kind && old.slots == u64::from(p.ring_capacity)
            && old.stride == col_byte_stride(p.col_kind, p.points_per_column);
        if same_shape && p.copy_plan.is_none()
            && (p.col_kind == ColumnKind::Classic
                || (p.pending_uploads.is_empty() && p.col_count == self.uniform_cache.col_count))
        {
            return;
        }
        if !same_shape || p.copy_plan.is_some() || p.col_count < self.uniform_cache.col_count {
            self.page_maxima = p.slot_counts.chunks(PAGE_COLUMNS)
                .map(|counts| counts.iter().copied().max().unwrap_or(0)).collect();
        } else if p.col_kind == ColumnKind::Reassigned {
            let mut changed = false;
            for index in pending_pages(p, PAGE_COLUMNS) {
                let maximum = p.slot_counts[index * PAGE_COLUMNS..].iter()
                    .take(PAGE_COLUMNS).copied().max().unwrap_or(0);
                changed |= self.page_maxima[index] != maximum;
                self.page_maxima[index] = maximum;
            }
            if !changed { return }
        }
        let layout = ring_layout(p, &self.page_maxima, old.page_columns < old.slots as usize);
        let repage = old.page_columns != layout.page_columns && old.kind == layout.kind
            && old.stride == layout.stride && old.slots == layout.slots
            && p.col_count >= self.uniform_cache.col_count && p.pending_uploads.len() < p.col_count as usize;
        let identity = if repage && p.copy_plan.is_none() { (0..p.ring_capacity).collect() } else { Vec::new() };
        let copy_plan = p.copy_plan.as_ref().or_else(|| (!identity.is_empty()).then_some(&identity))
            .filter(|copies| copies.iter().any(|&dst| dst < p.ring_capacity));
        if can_reuse_ring(self.ring.layout, layout, copy_plan.is_some())
            && p.col_count >= self.uniform_cache.col_count
        {
            self.resize_point_pages(device, queue, bgls[3], p);
            self.fit_indices(device, p);
            return;
        }

        let new_ring = create_ring(device, bgls, &self.uniform_buf, p, layout);
        if let Some(copies) = copy_plan {
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            for (src, &dst) in copies.iter().enumerate().filter(|(_, dst)| **dst < p.ring_capacity) {
                if let (Some((src_buf, src_offset, src_stride)), Some((dst_buf, dst_offset, dst_stride))) =
                    (self.ring.column(src), new_ring.column(dst as usize))
                {
                    let bytes = match layout.kind {
                        ColumnKind::Reassigned => col_byte_stride(layout.kind, p.slot_counts[dst as usize]),
                        ColumnKind::Classic => layout.stride,
                    }.min(src_stride).min(dst_stride);
                    if bytes > 0 {
                        encoder.copy_buffer_to_buffer(src_buf, src_offset, dst_buf, dst_offset, bytes);
                    }
                }
            }
            queue.submit(std::iter::once(encoder.finish()));
        }
        self.ring = new_ring;
        self.fit_indices(device, p);
    }

    fn resize_point_pages(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, bgl: &wgpu::BindGroupLayout, p: &SpectrogramParams) {
        let RingStorage::Reassigned { pages, .. } = &mut self.ring.storage else { return };
        let columns = self.ring.layout.page_columns;
        let mut encoder = None;
        for index in pending_pages(p, columns) {
            let page = &mut pages[index];
            let start = index * columns;
            let counts = &p.slot_counts[start..(start + columns).min(p.slot_counts.len())];
            let needed = if columns == PAGE_COLUMNS { self.page_maxima[index] }
                else { self.page_maxima.iter().copied().max().unwrap_or(0) };
            let current = page.as_ref().map_or(0, |page| page.stride);
            if needed <= current && current <= needed.saturating_mul(4) { continue }
            let new_page = PointPage::new(device, bgl, &self.uniform_buf, counts);
            if let (Some(old), Some(new)) = (page.as_ref(), new_page.as_ref()) {
                let encoder = encoder.get_or_insert_with(|| device.create_command_encoder(&Default::default()));
                for (slot, &count) in counts.iter().enumerate() {
                    let bytes = col_byte_stride(ColumnKind::Reassigned, count.min(old.stride));
                    if bytes > 0 {
                        encoder.copy_buffer_to_buffer(&old.buf, slot as u64 * col_byte_stride(ColumnKind::Reassigned, old.stride),
                            &new.buf, slot as u64 * col_byte_stride(ColumnKind::Reassigned, new.stride), bytes);
                    }
                }
            }
            *page = new_page;
        }
        if let Some(encoder) = encoder { queue.submit([encoder.finish()]); }
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
        self.accum = Some(AccumTarget { dirty: AtomicBool::new(true), tex, view, bg });
    }

    fn upload_pending(&mut self, queue: &wgpu::Queue, p: &SpectrogramParams) {
        let write = |slot: u32, data: &[u8]| {
            if let Some((buf, offset, _)) = self.ring.column(slot as usize) {
                queue.write_buffer(buf, offset, data);
            }
        };
        let first = (p.write_slot + p.ring_capacity - p.pending_uploads.len() as u32)
            % p.ring_capacity;
        let slot = |offset: usize| (first + offset as u32) % p.ring_capacity;
        match p.col_kind {
            ColumnKind::Reassigned => {
                for (offset, column) in p.pending_uploads.iter().enumerate() {
                    if let SpectrogramColumn::Reassigned(points) = column
                        && !points.is_empty()
                    {
                        write(slot(offset), bytemuck::cast_slice(points));
                    }
                }
            }
            ColumnKind::Classic => {
                let u16_stride = (self.ring.layout.stride / 2) as usize;
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
    layout: RingLayout,
) -> ColumnRing {
    let copy = wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
    let storage = match layout.kind {
        ColumnKind::Reassigned => {
            let pages: Vec<_> = p.slot_counts.chunks(layout.page_columns)
                .map(|values| PointPage::new(device, bgls[3], uniform_buf, values)).collect();
            let bg = make_bind_group(device, bgls[0], uniform_buf, None, None);
            RingStorage::Reassigned { pages, bg }
        }
        ColumnKind::Classic => {
            let buf = create_buffer(device, "Spectrogram mag ring", layout.stride * layout.slots, copy | wgpu::BufferUsages::STORAGE);
            let bg = make_bind_group(device, bgls[2], uniform_buf, Some(&buf), None);
            RingStorage::Classic { buf, bg }
        }
    };
    ColumnRing { layout, storage }
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
    fn page_tags_fit_the_history_budget_and_preserve_corners() {
        use super::super::processor::{MAX_SPECTROGRAM_HISTORY_COLUMNS, history_columns};
        for bins in 1..=(crate::util::audio::MAX_DSP_BUFFER_LEN / 2 + 1) as u32 {
            let capacity = history_columns(ColumnKind::Reassigned, bins, MAX_SPECTROGRAM_HISTORY_COLUMNS) as u32;
            let shift = capacity.next_power_of_two().trailing_zeros();
            let mask = capacity.next_power_of_two() - 1;
            let vertex = page_vertex(capacity - 1, bins, shift);
            for corner in 0..4 {
                let tag = (vertex + corner) / 4;
                assert_eq!(tag & mask, capacity - 1);
                assert_eq!(tag >> shift, bins);
                assert_eq!((vertex + corner) % 4, corner);
            }
        }
    }

    #[test]
    fn draw_ranges_cover_only_live_points_in_slot_order() {
        for len in 0..=7 {
            for code in 0..5_usize.pow(len) {
                let mut code = code;
                let counts: Vec<u32> = (0..len).map(|_| { let n = code % 5; code /= 5; n as u32 }).collect();
                let actual: Vec<_> = point_draws(&counts, 4).flatten().collect();
                let expected: Vec<_> = counts.iter().enumerate()
                    .flat_map(|(slot, &count)| slot as u32 * 4..slot as u32 * 4 + count).collect();
                assert_eq!(actual, expected, "{counts:?}");
            }
        }
        assert_eq!(point_draws(&[4, 4, 0, 2, 4, 4], 4).collect::<Vec<_>>(), [0..8, 12..14, 16..24]);
        assert!(point_draws(&[0; 32], 0).next().is_none());
    }

    #[test]
    fn equal_byte_capacity_does_not_reuse_a_different_ring_layout() {
        let current = RingLayout {
            kind: ColumnKind::Classic,
            stride: col_byte_stride(ColumnKind::Classic, 513),
            slots: 513,
            page_columns: 1,
        };
        let requested = RingLayout {
            kind: ColumnKind::Classic,
            stride: col_byte_stride(ColumnKind::Classic, 1025),
            slots: 257,
            page_columns: 1,
        };

        assert_eq!(current.stride * current.slots, requested.stride * requested.slots);
        assert!(!can_reuse_ring(current, requested, false));
    }
}
