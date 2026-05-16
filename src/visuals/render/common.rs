// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use bytemuck::{Pod, Zeroable};
use iced::advanced::graphics::Viewport;
use iced::{Border, Color, Rectangle, Renderer, Size};
use iced_wgpu::wgpu;
use std::borrow::Cow;
use std::collections::HashMap;
use std::mem::size_of;

#[derive(Clone, Copy)]
pub struct ClipTransform(f32, f32);

impl ClipTransform {
    fn new(w: f32, h: f32) -> Self {
        Self(2.0 / w.max(1.0), 2.0 / h.max(1.0))
    }

    pub fn from_viewport(vp: &Viewport) -> Self {
        let s = vp.logical_size();
        Self::new(s.width, s.height)
    }

    #[inline]
    pub fn to_clip(self, x: f32, y: f32) -> [f32; 2] {
        [x * self.0 - 1.0, 1.0 - y * self.1]
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

    #[inline]
    pub fn center_y(self, channel: usize) -> f32 {
        self.top + channel as f32 * self.stride + self.channel_height * 0.5
    }
}

fn text<C>(content: C, px: f32, bounds: Size) -> iced::advanced::text::Text<C> {
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

pub(crate) fn measure_text(s: &str, px: f32) -> Size {
    use iced::advanced::graphics::text::Paragraph;
    use iced::advanced::text::Paragraph as _;
    Paragraph::with_text(text(s, px, Size::INFINITE)).min_bounds()
}

pub(crate) fn make_text(s: &str, px: f32, bounds: Size) -> iced::advanced::text::Text<String> {
    text(s.to_string(), px, bounds)
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

pub(crate) fn fill_rect(r: &mut Renderer, bounds: Rectangle, color: Color) {
    fill_rect_quad(r, bounds, color, Default::default(), true);
}

pub(crate) fn fill_bordered_rect(
    r: &mut Renderer,
    bounds: Rectangle,
    color: Color,
    border: Border,
) {
    fill_rect_quad(r, bounds, color, border, false);
}

pub(crate) fn fill_snapped_bordered_rect(
    r: &mut Renderer,
    bounds: Rectangle,
    color: Color,
    border: Border,
) {
    fill_rect_quad(r, bounds, color, border, true);
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct SdfVertex {
    pub position: [f32; 2],
    pub color: [f32; 4],
    pub params: [f32; 4],
}

impl SdfVertex {
    const SOLID_PARAMS: [f32; 4] = [0.0, 0.0, 1000.0, 0.0];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 3] =
            wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4, 2 => Float32x4];
        wgpu::VertexBufferLayout {
            array_stride: size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRS,
        }
    }

    #[inline]
    pub fn solid(pos: [f32; 2], color: [f32; 4]) -> Self {
        Self {
            position: pos,
            color,
            params: Self::SOLID_PARAMS,
        }
    }

    #[inline]
    fn antialiased(pos: [f32; 2], color: [f32; 4], dist: f32, radius: f32) -> Self {
        Self {
            position: pos,
            color,
            params: [dist, 0.0, radius, 0.0],
        }
    }
}

#[inline]
pub fn quad_vertices(
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    clip: ClipTransform,
    color: [f32; 4],
) -> [SdfVertex; 6] {
    gradient_quad_vertices(x0, y0, x1, y1, clip, color, color)
}

#[inline]
pub(crate) fn gradient_quad_vertices(
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    clip: ClipTransform,
    top: [f32; 4],
    bot: [f32; 4],
) -> [SdfVertex; 6] {
    let (tl, tr, bl, br) = (
        clip.to_clip(x0, y0),
        clip.to_clip(x1, y0),
        clip.to_clip(x0, y1),
        clip.to_clip(x1, y1),
    );
    [
        SdfVertex::solid(tl, top),
        SdfVertex::solid(bl, bot),
        SdfVertex::solid(br, bot),
        SdfVertex::solid(tl, top),
        SdfVertex::solid(br, bot),
        SdfVertex::solid(tr, top),
    ]
}

#[inline]
pub(crate) fn baseline_segment_vertices(
    p0: (f32, f32),
    p1: (f32, f32),
    baseline: f32,
    clip: ClipTransform,
    colors: [[f32; 4]; 2],
) -> [SdfVertex; 6] {
    let (t0, b0) = (p0.1.min(baseline), p0.1.max(baseline));
    let (t1, b1) = (p1.1.min(baseline), p1.1.max(baseline));
    let [c0, c1] = colors;
    [
        (p0.0, t0, c0),
        (p0.0, b0, c0),
        (p1.0, b1, c1),
        (p0.0, t0, c0),
        (p1.0, b1, c1),
        (p1.0, t1, c1),
    ]
    .map(|(x, y, c)| SdfVertex::solid(clip.to_clip(x, y), c))
}

#[inline]
pub fn line_vertices(
    p0: (f32, f32),
    p1: (f32, f32),
    c0: [f32; 4],
    c1: [f32; 4],
    width: f32,
    clip: ClipTransform,
) -> [SdfVertex; 6] {
    let (dx, dy) = (p1.0 - p0.0, p1.1 - p0.1);
    let inv = (dx * dx + dy * dy).sqrt().max(1e-6).recip();
    let (half, outer) = (width * 0.5, width * 0.5 + 1.0);
    let (ox, oy) = (-dy * inv * outer, dx * inv * outer);
    let v = |px, py, c, d| SdfVertex::antialiased(clip.to_clip(px, py), c, d, half);
    [
        v(p0.0 - ox, p0.1 - oy, c0, -outer),
        v(p0.0 + ox, p0.1 + oy, c0, outer),
        v(p1.0 + ox, p1.1 + oy, c1, outer),
        v(p0.0 - ox, p0.1 - oy, c0, -outer),
        v(p1.0 + ox, p1.1 + oy, c1, outer),
        v(p1.0 - ox, p1.1 - oy, c1, -outer),
    ]
}

#[inline]
pub fn dot_vertices(
    cx: f32,
    cy: f32,
    radius: f32,
    color: [f32; 4],
    clip: ClipTransform,
    additive: bool,
) -> [SdfVertex; 6] {
    let outer = radius + 1.0;
    let flag = if additive { 1.0 } else { 0.0 };
    [
        (-outer, -outer),
        (-outer, outer),
        (outer, -outer),
        (outer, -outer),
        (-outer, outer),
        (outer, outer),
    ]
    .map(|(ox, oy)| SdfVertex {
        position: clip.to_clip(cx + ox, cy + oy),
        color,
        params: [ox, oy, radius, flag],
    })
}

pub fn extend_aa_line_list(
    out: &mut Vec<SdfVertex>,
    pts: &[(f32, f32)],
    stroke: f32,
    color: [f32; 4],
    clip: ClipTransform,
) {
    let width = stroke.max(0.1);
    out.reserve(pts.len().saturating_sub(1) * 6);
    for seg in pts.windows(2) {
        let (dx, dy) = (seg[1].0 - seg[0].0, seg[1].1 - seg[0].1);
        if (dx * dx + dy * dy) >= 1e-8 {
            out.extend(line_vertices(seg[0], seg[1], color, color, width, clip));
        }
    }
}

pub fn extend_filled_line(
    out: &mut Vec<SdfVertex>,
    pts: &[(f32, f32)],
    baseline: f32,
    stroke: f32,
    line: [f32; 4],
    fill: [f32; 4],
    clip: ClipTransform,
) {
    out.reserve(pts.len().saturating_sub(1) * 12);
    for seg in pts.windows(2) {
        out.extend(baseline_segment_vertices(
            seg[0], seg[1], baseline, clip, [fill; 2],
        ));
    }
    extend_aa_line_list(out, pts, stroke, line, clip);
}

pub fn decimate_line(pts: &[(f32, f32)], max_points: usize) -> Cow<'_, [(f32, f32)]> {
    if pts.len() <= max_points {
        return Cow::Borrowed(pts);
    }
    let buckets = max_points / 2;
    let bucket_size = pts.len() as f32 / buckets.max(1) as f32;
    let mut result = Vec::with_capacity(max_points);
    for b in 0..buckets {
        let lo = (b as f32 * bucket_size) as usize;
        let hi = (((b + 1) as f32 * bucket_size) as usize).min(pts.len());
        if lo >= hi {
            continue;
        }
        let (mut mn_i, mut mx_i) = (0, 0);
        for (i, &(_, y)) in pts[lo..hi].iter().enumerate() {
            if y < pts[lo + mn_i].1 {
                mn_i = i;
            }
            if y > pts[lo + mx_i].1 {
                mx_i = i;
            }
        }
        result.push(pts[lo + mn_i.min(mx_i)]);
        result.push(pts[lo + mn_i.max(mx_i)]);
    }
    Cow::Owned(result)
}

