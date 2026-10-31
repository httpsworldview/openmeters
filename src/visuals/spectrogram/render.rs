// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use bytemuck::{Pod, Zeroable};
use iced::Rectangle;
use iced::advanced::graphics::Viewport;
use iced_wgpu::primitive::{self, Primitive};
use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::sync::{Arc, Mutex, Weak};
use wgpu::util::DeviceExt as _;

use crate::visuals::render::common::{
    QuarterTurn, RenderPipelineSpec, begin_pass, create_buffer, create_render_pipeline,
    create_shader_module,
};

use super::processor::{ColumnKind, SpectrogramColumn, SpectrogramPoint, col_byte_stride};
use crate::util::audio::FrequencyScale;

pub const SPECTROGRAM_PALETTE_SIZE: usize = crate::visuals::palettes::spectrogram::SIZE;

const ACCUM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;
const PAGE_COLUMNS: usize = 64;
const PAGE_SNAP: u32 = 8;
// Columns kept right of the view in the accumulation ring, so that a scroll of up to this many
// columns clears only texels no splat reaches.
const RING_MARGIN_COLUMNS: f32 = 16.0;
const SPLAT_STRIDE: usize = 64;
// Instanced column runs pad at most 1/RUN_PADDING of their points and span at most RUN_QUADS
// quads, past which one draw measured slower than several.
const RUN_PADDING: u64 = 8;
const RUN_QUADS: u64 = 1 << 16;
const NO_SPAN: [f32; 2] = [f32::INFINITY, f32::NEG_INFINITY];
// Set in a draw's instance indices to splat one texture width left, onto the wrapped part
// of the ring.
const WRAP_BIT: u32 = 1 << 31;
// Points per indexed draw: a 2^14 + 1 bin column, so longer 2^k + 1 bin columns split into
// whole draws with no one-point remainder.
const QUAD_BATCH: u32 = (1 << 14) + 1;

// Work not yet on the GPU. Shared with the state and drained by prepare, so a frame that is
// drawn but never prepared loses nothing: its work joins the next one, and copy plans compose.
#[derive(Debug, Default)]
pub struct PendingColumns {
    pub uploads: VecDeque<SpectrogramColumn>,
    // Old GPU slot -> new slot; u32::MAX discards.
    pub copy_plan: Option<Vec<u32>>,
}

