//! Render a LUFS meter using custom wgpu rendering in iced.

use bytemuck::{Pod, Zeroable};
use iced::Rectangle;
use iced::advanced::graphics::Viewport;
use iced_wgpu::primitive::{Primitive, Storage};
use std::collections::HashMap;
use std::mem;

/// Describes how a LUFS meter should be rendered for a single frame.
#[derive(Debug, Clone)]
pub struct VisualParams {
    pub min_lufs: f32,
    pub max_lufs: f32,
    pub channels: Vec<ChannelVisual>,
    pub channel_gap_fraction: f32,
}

impl VisualParams {
    fn clamp_ratio(&self, value: f32) -> f32 {
        if self.max_lufs - self.min_lufs <= f32::EPSILON {
            return 0.0;
        }
        ((value - self.min_lufs) / (self.max_lufs - self.min_lufs)).clamp(0.0, 1.0)
    }

    fn gap_fraction(&self) -> f32 {
        self.channel_gap_fraction.clamp(0.0, 0.5)
    }
}

#[derive(Debug, Clone)]
pub struct ChannelVisual {
    pub momentary_lufs: f32,
    pub peak_lufs: f32,
    pub background_color: [f32; 4],
    pub fill_color: [f32; 4],
    pub peak_color: [f32; 4],
}

/// Custom primitive that draws a LUFS meter using the iced_wgpu backend.
#[derive(Debug)]
pub struct LufsMeterPrimitive {
    pub params: VisualParams,
}

impl LufsMeterPrimitive {
    pub fn new(params: VisualParams) -> Self {
        Self { params }
    }

    fn key(&self) -> usize {
        self as *const Self as usize
    }

    fn build_vertices(&self, bounds: &Rectangle, viewport: &Viewport) -> Vec<Vertex> {
        if self.params.channels.is_empty() {
            return Vec::new();
        }

        let viewport_size = viewport.logical_size();
        let width = viewport_size.width.max(1.0);
        let height = viewport_size.height.max(1.0);

        let x0 = bounds.x;
        let y0 = bounds.y;
        let y1 = bounds.y + bounds.height;

        let channel_count = self.params.channels.len();
        let gap = (bounds.width * self.params.gap_fraction()).min(bounds.width);
        let total_gap = gap * (channel_count.saturating_sub(1) as f32);
        let available_width = (bounds.width - total_gap).max(0.0);
        let bar_width = if channel_count == 0 {
            0.0
        } else {
            available_width / channel_count as f32
        };

        let mut vertices = Vec::with_capacity(channel_count * 18);

        for (index, channel) in self.params.channels.iter().enumerate() {
            let bar_x0 = x0 + index as f32 * (bar_width + gap);
            let mut bar_x1 = bar_x0 + bar_width;
            if index == channel_count - 1 {
                bar_x1 = (bounds.x + bounds.width).max(bar_x1);
            }

            let fill_ratio = self.params.clamp_ratio(channel.momentary_lufs);
            let fill_top = y1 - (y1 - y0) * fill_ratio;

            let peak_ratio = self.params.clamp_ratio(channel.peak_lufs);
            let peak_center = y1 - (y1 - y0) * peak_ratio;
            let peak_thickness = (bounds.height * 0.015).clamp(1.0, 6.0);
            let peak_half = peak_thickness * 0.5;
            let peak_top = (peak_center - peak_half).clamp(y0, y1);
            let peak_bottom = (peak_center + peak_half).clamp(y0, y1);

            vertices.extend(quad_vertices(
                bar_x0,
                y0,
                bar_x1,
                y1,
                width,
                height,
                channel.background_color,
            ));

            vertices.extend(quad_vertices(
                bar_x0,
                fill_top,
                bar_x1,
                y1,
                width,
                height,
                channel.fill_color,
            ));

            if peak_bottom > peak_top {
                vertices.extend(quad_vertices(
                    bar_x0,
                    peak_top,
                    bar_x1,
                    peak_bottom,
                    width,
                    height,
                    channel.peak_color,
                ));
            }
        }

        vertices
    }
}

impl Primitive for LufsMeterPrimitive {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        storage: &mut Storage,
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        if !storage.has::<Pipeline>() {
            storage.store(Pipeline::new(device, format));
        }

