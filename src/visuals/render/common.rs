// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use bytemuck::{Pod, Zeroable};
use iced::advanced::text::Text as IcedText;
use iced::{Border, Color, Rectangle, Renderer, Size};
use std::collections::HashMap;
use std::mem::size_of;

#[derive(Clone, Copy)]
pub struct ClipTransform {
    origin: [f32; 2],
    scale: [f32; 2],
}

impl ClipTransform {
    pub fn from_bounds(bounds: Rectangle) -> Self {
        Self {
            origin: [bounds.x, bounds.y],
            scale: [2.0 / bounds.width.max(1.0), 2.0 / bounds.height.max(1.0)],
        }
    }

    pub fn to_clip(self, x: f32, y: f32) -> [f32; 2] {
        [
            (x - self.origin[0]) * self.scale[0] - 1.0,
            1.0 - (y - self.origin[1]) * self.scale[1],
        ]
    }
}

#[derive(Clone, Copy)]
pub struct ChannelLayout {
    top: f32,
    stride: f32,
    pub channel_height: f32,
    pub amplitude_scale: f32,
}

impl ChannelLayout {
    pub fn new(bounds: Rectangle, channels: usize, padding: f32, gap: f32, amp: f32) -> Self {
        let channels = channels.max(1) as f32;
        let (padding, gap) = (padding.max(0.0), gap.max(0.0));
        let channel_height =
            (bounds.height - padding * 2.0 - gap * (channels - 1.0)).max(1.0) / channels;
        Self {
            top: bounds.y + padding,
            stride: channel_height + gap,
            channel_height,
            amplitude_scale: channel_height * 0.5 * amp.max(0.01),
        }
    }

    pub fn center_y(self, channel: usize) -> f32 {
        self.top + channel as f32 * self.stride + self.channel_height * 0.5
    }
}

pub(in crate::visuals) fn text<C>(content: C, px: f32, bounds: Size) -> IcedText<C> {
    use iced::advanced::text;
    text::Text {
        content,
        bounds,
        size: iced::Pixels(px),
        font: iced::Font::default(),
        align_x: iced::alignment::Horizontal::Left.into(),
        align_y: iced::alignment::Vertical::Top,
        line_height: text::LineHeight::default(),
        shaping: text::Shaping::Basic,
        wrapping: text::Wrapping::None,
    }
}

fn fill_rect_quad(r: &mut Renderer, bounds: Rectangle, color: Color, border: Border, snap: bool) {
    use iced::advanced::{Renderer as _, renderer::Quad};
    r.fill_quad(
        Quad {
            bounds,
            border,
            snap,
            ..Default::default()
        },
        color,
    );
}

pub(in crate::visuals) fn fill_rect(r: &mut Renderer, bounds: Rectangle, color: Color) {
    fill_rect_quad(r, bounds, color, Border::default(), true);
}

pub(in crate::visuals) fn fill_bordered_rect(
    r: &mut Renderer,
    bounds: Rectangle,
    color: Color,
    border: Border,
    snap: bool,
) {
    fill_rect_quad(r, bounds, color, border, snap);
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct SdfInstance {
    p0: [f32; 2],
    p1: [f32; 2],
    color0: [f32; 4],
    color1: [f32; 4],
    params: [f32; 4],
}

impl SdfInstance {
    const BASELINE: f32 = 1.0;
    const LINE: f32 = 2.0;
    const DOT: f32 = 3.0;
    const RADIAL_DOT: f32 = 4.0;

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
            0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4
        ];
        wgpu::VertexBufferLayout {
            array_stride: size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &ATTRS,
        }
    }
}

pub fn quad_instance(
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    clip: ClipTransform,
    color: [f32; 4],
) -> SdfInstance {
    gradient_quad_instance(x0, y0, x1, y1, clip, color, color)
}

pub(in crate::visuals) fn gradient_quad_instance(
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    clip: ClipTransform,
    top: [f32; 4],
    bottom: [f32; 4],
) -> SdfInstance {
    SdfInstance {
        p0: clip.to_clip(x0, y0),
        p1: clip.to_clip(x1, y1),
        color0: top,
        color1: bottom,
        params: [0.0; 4],
    }
}