#[derive(Debug)]
pub struct SpectrogramParams {
    pub key: u64,
    pub bounds: Rectangle,
    pub ring_capacity: u32,
    pub points_per_column: u32,
    pub col_count: u32,
    pub write_slot: u32,
    pub pending: Arc<Mutex<PendingColumns>>,
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
    pub turn: QuarterTurn,
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
        let scale_factor = vp.scale_factor();
        let bgls = &pipeline.bgls;
        let res = pipeline
            .instances
            .entry(self.key)
            .or_insert_with(|| Resources::new(device, bgls, self));
        if res.ring.layout.kind != self.col_kind {
            *res = Resources::new(device, bgls, self);
        }
        res.history = Arc::downgrade(&self.pending);
        let pending = std::mem::take(&mut *crate::util::unpoison(self.pending.lock()));
        res.resize_ring(device, queue, bgls, self, &pending);
        res.resize_accum(device, &bgls[0], self, scale_factor);
        res.upload_pending(queue, self, &pending.uploads);
        let mut uniforms = Uniforms::from_params(self, vp.logical_size().into(), scale_factor);
        uniforms.page_slot_mask = (res.ring.layout.page_columns as u32).next_power_of_two() - 1;
        let (old, hl) = (&res.uniform_cache, uniforms.history_length);
        let columns = pending.uploads.len() as u32;
        let appended = pending.copy_plan.is_none()
            && uniforms.accumulation_key() == old.accumulation_key()
            && uniforms.col_count == (old.col_count + columns).min(hl)
            && uniforms.newest_col == (old.newest_col + columns) % hl;
        res.plan(&mut uniforms, appended.then_some(columns));
        if uniforms != res.uniform_cache {
            queue.write_buffer(&res.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
            res.uniform_cache = uniforms;
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
        let (index, bg) = match &r.ring.storage {
            RingStorage::Reassigned { .. } => {
                let Some(accum) = r.accum.as_ref() else {
                    return;
                };
                if !self
                    .slot_counts
                    .iter()
                    .take(r.uniform_cache.live_columns() as usize)
                    .any(|&count| count > 0)
                {
                    return;
                }
                let state = &mut *accum.state.lock().unwrap();
                let counts = &self.slot_counts;
                match std::mem::replace(&mut state.pending, Pending::Idle) {
                    Pending::Idle => {}
                    Pending::Redraw => r.accumulate(pipeline, encoder, accum, None, counts),
                    Pending::Scroll { scroll, .. } => {
                        r.accumulate(pipeline, encoder, accum, Some(&scroll), counts);
                    }
                }
                state.origin = r.uniform_cache.ring_origin;
                state.phase = r.uniform_cache.scroll_phase;
                (1, &accum.bg)
            }
            RingStorage::Classic { bg, .. } => {
                if self.points_per_column < 2 {
                    return;
                }
                (2, bg)
            }
        };
        let mut pass = begin_pass(
            encoder,
            target,
            Some(clip),
            "Spectrogram pass",
            wgpu::LoadOp::Load,
        );
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
    ring_origin: u32, // reassigned: frame x is stored at texel (x + ring_origin) % ring_width
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
    // Reassigned: splats sit this far left of their ideal x, so scrolls move whole pixels.
    scroll_phase: f32,
    // Reassigned: accumulation texture width, the view plus a margin.
    ring_width: u32,
    // Reassigned: frame x where the resolve begins showing the accumulation.
    image_start: f32,
    _pad: u32,
    // (pos1, pos2, pos3, spread0), (spread1, spread2, spread3, spread4).
    // Stops 0 and 4 are constant 0.0 / 1.0 and live in the shader.
    stops: [[f32; 4]; 2],
    palette: [[f32; 4]; SPECTROGRAM_PALETTE_SIZE],
}

// Rust layout checks against expected WGSL values.
const _: () = assert!(std::mem::size_of::<Uniforms>() == 224);
const _: () = assert!(std::mem::offset_of!(Uniforms, ring_origin) == 28);
const _: () = assert!(std::mem::offset_of!(Uniforms, reassigned_power_scale) == 88);
const _: () = assert!(std::mem::offset_of!(Uniforms, page_slot_mask) == 92);
const _: () = assert!(std::mem::offset_of!(Uniforms, scroll_phase) == 96);
const _: () = assert!(std::mem::offset_of!(Uniforms, stops) == 112);
const _: () = assert!(std::mem::offset_of!(Uniforms, palette) == 144);

impl Uniforms {
    // Unrotated frame size, as the shader's extents().
    fn extents(&self) -> [f32; 2] {
        let size = [self.bounds[2], self.bounds[3]];
        if self.rotation.is_multiple_of(2) {
            size
        } else {
            [size[1], size[0]]
        }
    }

    fn live_columns(&self) -> u32 {
        self.col_count.min(self.history_length)
    }

    fn accumulation_key(&self) -> impl PartialEq {
        (
            self.freq_axis,
            self.freq_scale,
            self.history_length,
            self.extents(),
            self.uv_y_range,
            self.scale_factor,
            self.tilt_db,
        )
    }

    fn from_params(p: &SpectrogramParams, viewport: [f32; 2], sf: f32) -> Self {
        let freq_lo = p.freq_scale.scale(p.freq_min);
        let freq_hi = p.freq_scale.scale(p.freq_max);
        let hl = p.ring_capacity;
        let [_, pos1, pos2, pos3, _] = p.stop_positions;
        let [spread0, spread1, spread2, spread3, spread4] = p.stop_spreads;
        Self {
            freq_axis: [freq_lo, 1.0 / (freq_hi - freq_lo)],
            freq_scale: p.freq_scale as u32,
            points_per_col: if p.col_kind == ColumnKind::Reassigned {
                hl.next_power_of_two().trailing_zeros()
            } else {
                p.points_per_column
            },
            history_length: p.ring_capacity,
            col_count: p.col_count,
            rotation: u32::from(p.turn.turns()),
            ring_origin: 0,
            bounds: [
                p.bounds.x * sf,
                p.bounds.y * sf,
                p.bounds.width.max(1.0) * sf,
                p.bounds.height.max(1.0) * sf,
            ],
            clip_scale: viewport.map(|extent| 2.0 / (extent * sf).max(1.0)),
            uv_y_range: p.uv_y_range,
            scale_factor: sf,
            floor_db: p.floor_db,
            tilt_db: p.tilt_db,
            newest_col: (p.write_slot + hl - 1) % hl,
            inv_uv_range: 1.0 / (p.uv_y_range[1] - p.uv_y_range[0]).max(1e-12),
            bin_hz: p.bin_hz,
            reassigned_power_scale: p.reassigned_power_scale,
            page_slot_mask: 0,
            scroll_phase: 0.0,
            ring_width: 0,
            image_start: 0.0,
            _pad: 0,
            stops: [
                [pos1, pos2, pos3, spread0],
                [spread1, spread2, spread3, spread4],
            ],
            palette: p.palette,
        }
    }
}

pub struct Pipeline {
    pipelines: [wgpu::RenderPipeline; 4],
    quads: wgpu::Buffer,
    bgls: Bgls,
    instances: HashMap<u64, Resources>,
}

impl primitive::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = create_shader_module(
            device,
            "Spectrogram shader",
            include_str!("../render/shaders/spectrogram.wgsl"),
        );
        let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty,
            count: None,
        };
        let buffer = |ty| wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let stages = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let storage = buffer(wgpu::BufferBindingType::Storage { read_only: true });
        let accum = wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };
        let uniform_entry = entry(0, stages, buffer(wgpu::BufferBindingType::Uniform));
        let bgls = [
            entry(1, stages, accum),
            entry(2, stages, storage),
            entry(3, wgpu::ShaderStages::VERTEX, storage),
        ]
        .map(|data_entry| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Spectrogram BGL"),
                entries: &[uniform_entry, data_entry],
            })
        });
        let [resolve_bgl, classic_bgl, points_bgl] = &bgls;

        let additive = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let accum_spec = RenderPipelineSpec {
            label: "Spectrogram accumulation pipeline",
            shader: &shader,
            vertex_entry: "vs_accum",
            topology: wgpu::PrimitiveTopology::TriangleList,
            fragment_entry: "fs_accum",
            buffers: &[],
            bind_group_layouts: &[points_bgl],
            blend: Some(wgpu::BlendState {
                color: additive,
                alpha: additive,
            }),
            write_mask: wgpu::ColorWrites::RED | wgpu::ColorWrites::GREEN,
        };
        let clear_spec = RenderPipelineSpec {
            label: "Spectrogram accumulation clear",
            vertex_entry: "vs_fill",
            fragment_entry: "fs_clear",
            bind_group_layouts: &[],
            blend: None,
            ..accum_spec
        };
        let indices: Vec<u32> = (0..QUAD_BATCH)
            .flat_map(|quad| [0, 1, 2, 2, 1, 3].map(|corner| quad * 4 + corner))
            .collect();
        let quads = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Spectrogram quad indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
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
                create_render_pipeline(device, ACCUM_FORMAT, accum_spec),
                pipeline(
                    "Spectrogram resolve pipeline",
                    "vs_resolve",
                    "fs_resolve",
                    resolve_bgl,
                ),
                pipeline(
                    "Spectrogram classic pipeline",
                    "vs_classic",
                    "fs_classic",
                    classic_bgl,
                ),
                create_render_pipeline(device, ACCUM_FORMAT, clear_spec),
            ],
            quads,
            bgls,
            instances: HashMap::new(),
        }
    }

    // The GPU ring is the only copy of uploaded columns, so it lives as long as its history,
    // even while its window does not present; disabling the spectrogram drops it.
    fn trim(&mut self) {
        self.instances
            .retain(|_, instance| instance.history.strong_count() > 0);
    }
}