#[derive(Debug)]
pub struct InstanceBuffer<V: Pod> {
    pub vertex_buffer: wgpu::Buffer,
    pub capacity: wgpu::BufferAddress,
    pub vertex_count: u32,
    _marker: std::marker::PhantomData<V>,
}

impl<V: Pod> InstanceBuffer<V> {
    pub fn new(device: &wgpu::Device, label: &'static str, size: wgpu::BufferAddress) -> Self {
        let size = size.max(1);
        Self {
            vertex_buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            capacity: size,
            vertex_count: 0,
            _marker: std::marker::PhantomData,
        }
    }

    pub fn ensure_capacity(
        &mut self,
        device: &wgpu::Device,
        label: &'static str,
        size: wgpu::BufferAddress,
    ) {
        if size > self.capacity {
            *self = Self::new(device, label, size.next_power_of_two());
        }
    }

    pub fn write(&mut self, queue: &wgpu::Queue, vertices: &[V]) {
        self.vertex_count = vertices.len() as u32;
        if !vertices.is_empty() {
            queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(vertices));
        }
    }

    #[inline]
    pub fn used_bytes(&self) -> wgpu::BufferAddress {
        self.vertex_count as wgpu::BufferAddress * size_of::<V>() as wgpu::BufferAddress
    }
}

