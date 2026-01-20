//! Common rendering utilities shared across visualization primitives.

use bytemuck::{Pod, Zeroable};
use iced::advanced::graphics::Viewport;
use iced_wgpu::wgpu;
use std::collections::HashMap;
use std::mem::size_of;

/// Transforms logical screen coordinates to clip space coordinates.
#[derive(Clone, Copy)]
pub struct ClipTransform(f32, f32);

impl ClipTransform {
    pub fn new(w: f32, h: f32) -> Self {
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

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct SdfVertex {
    pub position: [f32; 2],
    pub color: [f32; 4],
    pub params: [f32; 4],
}

impl SdfVertex {
    pub const SOLID_PARAMS: [f32; 4] = [0.0, 0.0, 1000.0, 1.0];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 8,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 24,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x4,
                },
            ],
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
    pub fn antialiased(pos: [f32; 2], color: [f32; 4], dist: f32, half: f32, feather: f32) -> Self {
        Self {
            position: pos,
            color,
            params: [dist, 0.0, half, feather],
        }
    }
}

/// Manages a growable GPU vertex buffer for a single primitive instance.
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

/// Produces eviction thresholds for pruning stale cache entries.
#[derive(Debug, Clone, Default)]
pub struct CacheTracker {
    frame: u64,
    counter: u64,
}

impl CacheTracker {
    const RETAIN: u64 = 1024;
    const INTERVAL: u64 = 256;

    /// Returns `(frame, Some(threshold))` every `INTERVAL` frames for eviction.
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

/// Creates a render pipeline using `sdf.wgsl` with the given topology.
pub fn create_sdf_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    label: &'static str,
    topology: wgpu::PrimitiveTopology,
) -> wgpu::RenderPipeline {
    let shader = create_shader_module(device, label, include_str!("shaders/sdf.wgsl"));
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(
            &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[],
                push_constant_ranges: &[],
            }),
        ),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[SdfVertex::layout()],
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
            topology,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: Default::default(),
        multiview: None,
        cache: None,
    })
}

/// Writes a tightly packed texture region.
#[inline]
pub fn write_texture_region(
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

/// Generates six SDF vertices forming a quad (two triangles).
/// Pass identical colors for solid fill, or different for vertical gradient.
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

/// Quad with per-edge colors for smooth vertical gradients.
#[inline]
pub fn gradient_quad_vertices(
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

/// Generates six SDF vertices forming an antialiased line segment.
#[inline]
pub fn line_vertices(
    p0: (f32, f32),
    p1: (f32, f32),
    c0: [f32; 4],
    c1: [f32; 4],
    width: f32,
    feather: f32,
    clip: ClipTransform,
) -> [SdfVertex; 6] {
    let (dx, dy) = (p1.0 - p0.0, p1.1 - p0.1);
    let inv = (dx * dx + dy * dy).sqrt().max(1e-6).recip();
    let (half, outer) = (width * 0.5, width + feather);
    let (ox, oy) = (-dy * inv * outer, dx * inv * outer);
    let v = |px, py, c, d| SdfVertex {
        position: clip.to_clip(px, py),
        color: c,
        params: [d, 0.0, half, feather],
    };
    [
        v(p0.0 - ox, p0.1 - oy, c0, -outer),
        v(p0.0 + ox, p0.1 + oy, c0, outer),
        v(p1.0 - ox, p1.1 - oy, c1, -outer),
        v(p0.0 + ox, p0.1 + oy, c0, outer),
        v(p1.0 + ox, p1.1 + oy, c1, outer),
        v(p1.0 - ox, p1.1 - oy, c1, -outer),
    ]
}

/// Generates six SDF vertices forming an antialiased dot.
#[inline]
pub fn dot_vertices(
    cx: f32,
    cy: f32,
    radius: f32,
    feather: f32,
    color: [f32; 4],
    clip: ClipTransform,
) -> [SdfVertex; 6] {
    let o = radius + feather;
    let v = |px, py, ox, oy| SdfVertex {
        position: clip.to_clip(px, py),
        color,
        params: [ox, oy, radius, feather],
    };
    [
        v(cx - o, cy - o, -o, -o),
        v(cx - o, cy + o, -o, o),
        v(cx + o, cy - o, o, -o),
        v(cx + o, cy - o, o, -o),
        v(cx - o, cy + o, -o, o),
        v(cx + o, cy + o, o, o),
    ]
}

#[derive(Debug)]
pub struct CachedInstance {
    pub buffer: InstanceBuffer<SdfVertex>,
    pub last_used: u64,
}

/// Pipeline + instance cache for SDF-based primitives.
#[derive(Debug)]
pub struct SdfPipeline<K> {
    pub pipeline: wgpu::RenderPipeline,
    pub instances: HashMap<K, CachedInstance>,
    pub cache: CacheTracker,
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

/// builds an iced_wgpu primitive
/// spectrogram has different requirements, so it does not use this macro
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
                let mut pass = encoder.begin_render_pass(&iced_wgpu::wgpu::RenderPassDescriptor {
                    label: Some($label),
                    color_attachments: &[Some(iced_wgpu::wgpu::RenderPassColorAttachment {
                        view: target, resolve_target: None, depth_slice: None,
                        ops: iced_wgpu::wgpu::Operations {
                            load: iced_wgpu::wgpu::LoadOp::Load,
                            store: iced_wgpu::wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None,
                });
                pass.set_scissor_rect(clip.x, clip.y, clip.width.max(1), clip.height.max(1));
                pass.set_pipeline(&pipeline.inner.pipeline);
                pass.set_vertex_buffer(0, inst.vertex_buffer.slice(0..inst.used_bytes()));
                pass.draw(0..inst.vertex_count, 0..1);
            }
        }

        #[derive(Debug)]
        pub struct $pipeline { inner: $crate::ui::render::common::SdfPipeline<$key_ty> }

        impl iced_wgpu::primitive::Pipeline for $pipeline {
            fn new(device: &iced_wgpu::wgpu::Device, _queue: &iced_wgpu::wgpu::Queue, format: iced_wgpu::wgpu::TextureFormat) -> Self {
                Self { inner: $crate::ui::render::common::SdfPipeline::new(device, format, $label, iced_wgpu::wgpu::PrimitiveTopology::$topology) }
            }
        }
    };
}