type Bgls = [wgpu::BindGroupLayout; 3];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RingLayout {
    kind: ColumnKind,
    stride: u64,
    slots: u64,
    page_columns: usize,
}

fn point_page_columns(
    maxima: &[u32],
    slots: usize,
    points_per_column: u32,
    was_paged: bool,
    binding: u64,
) -> usize {
    let (mut maximum, mut paged) = (0, 0_u64);
    for (index, &max) in maxima.iter().enumerate() {
        maximum = maximum.max(max);
        paged += u64::from(max) * PAGE_COLUMNS.min(slots - index * PAGE_COLUMNS) as u64;
    }
    let flat = u64::from(maximum) * slots as u64;
    // Avoid extra bindings for dense data and repeated repacking near the threshold.
    let saves_enough = if was_paged {
        paged * 8 < flat * 7
    } else {
        paged * 4 < flat * 3
    };
    // Full columns per storage binding, so any page stays bindable whatever arrives.
    let fit = binding / col_byte_stride(ColumnKind::Reassigned, points_per_column).max(1);
    if !saves_enough && slots as u64 <= fit {
        return slots.max(1);
    }
    let columns = PAGE_COLUMNS.min(slots).max(1);
    if columns as u64 <= fit {
        columns
    } else {
        // page_slot_mask addresses slots within power-of-two pages.
        1 << fit.max(1).ilog2()
    }
}

fn ring_layout(p: &SpectrogramParams, was_paged: bool, binding: u64) -> RingLayout {
    let maxima: Vec<_> = p
        .slot_counts
        .chunks(PAGE_COLUMNS)
        .map(|counts| counts.iter().copied().max().unwrap_or(0))
        .collect();
    RingLayout {
        kind: p.col_kind,
        stride: col_byte_stride(p.col_kind, p.points_per_column),
        slots: u64::from(p.ring_capacity),
        page_columns: point_page_columns(
            &maxima,
            p.slot_counts.len(),
            p.points_per_column,
            was_paged,
            binding,
        ),
    }
}

enum RingUpdate<'a> {
    Unchanged,
    ResizePages,
    Replace {
        layout: RingLayout,
        copies: Option<Cow<'a, [u32]>>,
    },
}

impl RingLayout {
    fn plan_update<'a>(
        self,
        p: &SpectrogramParams,
        pending: &'a PendingColumns,
        previous_count: u32,
        binding: u64,
    ) -> RingUpdate<'a> {
        let stride = col_byte_stride(p.col_kind, p.points_per_column);
        let same_shape = (self.kind, self.stride, self.slots)
            == (p.col_kind, stride, u64::from(p.ring_capacity));
        let reset = p.col_count < previous_count;
        if same_shape
            && pending.copy_plan.is_none()
            && (p.col_kind == ColumnKind::Classic
                || (pending.uploads.is_empty() && p.col_count == previous_count))
        {
            return RingUpdate::Unchanged;
        }
        let layout = ring_layout(p, self.page_columns < self.slots as usize, binding);
        let copies = match &pending.copy_plan {
            Some(copies) => Some(Cow::Borrowed(copies.as_slice())),
            None if self.page_columns != layout.page_columns
                && same_shape
                && !reset
                && pending.uploads.len() < p.col_count as usize =>
            {
                Some(Cow::Owned((0..p.ring_capacity).collect()))
            }
            None => None,
        }
        .filter(|copies| copies.iter().any(|&dst| dst < p.ring_capacity));
        if self == layout && copies.is_none() && !reset {
            RingUpdate::ResizePages
        } else {
            RingUpdate::Replace { layout, copies }
        }
    }
}

// Carry page addressing in instance_index without per-page uniform bindings.
fn page_tag(first_slot: u32, stride: u32, shift: u32) -> u32 {
    let tag = (u64::from(stride) << shift) | u64::from(first_slot);
    u32::try_from(tag)
        .ok()
        .filter(|&tag| tag < WRAP_BIT)
        .expect("history budget bounds the page tag")
}

// Indexed draws as (columns, quads): runs of columns drawn at their largest point count, the
// shader culling the zeroed padding past each column's points, split at QUAD_BATCH. Longer
// columns draw alone, so each column's points stay in order.
fn quad_draws(counts: &[u32], limit: u32) -> impl Iterator<Item = (Range<u32>, Range<u32>)> + '_ {
    column_runs(counts, limit).flat_map(|(columns, count)| {
        (0..count)
            .step_by(QUAD_BATCH as usize)
            .map(move |first| (columns.clone(), first..count.min(first + QUAD_BATCH)))
    })
}