#[derive(Debug, Clone, Default)]
pub struct CacheTracker {
    frame: u64,
    counter: u64,
}

impl CacheTracker {
    const RETAIN: u64 = 1024;
    const INTERVAL: u64 = 256;

    pub fn advance(&mut self) -> (u64, Option<u64>) {
        self.frame = self.frame.wrapping_add(1).max(1);
        self.counter = self.counter.wrapping_add(1);
        let threshold = self
            .counter
            .is_multiple_of(Self::INTERVAL)
            .then(|| self.frame.saturating_sub(Self::RETAIN));
        (self.frame, threshold)
    }
}

#[inline]
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

pub(crate) fn begin_load_pass<'a>(
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

pub(crate) struct RenderPipelineSpec<'a> {
    pub(crate) label: &'static str,
    pub(crate) shader: &'a wgpu::ShaderModule,
    pub(crate) vertex_entry: &'static str,
    pub(crate) buffers: &'a [wgpu::VertexBufferLayout<'a>],
    pub(crate) bind_group_layouts: &'a [&'a wgpu::BindGroupLayout],
    pub(crate) topology: wgpu::PrimitiveTopology,
}

pub(crate) fn create_render_pipeline(
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
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: spec.shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: spec.topology,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: Default::default(),
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
            buffers: &[SdfVertex::layout()],
            bind_group_layouts: &[],
            topology,
        },
    )
}

#[derive(Debug)]
struct CachedInstance {
    buffer: InstanceBuffer<SdfVertex>,
    last_used: u64,
}

#[derive(Debug)]
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

    pub fn prepare_instance(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &'static str,
        key: K,
        vertices: &[SdfVertex],
    ) {
        let (frame, threshold) = self.cache.advance();
        let required =
            size_of::<SdfVertex>() as wgpu::BufferAddress * vertices.len() as wgpu::BufferAddress;
        let entry = self.instances.entry(key).or_insert_with(|| CachedInstance {
            buffer: InstanceBuffer::new(device, label, required.max(1)),
            last_used: frame,
        });
        entry.last_used = frame;
        if vertices.is_empty() {
            entry.buffer.vertex_count = 0;
        } else {
            entry.buffer.ensure_capacity(device, label, required);
            entry.buffer.write(queue, vertices);
        }
        if let Some(t) = threshold {
            self.instances.retain(|_, e| e.last_used >= t);
        }
    }

    #[inline]
    pub fn instance(&self, key: K) -> Option<&InstanceBuffer<SdfVertex>> {
        self.instances.get(&key).map(|e| &e.buffer)
    }
}

// Spectrogram has different requirements, so it does not use this macro.
#[macro_export]
macro_rules! sdf_primitive {
    ($primitive:ident, $pipeline:ident, $key_ty:ty, $label:expr, $topology:ident, |$self:ident| $key_expr:expr) => {
        impl iced_wgpu::primitive::Primitive for $primitive {
            type Pipeline = $pipeline;

            fn prepare(
                &$self,
                pipeline: &mut Self::Pipeline,
                device: &iced_wgpu::wgpu::Device,
                queue: &iced_wgpu::wgpu::Queue,
                _bounds: &iced::Rectangle,
                viewport: &iced::advanced::graphics::Viewport,
            ) {
                let key: $key_ty = $key_expr;
                pipeline.inner.prepare_instance(device, queue, $label, key, &$self.build_vertices(viewport));
            }

            fn render(
                &$self,
                pipeline: &Self::Pipeline,
                encoder: &mut iced_wgpu::wgpu::CommandEncoder,
                target: &iced_wgpu::wgpu::TextureView,
                clip: &iced::Rectangle<u32>,
            ) {
                let key: $key_ty = $key_expr;
                let Some(inst) = pipeline.inner.instance(key) else { return };
                if inst.vertex_count == 0 { return }
                let mut pass = $crate::visuals::render::common::begin_load_pass(
                    encoder, target, clip, $label,
                );
                pass.set_pipeline(&pipeline.inner.pipeline);
                pass.set_vertex_buffer(0, inst.vertex_buffer.slice(0..inst.used_bytes()));
                pass.draw(0..inst.vertex_count, 0..1);
            }
        }

        #[derive(Debug)]
        pub struct $pipeline { inner: $crate::visuals::render::common::SdfPipeline<$key_ty> }

        impl iced_wgpu::primitive::Pipeline for $pipeline {
            fn new(device: &iced_wgpu::wgpu::Device, _queue: &iced_wgpu::wgpu::Queue, format: iced_wgpu::wgpu::TextureFormat) -> Self {
                Self { inner: $crate::visuals::render::common::SdfPipeline::new(device, format, $label, iced_wgpu::wgpu::PrimitiveTopology::$topology) }
            }
        }
    };
}