pub(in crate::visuals) fn baseline_segment_instance(
    p0: (f32, f32),
    p1: (f32, f32),
    baseline: f32,
    clip: ClipTransform,
    [color0, color1]: [[f32; 4]; 2],
) -> SdfInstance {
    SdfInstance {
        p0: clip.to_clip(p0.0, p0.1),
        p1: clip.to_clip(p1.0, p1.1),
        color0,
        color1,
        params: [
            1.0 - (baseline - clip.origin[1]) * clip.scale[1],
            0.0,
            0.0,
            SdfInstance::BASELINE,
        ],
    }
}

pub fn line_instance(
    p0: (f32, f32),
    p1: (f32, f32),
    color0: [f32; 4],
    color1: [f32; 4],
    width: f32,
    clip: ClipTransform,
) -> SdfInstance {
    let (dx, dy) = (p1.0 - p0.0, p1.1 - p0.1);
    let scale = (dx * dx + dy * dy).max(1e-12).sqrt().recip() * (width * 0.5 + 1.0);
    SdfInstance {
        p0: clip.to_clip(p0.0, p0.1),
        p1: clip.to_clip(p1.0, p1.1),
        color0,
        color1,
        params: [
            -dy * scale * clip.scale[0],
            -dx * scale * clip.scale[1],
            width * 0.5,
            SdfInstance::LINE,
        ],
    }
}

#[derive(Clone, Copy)]
pub struct RadialDotTemplate(SdfInstance);

impl RadialDotTemplate {
    pub fn new(
        center_radius: [f32; 3],
        scale: f32,
        dot_radius: f32,
        clip: ClipTransform,
        additive: bool,
    ) -> Self {
        let center = clip.to_clip(center_radius[0], center_radius[1]);
        Self(SdfInstance {
            p0: [0.0; 2],
            p1: [clip.scale[0], -clip.scale[1]],
            color0: [0.0; 4],
            color1: [
                center[0],
                center[1],
                center_radius[2] * clip.scale[0],
                -center_radius[2] * clip.scale[1],
            ],
            params: [
                dot_radius,
                if additive { 1.0 } else { 0.0 },
                scale,
                SdfInstance::RADIAL_DOT,
            ],
        })
    }

    pub fn instance(self, point: (f32, f32), color: [f32; 4]) -> SdfInstance {
        SdfInstance {
            p0: [point.0, point.1],
            color0: color,
            ..self.0
        }
    }
}

pub fn dot_instance(
    cx: f32,
    cy: f32,
    radius: f32,
    color: [f32; 4],
    clip: ClipTransform,
    additive: bool,
) -> SdfInstance {
    let outer = radius + 1.0;
    SdfInstance {
        p0: clip.to_clip(cx, cy),
        p1: [outer * clip.scale[0], -outer * clip.scale[1]],
        color0: color,
        color1: color,
        params: [
            radius,
            if additive { 1.0 } else { 0.0 },
            0.0,
            SdfInstance::DOT,
        ],
    }
}

pub fn extend_aa_line_list(
    out: &mut Vec<SdfInstance>,
    pts: &[(f32, f32)],
    stroke: f32,
    color: [f32; 4],
    clip: ClipTransform,
) {
    let width = stroke.max(0.1);
    out.reserve(pts.len().saturating_sub(1));
    for seg in pts.windows(2) {
        let (dx, dy) = (seg[1].0 - seg[0].0, seg[1].1 - seg[0].1);
        if (dx * dx + dy * dy) >= 1e-8 {
            out.push(line_instance(seg[0], seg[1], color, color, width, clip));
        }
    }
}

pub fn extend_filled_line(
    out: &mut Vec<SdfInstance>,
    pts: &[(f32, f32)],
    baseline: f32,
    stroke: f32,
    line: [f32; 4],
    fill: [f32; 4],
    clip: ClipTransform,
) {
    out.reserve(pts.len().saturating_sub(1) * 2);
    for seg in pts.windows(2) {
        out.push(baseline_segment_instance(
            seg[0], seg[1], baseline, clip, [fill; 2],
        ));
    }
    extend_aa_line_list(out, pts, stroke, line, clip);
}

#[derive(Default)]
pub struct GeometryScratch {
    pub instances: Vec<SdfInstance>,
    pub points: Vec<(f32, f32)>,
    pub points2: Vec<(f32, f32)>,
    pub scalars: Vec<f32>,
}

impl GeometryScratch {
    pub fn clear(&mut self) {
        self.instances.clear();
        self.points.clear();
        self.points2.clear();
        self.scalars.clear();
    }
}