// Runs of non-empty columns as (columns, largest count), within RUN_PADDING and RUN_QUADS.
fn column_runs(counts: &[u32], limit: u32) -> impl Iterator<Item = (Range<u32>, u32)> + '_ {
    let mut slot = 0;
    std::iter::from_fn(move || {
        while slot < counts.len() && counts[slot] == 0 {
            slot += 1;
        }
        if slot == counts.len() || limit == 0 {
            return None;
        }
        let first = slot;
        let mut longest = counts[slot].min(limit);
        let mut points = u64::from(longest);
        slot += 1;
        while longest <= QUAD_BATCH && slot < counts.len() {
            let count = counts[slot].min(limit);
            let (wider, total) = (longest.max(count), points + u64::from(count));
            let drawn = u64::from(wider) * (slot - first + 1) as u64;
            let padded = (drawn - total) * RUN_PADDING > total;
            if count == 0 || wider > QUAD_BATCH || padded || drawn > RUN_QUADS {
                break;
            }
            (longest, points) = (wider, total);
            slot += 1;
        }
        Some((first as u32..slot as u32, longest))
    })
}

struct PointPage {
    stride: u32,
    // Per slot bound on the points it holds; past them it holds zeros.
    stored: Vec<u32>,
    buf: wgpu::Buffer,
    bg: wgpu::BindGroup,
}

impl PointPage {
    // A page for columns of `counts` points, of `full` possible, holding at most `counts`;
    // None when none has points. Strides within 1/PAGE_SNAP of full snap to it: dense columns
    // vary just below full, so exact strides would keep growing, and each growth reallocates
    // and copies the page.
    fn new(
        device: &wgpu::Device,
        bgl: &wgpu::BindGroupLayout,
        ub: &wgpu::Buffer,
        counts: &[u32],
        full: u32,
    ) -> Option<Self> {
        let max = counts.iter().copied().max().filter(|&n| n > 0)?;
        let stride = if max >= full - full / PAGE_SNAP {
            full.max(max)
        } else {
            max
        };
        let buf = create_buffer(
            device,
            "Spectrogram point page",
            col_byte_stride(ColumnKind::Reassigned, stride) * counts.len() as u64,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        );
        let bg = make_bind_group(device, bgl, ub, 3, buf.as_entire_binding());
        Some(Self {
            stride,
            stored: counts.to_vec(),
            buf,
            bg,
        })
    }

    // Stores a column's points in sweeps of every SPLAT_STRIDE-th one, then zeroes what earlier
    // columns left past them: instanced runs draw that padding and the shader culls it.
    // Neighbouring bins often share a pixel, and blending into one pixel serializes, most on
    // short views.
    fn write(&mut self, queue: &wgpu::Queue, slot: usize, points: &[SpectrogramPoint]) {
        const POINT_BYTES: usize = std::mem::size_of::<SpectrogramPoint>();
        let count = points.len().min(self.stride as usize);
        let len = count.max(self.stored[slot] as usize) * POINT_BYTES;
        let offset = slot as u64 * col_byte_stride(ColumnKind::Reassigned, self.stride);
        let Some(size) = wgpu::BufferSize::new(len as u64) else {
            return;
        };
        let Some(mut view) = queue.write_buffer_with(&self.buf, offset, size) else {
            return;
        };
        // Reading the view warns and reads mapped memory, so size it from the count instead.
        let (filled, stale) = view.split_at_mut(count * POINT_BYTES);
        // Whole-array stores, as a slice copy per point compiled to a call in some builds.
        let filled: &mut [[u8; POINT_BYTES]] = bytemuck::cast_slice_mut(filled);
        // Index loops: an iterator chain here measured 2-3x slower.
        let mut dst = 0;
        for first in 0..SPLAT_STRIDE {
            let mut index = first;
            while index < points.len() && dst < filled.len() {
                filled[dst] = bytemuck::cast(points[index]);
                dst += 1;
                index += SPLAT_STRIDE;
            }
        }
        stale.fill(0);
        self.stored[slot] = count as u32;
    }
}

enum RingStorage {
    Reassigned {
        pages: Vec<Option<PointPage>>,
    },
    Classic {
        buf: wgpu::Buffer,
        bg: wgpu::BindGroup,
    },
}

struct ColumnRing {
    layout: RingLayout,
    storage: RingStorage,
}

impl ColumnRing {
    fn column(&self, slot: usize) -> Option<(&wgpu::Buffer, u64, u64)> {
        match &self.storage {
            RingStorage::Classic { buf, .. } => {
                Some((buf, slot as u64 * self.layout.stride, self.layout.stride))
            }
            RingStorage::Reassigned { pages } => {
                let page = pages[slot / self.layout.page_columns].as_ref()?;
                let stride = col_byte_stride(ColumnKind::Reassigned, page.stride);
                Some((
                    &page.buf,
                    (slot % self.layout.page_columns) as u64 * stride,
                    stride,
                ))
            }
        }
    }
}

// Accumulation work owed since the last accumulation.
#[derive(Default)]
enum Pending {
    Idle,
    // Clear and splat every live column at ring origin 0.
    #[default]
    Redraw,
    // `columns` appended, and the scroll that splats them in place.
    Scroll {
        columns: u32,
        scroll: Scroll,
    },
}

// Moves the ring origin to `origin` and the splats to `phase`, clears the right strip, then
// splats each piece within its scissor.
struct Scroll {
    origin: u32,
    phase: f32,
    pieces: Vec<Piece>,
}

