// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use bytemuck::{Pod, Zeroable};
use iced::advanced::text::Text as IcedText;
use iced::{Border, Color, Point, Rectangle, Renderer, Size};
use std::collections::HashMap;
use std::mem::size_of;

pub(in crate::visuals) const SDF_VERTICES_PER_INSTANCE: u32 = 4;

pub(crate) fn scroll_delta_lines(delta: iced::mouse::ScrollDelta) -> f32 {
    match delta {
        iced::mouse::ScrollDelta::Lines { y, .. } => y,
        iced::mouse::ScrollDelta::Pixels { y, .. } => y / 50.0,
    }
}

pub(crate) fn border_color(theme: &iced::Theme, emphasized: bool) -> Color {
    use crate::util::color::{lerp_color, with_alpha};
    let base = theme.extended_palette().background.base;
    let mix = if emphasized { 0.58 } else { 0.32 };
    with_alpha(lerp_color(base.color, base.text, mix), 1.0)
}

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

// Clockwise quarter turns of a pane. Content is laid out unturned in
// `frame`, which shares the pane's origin and swaps its size on odd
// turns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QuarterTurn(u8);

impl QuarterTurn {
    pub fn new(rotation: i8) -> Self {
        Self(rotation.rem_euclid(4) as u8)
    }

    pub fn turns(self) -> u8 {
        self.0
    }

    pub fn swaps_axes(self) -> bool {
        self.0 % 2 == 1
    }

    // Whether the frame's y axis runs against its screen axis.
    pub fn flips_y(self) -> bool {
        matches!(self.0, 1 | 2)
    }

    pub fn frame_size(self, size: Size) -> Size {
        if self.swaps_axes() {
            Size::new(size.height, size.width)
        } else {
            size
        }
    }

    pub fn frame(self, bounds: Rectangle) -> Rectangle {
        Rectangle::new(bounds.position(), self.frame_size(bounds.size()))
    }

    fn point_to_screen(self, bounds: Rectangle, p: Point) -> Point {
        let (u, v) = (p.x - bounds.x, p.y - bounds.y);
        let (x, y) = match self.0 {
            0 => return p,
            1 => (bounds.width - v, u),
            2 => (bounds.width - u, bounds.height - v),
            _ => (v, bounds.height - u),
        };
        Point::new(bounds.x + x, bounds.y + y)
    }

    pub fn to_screen(self, bounds: Rectangle, r: Rectangle) -> Rectangle {
        let a = self.point_to_screen(bounds, r.position());
        let b = self.point_to_screen(bounds, r.position() + r.size().into());
        Rectangle::new(
            Point::new(a.x.min(b.x), a.y.min(b.y)),
            self.frame_size(r.size()),
        )
    }

    pub fn to_frame(self, bounds: Rectangle, p: Point) -> Point {
        Self((4 - self.0) % 4).point_to_screen(self.frame(bounds), p)
    }
}

pub fn lane_bounds(bounds: Rectangle, count: usize, index: usize) -> Option<Rectangle> {
    let height = bounds.height / count as f32;
    bounds.intersection(&Rectangle {
        y: bounds.y + index as f32 * height,
        height,
        ..bounds
    })
}