        let pipeline = storage
            .get_mut::<Pipeline>()
            .expect("pipeline must exist after storage check");

        let vertices = self.build_vertices(bounds, viewport);
        pipeline.prepare_instance(device, queue, self.key(), &vertices);
    }

    fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        storage: &Storage,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        let Some(pipeline) = storage.get::<Pipeline>() else {
            return;
        };

        let Some(instance) = pipeline.instance(self.key()) else {
            return;
        };

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("LUFS meter pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        pass.set_scissor_rect(
            clip_bounds.x,
            clip_bounds.y,
            clip_bounds.width.max(1),
            clip_bounds.height.max(1),
        );
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_vertex_buffer(0, instance.vertex_buffer.slice(0..instance.used_bytes()));
        pass.draw(0..instance.vertex_count, 0..1);
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    color: [f32; 4],
}

impl Vertex {
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Vertex>() as wgpu::BufferAddress,
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
            ],
        }
    }
}

fn quad_vertices(
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    viewport_width: f32,
    viewport_height: f32,
    color: [f32; 4],
) -> [Vertex; 6] {
    let tl = to_clip(x0, y0, viewport_width, viewport_height);
    let tr = to_clip(x1, y0, viewport_width, viewport_height);
    let bl = to_clip(x0, y1, viewport_width, viewport_height);
    let br = to_clip(x1, y1, viewport_width, viewport_height);

    [
        Vertex {
            position: tl,
            color,
        },
        Vertex {
            position: bl,
            color,
        },
        Vertex {
            position: br,
            color,
        },
        Vertex {
            position: tl,
            color,
        },
        Vertex {
            position: br,
            color,
        },
        Vertex {
            position: tr,
            color,
        },
    ]
}

fn to_clip(x: f32, y: f32, width: f32, height: f32) -> [f32; 2] {
    let nx = (x / width) * 2.0 - 1.0;
    let ny = 1.0 - (y / height) * 2.0;
    [nx, ny]
}

#[derive(Debug)]
struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    instances: HashMap<usize, InstanceBuffer>,
}

impl Pipeline {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("LUFS meter shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/lufs_meter.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("LUFS meter pipeline layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("LUFS meter pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[Vertex::layout()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
        });

        Self {
            pipeline,
            instances: HashMap::new(),
        }
    }

    fn prepare_instance(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: usize,
        vertices: &[Vertex],
    ) {
        let required_size = (vertices.len() * mem::size_of::<Vertex>()) as wgpu::BufferAddress;
        let entry = self
            .instances
            .entry(key)
            .or_insert_with(|| InstanceBuffer::new(device, required_size));
        entry.ensure_capacity(device, required_size);
        queue.write_buffer(&entry.vertex_buffer, 0, bytemuck::cast_slice(vertices));
        entry.vertex_count = vertices.len() as u32;
    }

    fn instance(&self, key: usize) -> Option<&InstanceBuffer> {
        self.instances.get(&key)
    }
}

#[derive(Debug)]
struct InstanceBuffer {
    vertex_buffer: wgpu::Buffer,
    capacity: wgpu::BufferAddress,
    vertex_count: u32,
}

impl InstanceBuffer {
    fn new(device: &wgpu::Device, size: wgpu::BufferAddress) -> Self {
        let buffer = create_vertex_buffer(device, size.max(1));
        Self {
            vertex_buffer: buffer,
            capacity: size.max(1),
            vertex_count: 0,
        }
    }

    fn ensure_capacity(&mut self, device: &wgpu::Device, size: wgpu::BufferAddress) {
        if size <= self.capacity {
            return;
        }

        let new_capacity = size.next_power_of_two().max(1);
        self.vertex_buffer = create_vertex_buffer(device, new_capacity);
        self.capacity = new_capacity;
    }

    fn used_bytes(&self) -> wgpu::BufferAddress {
        self.vertex_count as wgpu::BufferAddress * mem::size_of::<Vertex>() as wgpu::BufferAddress
    }
}

fn create_vertex_buffer(device: &wgpu::Device, size: wgpu::BufferAddress) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("LUFS meter vertex buffer"),
        size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