/// Decimates points known to be finite and ordered by x.
pub fn decimate_finite_ordered_line_in_place(pts: &mut Vec<(f32, f32)>, max_points: usize) {
    debug_assert!(
        pts.iter()
            .all(|point| point.0.is_finite() && point.1.is_finite())
    );
    debug_assert!(pts.windows(2).all(|window| window[0].0 <= window[1].0));
    if max_points < 2 {
        pts.truncate(max_points);
        return;
    }
    if pts.len() <= 1 {
        return;
    }

    let Some(&last) = pts.last() else { return };
    let (x0, width) = (pts[0].0, last.0 - pts[0].0);
    let bucketed = crate::util::finite_positive(width).is_some();
    let buckets = if bucketed {
        (max_points / 2).min(width.ceil().max(1.0) as usize)
    } else {
        1
    };
    let bucket_scale = if bucketed {
        buckets as f32 / width
    } else {
        0.0
    };
    let bucket_width = if bucketed {
        width / buckets as f32
    } else {
        0.0
    };
    let (mut read, mut out, mut groups) = (0, 0, 0);

    while read < pts.len() {
        let start = read;
        let bucket = if bucketed {
            ((pts[start].0 - x0) * bucket_scale).clamp(0.0, (buckets - 1) as f32) as usize
        } else {
            0
        };
        groups += 1;
        // A rounded edge can fall below a point assigned to its bucket. Let the
        // last budgeted group consume the remainder rather than exceed the cap.
        let end_x = if bucketed && groups < buckets {
            x0 + bucket_width * (bucket + 1) as f32
        } else {
            f32::INFINITY
        };
        let (mut mn, mut mx) = (start, start);
        read = start + 1;
        while read < pts.len() && pts[read].0 <= end_x {
            if pts[read].1 < pts[mn].1 {
                mn = read;
            }
            if pts[read].1 > pts[mx].1 {
                mx = read;
            }
            read += 1;
        }
        let push = |pts: &mut [(f32, f32)], out: &mut usize, point| {
            if *out == 0 || pts[*out - 1] != point {
                pts[*out] = point;
                *out += 1;
            }
        };
        if pts[read - 1].0 - pts[start].0 <= 1.0 {
            let (x, lo, hi) = ((pts[start].0 + pts[read - 1].0) * 0.5, pts[mn].1, pts[mx].1);
            push(pts, &mut out, (x, lo));
            push(pts, &mut out, (x, hi));
        } else {
            for point in [pts[mn.min(mx)], pts[mn.max(mx)]] {
                push(pts, &mut out, point);
            }
        }
    }
    pts.truncate(out);
}

pub struct InstanceBuffer {
    pub vertex_buffer: wgpu::Buffer,
    pub capacity: wgpu::BufferAddress,
    pub vertex_count: u32,
}

impl InstanceBuffer {
    fn capacity_for(size: wgpu::BufferAddress) -> wgpu::BufferAddress {
        size.max(1).next_power_of_two()
    }

    pub fn new(device: &wgpu::Device, label: &'static str, size: wgpu::BufferAddress) -> Self {
        let size = Self::capacity_for(size);
        Self {
            vertex_buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            capacity: size,
            vertex_count: 0,
        }
    }

    pub fn ensure_capacity(
        &mut self,
        device: &wgpu::Device,
        label: &'static str,
        size: wgpu::BufferAddress,
    ) {
        let target = Self::capacity_for(size);
        if target > self.capacity || (size > 0 && self.capacity > target.saturating_mul(4)) {
            *self = Self::new(device, label, target);
        }
    }

    pub fn write(&mut self, queue: &wgpu::Queue, instances: &[SdfInstance]) {
        self.vertex_count = instances.len() as u32;
        if !instances.is_empty() {
            queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(instances));
        }
    }

    pub fn used_bytes(&self) -> wgpu::BufferAddress {
        self.vertex_count as wgpu::BufferAddress * size_of::<SdfInstance>() as wgpu::BufferAddress
    }
}

#[derive(Debug, Clone, Default)]
pub struct CacheTracker {
    frame: u64,
    counter: u64,
}

impl CacheTracker {
    const RETAIN: u64 = 256;
    const INTERVAL: u64 = 64;

    pub fn advance(&mut self) -> (u64, Option<u64>) {
        self.frame = self.frame.wrapping_add(1).max(1);
        self.counter = self.counter.wrapping_add(1);
        let threshold = self
            .counter
            .is_multiple_of(Self::INTERVAL)
            .then_some(self.frame.saturating_sub(Self::RETAIN));
        (self.frame, threshold)
    }
}