pub fn amplitude_y(lane: Rectangle, amplitude: f32) -> f32 {
    lane.center_y() - amplitude * (lane.height * 0.5)
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

pub(in crate::visuals) fn fill_rect(r: &mut Renderer, bounds: Rectangle, color: Color) {
    fill_bordered_rect(r, bounds, color, Border::default(), true);
}

pub(in crate::visuals) fn fill_bordered_rect(
    r: &mut Renderer,
    bounds: Rectangle,
    color: Color,
    border: Border,
    snap: bool,
) {
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
    // Match shaders/sdf.wgsl primitive tags.
    const BASELINE: f32 = 1.0;
    const LINE: f32 = 2.0;
    const DOT: f32 = 3.0;
    const RADIAL_DOT: f32 = 4.0;
    // bits > primitive count = clockwise quarter turns.
    const TURN: f32 = 8.0;
}

const _: () = assert!(SdfInstance::RADIAL_DOT < SdfInstance::TURN);

// Turns instances built in `turn.frame(bounds)` with a clip transform
// of that frame so they fill `bounds`. The shader turns each
// primitive after expanding it, so fills and gradients keep their
// frame axes.
pub(in crate::visuals) fn turn_instances(instances: &mut [SdfInstance], turn: QuarterTurn) {
    let tag = SdfInstance::TURN * f32::from(turn.0);
    for instance in instances {
        instance.params[3] = instance.params[3] % SdfInstance::TURN + tag;
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

pub(in crate::visuals) fn replace_quad_instance(
    bounds: Rectangle,
    clip: ClipTransform,
    color: [f32; 4],
) -> SdfInstance {
    let mut instance = quad_instance(
        bounds.x,
        bounds.y,
        bounds.x + bounds.width,
        bounds.y + bounds.height,
        clip,
        color,
    );
    instance.params[0] = 1.0;
    instance
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
    fade: bool,
) -> SdfInstance {
    SdfInstance {
        p0: clip.to_clip(p0.0, p0.1),
        p1: clip.to_clip(p1.0, p1.1),
        color0,
        color1,
        params: [
            1.0 - (baseline - clip.origin[1]) * clip.scale[1],
            if fade { 1.0 } else { 0.0 },
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
) -> SdfInstance {
    let outer = radius + 1.0;
    SdfInstance {
        p0: clip.to_clip(cx, cy),
        p1: [outer * clip.scale[0], -outer * clip.scale[1]],
        color0: color,
        color1: color,
        params: [radius, 0.0, 0.0, SdfInstance::DOT],
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
    fill: [[f32; 4]; 2],
    clip: ClipTransform,
) {
    out.reserve(pts.len().saturating_sub(1) * 2);
    let mut push_fill = |p0, p1| {
        out.push(baseline_segment_instance(
            p0, p1, baseline, clip, fill, true,
        ))
    };
    for seg in pts.windows(2) {
        let (d0, d1) = (seg[0].1 - baseline, seg[1].1 - baseline);
        if d0 * d1 < 0.0 {
            let crossing = (seg[0].0 + (seg[1].0 - seg[0].0) * d0 / (d0 - d1), baseline);
            push_fill(seg[0], crossing);
            push_fill(crossing, seg[1]);
        } else {
            push_fill(seg[0], seg[1]);
        }
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

pub fn decimate_line(pts: &mut Vec<(f32, f32)>, max_points: usize) {
    debug_assert!(
        pts.iter()
            .all(|point| point.0.is_finite() && point.1.is_finite())
    );
    debug_assert!(pts.windows(2).all(|window| window[0].0 <= window[1].0));
    let len = pts.len();
    if max_points < 2 || len <= 2 {
        pts.truncate(max_points);
        return;
    }
    let kept = decimate_finite_ordered_line_in_place(&mut pts[1..len - 1], max_points - 2);
    pts[kept + 1] = pts[len - 1];
    pts.truncate(kept + 2);
}

fn decimate_finite_ordered_line_in_place(pts: &mut [(f32, f32)], max_points: usize) -> usize {
    if max_points < 2 || pts.len() <= 1 {
        return pts.len().min(max_points);
    }

    let last = pts[pts.len() - 1];
    let (x0, width) = (pts[0].0, last.0 - pts[0].0);
    let (buckets, bucket_scale, bucket_width) =
        crate::util::finite_positive(width).map_or((1, 0.0, 0.0), |width| {
            let buckets = (max_points / 2).min(width.ceil().max(1.0) as usize);
            (buckets, buckets as f32 / width, width / buckets as f32)
        });
    let (mut read, mut out, mut groups) = (0, 0, 0);

    while read < pts.len() {
        let start = read;
        let bucket = ((pts[start].0 - x0) * bucket_scale).clamp(0.0, (buckets - 1) as f32) as usize;
        groups += 1;
        let end_x = if groups < buckets {
            x0 + bucket_width * (bucket + 1) as f32
        } else {
            f32::INFINITY
        };
        let (mut mn, mut mx) = (start, start);
        let (mut min_y, mut max_y) = (pts[start].1, pts[start].1);
        read = start + 1;
        while read < pts.len() && pts[read].0 <= end_x {
            let y = pts[read].1;
            if y < min_y {
                (mn, min_y) = (read, y);
            }
            if y > max_y {
                (mx, max_y) = (read, y);
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
    out
}

pub(in crate::visuals) fn create_buffer(
    device: &wgpu::Device,
    label: &'static str,
    size: wgpu::BufferAddress,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation: false,
    })
}

#[derive(Default)]
pub struct CacheTracker {
    frame: u64,
}

impl CacheTracker {
    // in advance calls
    const RETAIN: u64 = 256;
    const INTERVAL: u64 = 64;

    pub fn advance(&mut self) -> (u64, Option<u64>) {
        self.frame = self.frame.wrapping_add(1).max(1);
        let threshold = self
            .frame
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

pub(in crate::visuals) fn begin_pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    target: &'a wgpu::TextureView,
    clip: Option<&Rectangle<u32>>,
    label: &'static str,
    load: wgpu::LoadOp<wgpu::Color>,
) -> wgpu::RenderPass<'a> {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
    if let Some(clip) = clip {
        pass.set_scissor_rect(clip.x, clip.y, clip.width.max(1), clip.height.max(1));
    }
    pass
}

pub(in crate::visuals) struct RenderPipelineSpec<'a> {
    pub(in crate::visuals) label: &'static str,
    pub(in crate::visuals) shader: &'a wgpu::ShaderModule,
    pub(in crate::visuals) vertex_entry: &'static str,
    pub(in crate::visuals) topology: wgpu::PrimitiveTopology,
    pub(in crate::visuals) fragment_entry: &'static str,
    pub(in crate::visuals) buffers: &'a [wgpu::VertexBufferLayout<'a>],
    pub(in crate::visuals) bind_group_layouts: &'a [&'a wgpu::BindGroupLayout],
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

pub type GeometryFingerprint = [u64; 6];

pub fn pack_f32_pair(first: f32, second: f32) -> u64 {
    u64::from(first.to_bits()) << 32 | u64::from(second.to_bits())
}

pub fn bounds_fingerprint(revision: u64, bounds: Rectangle) -> GeometryFingerprint {
    [
        revision,
        pack_f32_pair(bounds.x, bounds.y),
        pack_f32_pair(bounds.width, bounds.height),
        0,
        0,
        0,
    ]
}

struct CachedInstance {
    buffer: wgpu::Buffer,
    vertex_count: u32,
    fingerprint: Option<GeometryFingerprint>,
    last_used: u64,
}

pub struct SdfPipeline<const REPLACE: bool = false> {
    pub(in crate::visuals) pipeline: wgpu::RenderPipeline,
    instances: HashMap<u64, CachedInstance>,
    cache: CacheTracker,
    pub(in crate::visuals) scratch: GeometryScratch,
}

impl<const REPLACE: bool> SdfPipeline<REPLACE> {
    pub fn prepare_required(&mut self, key: u64, fingerprint: Option<GeometryFingerprint>) -> bool {
        let (frame, threshold) = self.cache.advance();
        let current = match fingerprint.and_then(|_| self.instances.get_mut(&key)) {
            Some(entry) if entry.fingerprint == fingerprint => {
                entry.last_used = frame;
                true
            }
            _ => false,
        };
        if let Some(threshold) = threshold {
            self.instances
                .retain(|_, entry| entry.last_used >= threshold);
        }
        !current
    }

    pub fn prepare_scratch(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &'static str,
        key: u64,
        fingerprint: Option<GeometryFingerprint>,
    ) {
        let instances = &self.scratch.instances;
        let required = size_of::<SdfInstance>() as wgpu::BufferAddress
            * instances.len() as wgpu::BufferAddress;
        let target = required.max(1).next_power_of_two();
        let usage = wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST;
        let allocate = || create_buffer(device, label, target, usage);
        let frame = self.cache.frame;
        let entry = self.instances.entry(key).or_insert_with(|| CachedInstance {
            buffer: allocate(),
            vertex_count: 0,
            fingerprint,
            last_used: frame,
        });
        entry.fingerprint = fingerprint;
        entry.last_used = frame;
        let capacity = entry.buffer.size();
        if target > capacity || (required > 0 && capacity > target.saturating_mul(4)) {
            entry.buffer = allocate();
        }
        entry.vertex_count = instances.len() as u32;
        if !instances.is_empty() {
            queue.write_buffer(&entry.buffer, 0, bytemuck::cast_slice(instances));
        }
    }

    pub(in crate::visuals) fn instance(&self, key: u64) -> Option<(&wgpu::Buffer, u32)> {
        self.instances
            .get(&key)
            .map(|entry| (&entry.buffer, entry.vertex_count))
    }
}

impl<const REPLACE: bool> iced_wgpu::primitive::Pipeline for SdfPipeline<REPLACE> {
    fn new(device: &wgpu::Device, _: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        const LABEL: &str = "Visual SDF pipeline";
        const ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
            0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4
        ];
        let shader = create_shader_module(device, LABEL, include_str!("shaders/sdf.wgsl"));
        let pipeline = create_render_pipeline(
            device,
            format,
            RenderPipelineSpec {
                label: LABEL,
                shader: &shader,
                vertex_entry: "vs_main",
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                fragment_entry: "fs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: size_of::<SdfInstance>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &ATTRS,
                }],
                bind_group_layouts: &[],
                blend: Some(if REPLACE {
                    wgpu::BlendState::REPLACE
                } else {
                    wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING
                }),
                write_mask: wgpu::ColorWrites::ALL,
            },
        );
        Self {
            pipeline,
            instances: HashMap::new(),
            cache: CacheTracker::default(),
            scratch: GeometryScratch::default(),
        }
    }
}

macro_rules! sdf_primitive {
    ($primitive:ty, $pipeline:ty, $label:expr, |$self:ident| $key:expr, $fingerprint:expr $(,)?) => {
        $crate::visuals::render::common::sdf_primitive!(
            $primitive, $pipeline, $label,
            layers |$self, scratch| {
                ($key, $fingerprint) => {
                    $self.build_vertices(scratch);
                }
            }
        );
    };
    ($primitive:ty, $pipeline:ty, $label:expr,
        layers |$self:ident, $scratch:ident| {
            $(($key:expr, $fingerprint:expr) => $build:block),+ $(,)?
        } $(,)?
    ) => {
        impl iced_wgpu::primitive::Primitive for $primitive {
            type Pipeline = $pipeline;

            fn prepare(
                &$self,
                pipeline: &mut Self::Pipeline,
                device: &wgpu::Device,
                queue: &wgpu::Queue,
                _bounds: &iced::Rectangle,
                _viewport: &iced::advanced::graphics::Viewport,
            ) {
                $({
                    let key: u64 = $key;
                    let fingerprint: Option<$crate::visuals::render::common::GeometryFingerprint> = $fingerprint;
                    if pipeline.prepare_required(key, fingerprint) {
                        pipeline.scratch.clear();
                        let $scratch = &mut pipeline.scratch;
                        $build
                        pipeline.prepare_scratch(device, queue, $label, key, fingerprint);
                    }
                })+
            }

            fn draw(
                &$self,
                pipeline: &Self::Pipeline,
                pass: &mut wgpu::RenderPass<'_>,
            ) -> bool {
                let mut pipeline_bound = false;
                for key in [$($key),+] {
                    if let Some((buffer, count)) = pipeline
                        .instance(key)
                        .filter(|(_, count)| *count > 0)
                    {
                        if !pipeline_bound {
                            pass.set_pipeline(&pipeline.pipeline);
                            pipeline_bound = true;
                        }
                        pass.set_vertex_buffer(0, buffer.slice(..));
                        pass.draw(
                            0..$crate::visuals::render::common::SDF_VERTICES_PER_INSTANCE,
                            0..count,
                        );
                    }
                }
                true
            }
        }
    };
}

pub(in crate::visuals) use sdf_primitive;

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::naga::front::wgsl::parse_str;

    #[test]
    fn quarter_turns_map_frame_onto_pane() {
        let bounds = Rectangle::new(Point::new(10.0, 20.0), Size::new(100.0, 50.0));
        let rect = Rectangle::new(bounds.position(), Size::new(4.0, 2.0));
        for (rotation, [x, y, width, height]) in [
            (0, [10.0, 20.0, 4.0, 2.0]),
            (1, [108.0, 20.0, 2.0, 4.0]),
            (2, [106.0, 68.0, 4.0, 2.0]),
            (-1, [10.0, 66.0, 2.0, 4.0]),
        ] {
            let turn = QuarterTurn::new(rotation);
            let expected = Rectangle::new(Point::new(x, y), Size::new(width, height));
            assert_eq!(
                turn.to_screen(bounds, rect),
                expected,
                "rotation={rotation}"
            );
            let point = Point::new(13.0, 21.0);
            let screen = turn.point_to_screen(bounds, point);
            assert_eq!(turn.to_frame(bounds, screen), point, "rotation={rotation}");
        }
        assert_eq!(QuarterTurn::new(3), QuarterTurn::new(-1));
        assert_eq!(QuarterTurn::new(i8::MIN), QuarterTurn::new(0));
    }

    #[test]
    fn shaders_are_valid() {
        for source in [
            include_str!("shaders/sdf.wgsl"),
            include_str!("shaders/spectrogram.wgsl"),
        ] {
            let module = parse_str(source).expect("shader should parse");
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .expect("shader should validate");
        }
    }

    #[test]
    fn decimate_line_advances_when_bucket_edge_rounds_below_point() {
        let mut points = [(667.6, 0.0), (3881.2603, 1.0)];
        let kept = decimate_finite_ordered_line_in_place(&mut points, 5507);
        assert_eq!(points[..kept], [(667.6, 0.0), (3881.2603, 1.0)]);
    }

    #[test]
    fn ordered_decimation_preserves_extrema_within_budget() {
        let mut points: Vec<_> = (0..16_385)
            .map(|i| (i as f32 * 0.125, (i as f32 * 0.017).sin()))
            .collect();
        let min = points.iter().map(|point| point.1).reduce(f32::min).unwrap();
        let max = points.iter().map(|point| point.1).reduce(f32::max).unwrap();

        let kept = decimate_finite_ordered_line_in_place(&mut points, 2_000);
        let points = &points[..kept];

        assert!(points.len() <= 2_000);
        assert!(points.windows(2).all(|window| window[0].0 <= window[1].0));
        assert!(points.iter().any(|point| point.1 == min));
        assert!(points.iter().any(|point| point.1 == max));

        for xs in [[1.0; 4], [-f32::MAX, -1.0, 1.0, f32::MAX]] {
            let mut points: Vec<_> = xs.into_iter().zip([0.0, -2.0, 3.0, 1.0]).collect();
            let kept = decimate_finite_ordered_line_in_place(&mut points, 2);
            assert_eq!(points[..kept], [(xs[1], -2.0), (xs[2], 3.0)]);
        }
    }

    #[test]
    fn ordered_decimation_honors_budget_at_rounded_bucket_edges() {
        let mut points: Vec<_> = (0..8)
            .map(|i| (0.1 + 1_000.3 * i as f32 / 7.0, (-1.0_f32).powi(i)))
            .collect();

        assert!(decimate_finite_ordered_line_in_place(&mut points, 6) <= 6);
    }

    #[test]
    fn filled_line_splits_fill_at_baseline_crossings() {
        let clip = ClipTransform::from_bounds(Rectangle::new(Point::ORIGIN, Size::new(8.0, 40.0)));
        let pts = [(0.0, 10.0), (4.0, 30.0), (6.0, 20.0), (8.0, 10.0)];
        let mut out = Vec::new();
        extend_filled_line(&mut out, &pts, 20.0, 1.0, [1.0; 4], [[0.0; 4]; 2], clip);
        let ends = [pts[0], (2.0, 20.0), pts[1], pts[2], pts[3]].map(|(x, y)| clip.to_clip(x, y));
        let fills: Vec<_> = out[..4].iter().map(|i| [i.p0, i.p1]).collect();
        assert_eq!(fills, ends.windows(2).collect::<Vec<_>>());
        assert!(out[..4].iter().all(|i| i.params[1] == 1.0));
        assert_eq!(out.len(), 4 + pts.len() - 1);
    }
}