struct Piece {
    // Frame pixel columns, stored one texture width left when `wrapped`.
    px: Range<u32>,
    wrapped: bool,
    // The right strip is cleared; the rest of the image keeps its scrolled content.
    clear: bool,
    // Slot runs, oldest first.
    runs: Vec<Range<u32>>,
}

#[derive(Default)]
struct Accumulation {
    pending: Pending,
    // Ring origin and scroll phase left by the last accumulation.
    origin: u32,
    phase: f32,
}

struct AccumTarget {
    state: Mutex<Accumulation>,
    tex: wgpu::Texture,
    view: wgpu::TextureView,
    bg: wgpu::BindGroup,
}

struct Resources {
    history: Weak<Mutex<PendingColumns>>,
    uniform_buf: wgpu::Buffer,
    uniform_cache: Uniforms,
    ring: ColumnRing,
    accum: Option<AccumTarget>,
    classic_upload_scratch: Vec<u16>,
    // Per slot splat x extent in columns, and the farthest right edge among them.
    spans: Vec<[f32; 2]>,
    reach: f32,
}

impl Resources {
    fn new(device: &wgpu::Device, bgls: &Bgls, p: &SpectrogramParams) -> Self {
        let uniform_buf = create_buffer(
            device,
            "Spectrogram UB",
            std::mem::size_of::<Uniforms>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let binding = u64::from(device.limits().max_storage_buffer_binding_size);
        let layout = ring_layout(p, false, binding);
        let ring = create_ring(device, bgls, &uniform_buf, p, layout);

        Self {
            history: Arc::downgrade(&p.pending),
            uniform_buf,
            uniform_cache: Uniforms::zeroed(),
            ring,
            accum: None,
            classic_upload_scratch: Vec::new(),
            spans: vec![NO_SPAN; p.ring_capacity as usize],
            reach: f32::NEG_INFINITY,
        }
    }

    // Plans the next accumulation and sets the ring origin, width, phase and image start it
    // leaves.
    fn plan(&mut self, u: &mut Uniforms, appended: Option<u32>) {
        let (spans, Some(accum)) = (&self.spans, &mut self.accum) else {
            return;
        };
        let (s, width) = (accum.state.get_mut().unwrap(), accum.tex.width());
        // Columns appended since the last accumulation.
        let owed = match s.pending {
            Pending::Idle => Some(0),
            Pending::Redraw => None,
            Pending::Scroll { columns, .. } => Some(columns),
        };
        s.pending = match owed.zip(appended) {
            None => Pending::Redraw,
            Some((0, 0)) => Pending::Idle,
            Some((owed, appended)) => {
                let columns = owed.saturating_add(appended);
                match plan_scroll(spans, self.reach, u, s, width, columns) {
                    Some(scroll) => Pending::Scroll { columns, scroll },
                    None => Pending::Redraw,
                }
            }
        };
        if matches!(s.pending, Pending::Redraw) {
            // Uploads only extend the reach; a redraw is the time to tighten it.
            self.reach = reach(spans);
        }
        (u.ring_origin, u.scroll_phase) = match &s.pending {
            Pending::Idle => (s.origin, s.phase),
            Pending::Redraw => (0, s.phase),
            Pending::Scroll { scroll, .. } => (scroll.origin, scroll.phase),
        };
        u.ring_width = width;
        u.image_start = image_start(u, self.reach);
    }

    // Splats in age order, so a scroll leaves each pixel the sum a full redraw would.
    fn accumulate(
        &self,
        pipeline: &Pipeline,
        encoder: &mut wgpu::CommandEncoder,
        accum: &AccumTarget,
        scroll: Option<&Scroll>,
        counts: &[u32],
    ) {
        let label = "Spectrogram accumulation pass";
        let Some(scroll) = scroll else {
            let u = &self.uniform_cache;
            let (hl, live) = (u.history_length, u.live_columns());
            let oldest = (u.newest_col + 1 + hl - live) % hl;
            let all = [
                oldest..hl.min(oldest + live),
                0..(oldest + live).saturating_sub(hl),
            ];
            let clear = wgpu::LoadOp::Clear(wgpu::Color::BLACK);
            let mut pass = begin_pass(encoder, &accum.view, None, label, clear);
            return self.draw_slots(pipeline, &mut pass, &all, counts, false);
        };
        let (width, height) = (accum.tex.width(), accum.tex.height());
        let scissor = |pass: &mut wgpu::RenderPass<'_>, piece: &Piece| {
            let x = piece.px.start + scroll.origin - if piece.wrapped { width } else { 0 };
            pass.set_scissor_rect(x, 0, piece.px.len() as u32, height);
        };
        // Clear the strip in its own pass: RADV may reorder it with later blending.
        let mut pass = begin_pass(encoder, &accum.view, None, label, wgpu::LoadOp::Load);
        pass.set_pipeline(&pipeline.pipelines[3]);
        for piece in scroll.pieces.iter().filter(|piece| piece.clear) {
            scissor(&mut pass, piece);
            pass.draw(0..3, 0..1);
        }
        drop(pass);
        let mut pass = begin_pass(encoder, &accum.view, None, label, wgpu::LoadOp::Load);
        for piece in scroll.pieces.iter().filter(|piece| !piece.runs.is_empty()) {
            scissor(&mut pass, piece);
            self.draw_slots(pipeline, &mut pass, &piece.runs, counts, piece.wrapped);
        }
    }