pub fn create_shader_module(
    device: &wgpu::Device,
    label: &'static str,
    source: &'static str,
) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

pub(in crate::visuals) fn begin_load_pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    target: &'a wgpu::TextureView,
    clip: &Rectangle<u32>,
    label: &'static str,
) -> wgpu::RenderPass<'a> {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
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
    pass
}

pub(in crate::visuals) struct RenderPipelineSpec<'a> {
    pub(in crate::visuals) label: &'static str,
    pub(in crate::visuals) shader: &'a wgpu::ShaderModule,
    pub(in crate::visuals) vertex_entry: &'static str,
    pub(in crate::visuals) fragment_entry: &'static str,
    pub(in crate::visuals) buffers: &'a [wgpu::VertexBufferLayout<'a>],
    pub(in crate::visuals) bind_group_layouts: &'a [&'a wgpu::BindGroupLayout],
    pub(in crate::visuals) topology: wgpu::PrimitiveTopology,
    pub(in crate::visuals) blend: Option<wgpu::BlendState>,
    pub(in crate::visuals) write_mask: wgpu::ColorWrites,
}

pub(in crate::visuals) fn create_render_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    spec: RenderPipelineSpec<'_>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(spec.label),
        layout: Some(
            &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(spec.label),
                bind_group_layouts: spec.bind_group_layouts,
                push_constant_ranges: &[],
            }),
        ),
        vertex: wgpu::VertexState {
            module: spec.shader,
            entry_point: Some(spec.vertex_entry),
            buffers: spec.buffers,
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: spec.shader,
            entry_point: Some(spec.fragment_entry),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: spec.blend,
                write_mask: spec.write_mask,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: spec.topology,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

fn create_sdf_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    label: &'static str,
    topology: wgpu::PrimitiveTopology,
) -> wgpu::RenderPipeline {
    let shader = create_shader_module(device, label, include_str!("shaders/sdf.wgsl"));
    create_render_pipeline(
        device,
        format,
        RenderPipelineSpec {
            label,
            shader: &shader,
            vertex_entry: "vs_main",
            fragment_entry: "fs_main",
            buffers: &[SdfInstance::layout()],
            bind_group_layouts: &[],
            topology,
            blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
        },
    )
}

pub type GeometryFingerprint = [u64; 6];

pub fn bounds_fingerprint(revision: u64, bounds: Rectangle) -> GeometryFingerprint {
    let pack = |a: f32, b: f32| u64::from(a.to_bits()) << 32 | u64::from(b.to_bits());
    [
        revision,
        pack(bounds.x, bounds.y),
        pack(bounds.width, bounds.height),
        0,
        0,
        0,
    ]
}

struct CachedInstance {
    buffer: InstanceBuffer,
    fingerprint: Option<GeometryFingerprint>,
    last_used: u64,
}

pub struct SdfPipeline<K> {
    pub pipeline: wgpu::RenderPipeline,
    instances: HashMap<K, CachedInstance>,
    cache: CacheTracker,
}

impl<K: std::hash::Hash + Eq + Copy> SdfPipeline<K> {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        label: &'static str,
        topology: wgpu::PrimitiveTopology,
    ) -> Self {
        Self {
            pipeline: create_sdf_pipeline(device, format, label, topology),
            instances: HashMap::new(),
            cache: CacheTracker::default(),
        }
    }

    pub fn prepare_required(&mut self, key: K, fingerprint: Option<GeometryFingerprint>) -> bool {
        let (frame, threshold) = self.cache.advance();
        let current = fingerprint.is_some_and(|fingerprint| {
            self.instances.get_mut(&key).is_some_and(|entry| {
                let current = entry.fingerprint == Some(fingerprint);
                if current {
                    entry.last_used = frame;
                }
                current
            })
        });
        if let Some(threshold) = threshold {
            self.instances
                .retain(|_, entry| entry.last_used >= threshold);
        }
        !current
    }

    pub fn prepare_instance(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &'static str,
        key: K,
        fingerprint: Option<GeometryFingerprint>,
        instances: &[SdfInstance],
    ) {
        let required = size_of::<SdfInstance>() as wgpu::BufferAddress
            * instances.len() as wgpu::BufferAddress;
        let frame = self.cache.frame;
        let entry = self.instances.entry(key).or_insert_with(|| CachedInstance {
            buffer: InstanceBuffer::new(device, label, required),
            fingerprint,
            last_used: frame,
        });
        entry.fingerprint = fingerprint;
        entry.last_used = frame;
        entry.buffer.ensure_capacity(device, label, required);
        entry.buffer.write(queue, instances);
    }

    pub fn instance(&self, key: K) -> Option<&InstanceBuffer> {
        self.instances.get(&key).map(|e| &e.buffer)
    }
}