    fn draw_slots(
        &self,
        pipeline: &Pipeline,
        pass: &mut wgpu::RenderPass<'_>,
        runs: &[Range<u32>],
        counts: &[u32],
        wrapped: bool,
    ) {
        let RingStorage::Reassigned { pages } = &self.ring.storage else {
            return;
        };
        let per_page = self.ring.layout.page_columns as u32;
        let ppc = self.uniform_cache.points_per_col;
        let wrap = if wrapped { WRAP_BIT } else { 0 };
        pass.set_pipeline(&pipeline.pipelines[0]);
        pass.set_index_buffer(pipeline.quads.slice(..), wgpu::IndexFormat::Uint32);
        let mut bound = None;
        for run in runs {
            let mut slot = run.start;
            while slot < run.end {
                let (index, offset) = (slot / per_page, slot % per_page);
                let end = run.end.min((index + 1) * per_page);
                let counts = &counts[slot as usize..end as usize];
                slot = end;
                let Some(page) = &pages[index as usize] else {
                    continue;
                };
                if bound.replace(index) != Some(index) {
                    pass.set_bind_group(0, &page.bg, &[]);
                }
                let tag = (page_tag(index * per_page, page.stride, ppc) + offset) | wrap;
                for (columns, quads) in quad_draws(counts, page.stride) {
                    // base_vertex selects the column's first point of this draw.
                    let base = (quads.start * 4) as i32;
                    let instances = tag + columns.start..tag + columns.end;
                    pass.draw_indexed(0..quads.len() as u32 * 6, base, instances);
                }
            }
        }
    }

    fn resize_ring(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bgls: &Bgls,
        p: &SpectrogramParams,
        pending: &PendingColumns,
    ) {
        let binding = u64::from(device.limits().max_storage_buffer_binding_size);
        let previous = self.uniform_cache.col_count;
        match self.ring.layout.plan_update(p, pending, previous, binding) {
            RingUpdate::Unchanged => {}
            RingUpdate::ResizePages => self.resize_point_pages(device, queue, &bgls[2], p, pending),
            RingUpdate::Replace { layout, copies } => {
                // Spans may not survive the replace, and scrolls need them to clear evicted
                // splats.
                if let Some(accum) = &mut self.accum {
                    accum.state.get_mut().unwrap().pending = Pending::Redraw;
                }
                let new_ring = create_ring(device, bgls, &self.uniform_buf, p, layout);
                let spans =
                    std::mem::replace(&mut self.spans, vec![NO_SPAN; p.ring_capacity as usize]);
                if let Some(copies) = copies {
                    let mut encoder = device.create_command_encoder(&Default::default());
                    for (src, &dst) in copies
                        .iter()
                        .enumerate()
                        .filter(|(_, dst)| **dst < p.ring_capacity)
                    {
                        self.spans[dst as usize] = spans.get(src).copied().unwrap_or(NO_SPAN);
                        let (
                            Some((src_buf, src_offset, src_stride)),
                            Some((dst_buf, dst_offset, dst_stride)),
                        ) = (self.ring.column(src), new_ring.column(dst as usize))
                        else {
                            continue;
                        };
                        let bytes = match layout.kind {
                            ColumnKind::Reassigned => {
                                col_byte_stride(layout.kind, p.slot_counts[dst as usize])
                            }
                            ColumnKind::Classic => layout.stride,
                        }
                        .min(src_stride)
                        .min(dst_stride);
                        if bytes > 0 {
                            encoder.copy_buffer_to_buffer(
                                src_buf, src_offset, dst_buf, dst_offset, bytes,
                            );
                        }
                    }
                    queue.submit([encoder.finish()]);
                }
                self.ring = new_ring;
            }
        }
    }

    fn resize_point_pages(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bgl: &wgpu::BindGroupLayout,
        p: &SpectrogramParams,
        pending: &PendingColumns,
    ) {
        let RingStorage::Reassigned { pages } = &mut self.ring.storage else {
            return;
        };
        let columns = self.ring.layout.page_columns;
        let uploads = pending.uploads.len();
        let first = (p.write_slot + p.ring_capacity - uploads as u32) % p.ring_capacity;
        let pending_pages = (0..uploads).filter_map(|offset| {
            let slot = (first as usize + offset) % p.ring_capacity as usize;
            (offset == 0 || slot.is_multiple_of(columns)).then_some(slot / columns)
        });
        let mut encoder = None;
        for index in pending_pages {
            let page = &mut pages[index];
            let start = index * columns;
            let counts = &p.slot_counts[start..(start + columns).min(p.slot_counts.len())];
            let needed = counts.iter().copied().max().unwrap_or(0);
            let current = page.as_ref().map_or(0, |page| page.stride);
            if needed <= current && current <= needed.saturating_mul(4) {
                continue;
            }
            let new_page =
                PointPage::new(device, bgl, &self.uniform_buf, counts, p.points_per_column);
            if let (Some(old), Some(new)) = (page.as_ref(), new_page.as_ref()) {
                let encoder = encoder
                    .get_or_insert_with(|| device.create_command_encoder(&Default::default()));
                for (slot, &count) in counts.iter().enumerate() {
                    let bytes = col_byte_stride(ColumnKind::Reassigned, count.min(old.stride));
                    if bytes > 0 {
                        encoder.copy_buffer_to_buffer(
                            &old.buf,
                            slot as u64 * col_byte_stride(ColumnKind::Reassigned, old.stride),
                            &new.buf,
                            slot as u64 * col_byte_stride(ColumnKind::Reassigned, new.stride),
                            bytes,
                        );
                    }
                }
            }
            *page = new_page;
        }
        if let Some(encoder) = encoder {
            queue.submit([encoder.finish()]);
        }
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
        let [width, height] = <[f32; 2]>::from(p.turn.frame_size(p.bounds.size()))
            .map(|value| (value.max(1.0) * scale_factor).ceil() as u32);
        // Room to scroll that many columns past the newest splats, which overhang the view by
        // up to half a column, and the phase.
        let margin = ((RING_MARGIN_COLUMNS + 0.5) * scale_factor).ceil() as u32 + 2;
        let max = device.limits().max_texture_dimension_2d;
        let size = wgpu::Extent3d {
            width: (width + margin).min(max),
            height: height.min(max),
            depth_or_array_layers: 1,
        };
        if self.accum.as_ref().is_some_and(|a| a.tex.size() == size) {
            return;
        }
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Spectrogram power accumulation texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: ACCUM_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        let resource = wgpu::BindingResource::TextureView(&view);
        let bg = make_bind_group(device, layout, &self.uniform_buf, 1, resource);
        self.accum = Some(AccumTarget {
            state: Mutex::default(),
            tex,
            view,
            bg,
        });
    }

    fn upload_pending(
        &mut self,
        queue: &wgpu::Queue,
        p: &SpectrogramParams,
        uploads: &VecDeque<SpectrogramColumn>,
    ) {
        if p.col_kind == ColumnKind::Classic {
            self.classic_upload_scratch
                .resize((self.ring.layout.stride / 2) as usize, 0);
        }
        let first = (p.write_slot + p.ring_capacity - uploads.len() as u32) % p.ring_capacity;
        for (index, column) in uploads.iter().enumerate() {
            let slot = (first + index as u32) % p.ring_capacity;
            if let SpectrogramColumn::Reassigned(points) = column {
                let times = points.iter().map(|point| point.time_offset);
                let (lo, hi) = times.fold(NO_SPAN.into(), |(lo, hi): (f32, f32), t| {
                    (lo.min(t), hi.max(t))
                });
                let span = [lo - 0.5, hi + 0.5];
                self.spans[slot as usize] = span;
                self.reach = self.reach.max(span[1]);
            }
            let slot = slot as usize;
            match (&mut self.ring.storage, column) {
                // Runs skip empty columns, so their slots need no zeroing.
                (RingStorage::Reassigned { pages }, SpectrogramColumn::Reassigned(points))
                    if !points.is_empty() =>
                {
                    let columns = self.ring.layout.page_columns;
                    if let Some(page) = &mut pages[slot / columns] {
                        page.write(queue, slot % columns, points);
                    }
                }
                (RingStorage::Classic { buf, .. }, SpectrogramColumn::Classic(mags))
                    if !mags.is_empty() =>
                {
                    let packed = &mut self.classic_upload_scratch;
                    let written = mags.len().min(packed.len());
                    packed[..written].copy_from_slice(&mags[..written]);
                    packed[written..].fill(0);
                    let offset = slot as u64 * self.ring.layout.stride;
                    queue.write_buffer(buf, offset, bytemuck::cast_slice(packed));
                }
                _ => {}
            }
        }
    }
}

// First frame column of the image, where the resolve starts showing it: evicted columns splat
// only left of it. It is past zero only when the history budget keeps fewer columns than the
// view spans, and it never decreases between redraws, as the reach only grows.
fn image_start(u: &Uniforms, reach: f32) -> f32 {
    // Rightmost evicted splat edge at any phase.
    let edge = u.extents()[0] + 0.5 - (u.history_length as f32 - reach) * u.scale_factor;
    edge.ceil().max(0.0)
}

// Plans scrolling the accumulation `last` left by `columns` appended columns; None when the
// image would not span the scroll, or a full redraw would splat no more columns.
fn plan_scroll(
    spans: &[[f32; 2]],
    reach: f32,
    u: &Uniforms,
    last: &Accumulation,
    width: u32,
    columns: u32,
) -> Option<Scroll> {
    let (hl, sf, live) = (u.history_length, u.scale_factor, u.live_columns());
    // Scroll whole pixels and carry the remainder in the phase, within half a pixel.
    let advance = columns as f32 * sf;
    let shift = (advance - last.phase).round();
    let phase = last.phase + shift - advance;
    let ext_x = u.extents()[0] - phase;
    let start = image_start(u, reach);
    if start >= width as f32 - shift {
        return None;
    }
    let (shift, start) = (shift as u32, start as u32);
    let (origin, right) = ((last.origin + shift) % width, width - shift);
    // Frame columns from `cut` on wrap past the texture edge.
    let cut = width - origin;
    let mut pieces: Vec<Piece> = [(start..right, false), (right..width, true)]
        .into_iter()
        .flat_map(|(px, clear)| {
            [
                (px.start..px.end.min(cut), false),
                (px.start.max(cut)..px.end, true),
            ]
            .map(|(px, wrapped)| Piece {
                px,
                wrapped,
                clear,
                runs: Vec::new(),
            })
        })
        .filter(|piece| !piece.px.is_empty())
        .collect();
    // Only new columns: the ring margin keeps the right strip past older splats.
    for age in (0..columns.min(live)).rev() {
        let slot = (u.newest_col + hl - age) % hl;
        let [lo, hi] = spans[slot as usize].map(|edge| ext_x - (age as f32 - edge) * sf);
        for piece in &mut pieces {
            if lo <= piece.px.end as f32 && hi >= piece.px.start as f32 {
                match piece.runs.last_mut() {
                    Some(run) if run.end == slot => run.end += 1,
                    _ => piece.runs.push(slot..slot + 1),
                }
            }
        }
    }
    let drawn: usize = pieces
        .iter()
        .flat_map(|piece| &piece.runs)
        .map(|run| run.len())
        .sum();
    (drawn < live as usize).then_some(Scroll {
        origin,
        phase,
        pieces,
    })
}