macro_rules! sdf_primitive {
    ($primitive:ident($params:ty), $($rest:tt)+) => {
        #[derive(Debug)]
        pub struct $primitive { params: $params }
        impl $primitive { pub fn new(params: $params) -> Self { Self { params } } }
        $crate::visuals::render::common::sdf_primitive!(@impl $primitive, $($rest)+);
    };
    (@impl $primitive:ident, $pipeline:ident, $key_ty:ty, $label:expr, $topology:ident, |$self:ident| $key_expr:expr $(, $fingerprint_expr:expr)?) => {
        impl iced_wgpu::primitive::Primitive for $primitive {
            type Pipeline = $pipeline;

            fn prepare(
                &$self,
                pipeline: &mut Self::Pipeline,
                device: &wgpu::Device,
                queue: &wgpu::Queue,
                _bounds: &iced::Rectangle,
                viewport: &iced::advanced::graphics::Viewport,
            ) {
                let key: $key_ty = $key_expr;
                let fingerprint = $crate::visuals::render::common::sdf_primitive!(
                    @fingerprint $self $(, $fingerprint_expr)?
                );
                if !pipeline.inner.prepare_required(key, fingerprint) {
                    return;
                }
                pipeline.scratch.clear();
                $self.build_vertices(viewport, &mut pipeline.scratch);
                pipeline.inner.prepare_instance(
                    device,
                    queue,
                    $label,
                    key,
                    fingerprint,
                    &pipeline.scratch.instances,
                );
            }

            fn draw(
                &$self,
                pipeline: &Self::Pipeline,
                pass: &mut wgpu::RenderPass<'_>,
            ) -> bool {
                let key: $key_ty = $key_expr;
                if let Some(inst) = pipeline.inner.instance(key).filter(|inst| inst.vertex_count > 0) {
                    pass.set_pipeline(&pipeline.inner.pipeline);
                    pass.set_vertex_buffer(0, inst.vertex_buffer.slice(0..inst.used_bytes()));
                    pass.draw(0..6, 0..inst.vertex_count);
                }
                true
            }
        }

        pub struct $pipeline {
            inner: $crate::visuals::render::common::SdfPipeline<$key_ty>,
            scratch: $crate::visuals::render::common::GeometryScratch,
        }

        impl iced_wgpu::primitive::Pipeline for $pipeline {
            fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
                Self {
                    inner: $crate::visuals::render::common::SdfPipeline::new(
                        device,
                        format,
                        $label,
                        wgpu::PrimitiveTopology::$topology,
                    ),
                    scratch: Default::default(),
                }
            }
        }
    };
    (@fingerprint $self:ident) => { None };
    (@fingerprint $self:ident, $fingerprint_expr:expr) => { Some($fingerprint_expr) };
}

pub(in crate::visuals) use sdf_primitive;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimate_line_advances_when_bucket_edge_rounds_below_point() {
        let mut points = vec![(667.6, 0.0), (3881.2603, 1.0)];
        decimate_finite_ordered_line_in_place(&mut points, 5507);
        assert_eq!(points.len(), 2);
    }

    #[test]
    fn ordered_decimation_preserves_extrema_within_budget() {
        let mut points: Vec<_> = (0..16_385)
            .map(|i| (i as f32 * 0.125, (i as f32 * 0.017).sin()))
            .collect();
        let min = points.iter().map(|point| point.1).reduce(f32::min).unwrap();
        let max = points.iter().map(|point| point.1).reduce(f32::max).unwrap();

        decimate_finite_ordered_line_in_place(&mut points, 2_000);

        assert!(points.len() <= 2_000);
        assert!(points.windows(2).all(|window| window[0].0 <= window[1].0));
        assert!(points.iter().any(|point| point.1 == min));
        assert!(points.iter().any(|point| point.1 == max));
    }

    #[test]
    fn ordered_decimation_honors_budget_at_rounded_bucket_edges() {
        let mut points = (0..8)
            .map(|i| (0.1 + 1_000.3 * i as f32 / 7.0, (-1.0_f32).powi(i)))
            .collect();

        decimate_finite_ordered_line_in_place(&mut points, 6);

        assert!(points.len() <= 6);
    }
}