fn reach(spans: &[[f32; 2]]) -> f32 {
    spans
        .iter()
        .map(|span| span[1])
        .fold(f32::NEG_INFINITY, f32::max)
}

fn create_ring(
    device: &wgpu::Device,
    bgls: &Bgls,
    uniform_buf: &wgpu::Buffer,
    p: &SpectrogramParams,
    layout: RingLayout,
) -> ColumnRing {
    let copy = wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
    let storage = match layout.kind {
        ColumnKind::Reassigned => {
            let pages: Vec<_> = p
                .slot_counts
                .chunks(layout.page_columns)
                .map(|counts| {
                    PointPage::new(device, &bgls[2], uniform_buf, counts, p.points_per_column)
                })
                .collect();
            RingStorage::Reassigned { pages }
        }
        ColumnKind::Classic => {
            let buf = create_buffer(
                device,
                "Spectrogram mag ring",
                layout.stride * layout.slots,
                copy | wgpu::BufferUsages::STORAGE,
            );
            let bg = make_bind_group(device, &bgls[1], uniform_buf, 2, buf.as_entire_binding());
            RingStorage::Classic { buf, bg }
        }
    };
    ColumnRing { layout, storage }
}

fn make_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    ub: &wgpu::Buffer,
    binding: u32,
    resource: wgpu::BindingResource<'_>,
) -> wgpu::BindGroup {
    let entries = [(0, ub.as_entire_binding()), (binding, resource)]
        .map(|(binding, resource)| wgpu::BindGroupEntry { binding, resource });
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
    fn pages_and_tags_fit_the_history_budget() {
        use super::super::processor::history_columns;
        for bins in 1..=crate::util::audio::MAX_DSP_BUFFER_LEN as u32 {
            let capacity = history_columns(ColumnKind::Reassigned, bins, u32::MAX as usize) as u32;
            let shift = capacity.next_power_of_two().trailing_zeros();
            let mask = capacity.next_power_of_two() - 1;
            let tag = page_tag(capacity - 1, bins, shift);
            assert_eq!(tag & mask, capacity - 1);
            assert_eq!(tag >> shift, bins);
            let binding = u64::from(wgpu::Limits::default().max_storage_buffer_binding_size);
            let pages = (capacity as usize).div_ceil(PAGE_COLUMNS);
            let mut sparse = vec![0; pages];
            sparse[0] = bins;
            for maxima in [vec![bins; pages], sparse] {
                let columns = point_page_columns(&maxima, capacity as usize, bins, false, binding);
                assert!(
                    col_byte_stride(ColumnKind::Reassigned, bins) * columns as u64 <= binding
                        && (columns == capacity as usize || columns.is_power_of_two()),
                    "{bins} bins, {capacity} slots: {columns} columns per page"
                );
            }
        }
    }

    #[test]
    fn history_margin_keeps_the_whole_view_shown() {
        use super::super::processor::history_columns;
        // Reassigned splats end at most half a column right of their own (processor
        // time_support).
        let reach = 0.5;
        for sf in [0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 3.0] {
            for tenths in 10..20_000 {
                let width = tenths as f32 / 10.0;
                let requested = width.round() as usize;
                let u = Uniforms {
                    bounds: [0.0, 0.0, width * sf, sf],
                    history_length: history_columns(ColumnKind::Reassigned, 1, requested) as u32,
                    scale_factor: sf,
                    ..Uniforms::zeroed()
                };
                assert_eq!(image_start(&u, reach), 0.0, "{width} wide at scale {sf}");
            }
        }
    }

    #[test]
    fn quad_draws_cover_live_points_in_column_order() {
        let long = QUAD_BATCH + 2;
        let small = (0..=7).flat_map(|len| {
            (0..5_usize.pow(len)).map(move |mut code| {
                (0..len)
                    .map(|_| {
                        let n = code % 5;
                        code /= 5;
                        n as u32
                    })
                    .collect::<Vec<u32>>()
            })
        });
        let edge = [long, long, 3, 3, 0, QUAD_BATCH, QUAD_BATCH - 1, long];
        for counts in small.chain([edge.to_vec()]) {
            // A draw splats its instances in turn, each over the draw's quads; the shader
            // culls quads past a column's count.
            let draws: Vec<_> = quad_draws(&counts, u32::MAX).collect();
            let actual: Vec<_> = draws
                .iter()
                .flat_map(|(columns, quads)| {
                    columns
                        .clone()
                        .flat_map(move |c| quads.clone().map(move |q| (c, q)))
                })
                .filter(|&(c, q)| q < counts[c as usize])
                .collect();
            let expected: Vec<_> = (0..counts.len() as u32)
                .flat_map(|c| (0..counts[c as usize]).map(move |q| (c, q)))
                .collect();
            assert!(actual == expected, "{:?}", &counts[..counts.len().min(8)]);
            let drawn: u64 = draws
                .iter()
                .map(|(columns, quads)| u64::from(columns.len() as u32 * quads.len() as u32))
                .sum();
            let points: u64 = counts.iter().map(|&count| u64::from(count)).sum();
            assert!((drawn - points) * RUN_PADDING <= points, "{counts:?}");
        }
        assert_eq!(
            quad_draws(&[4, 4, 0, 2, 4, 4, 5, 4, 5], u32::MAX).collect::<Vec<_>>(),
            [(0..2, 0..4), (3..4, 0..2), (4..6, 0..4), (6..9, 0..5)]
        );
    }
}
