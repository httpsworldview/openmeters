use bytemuck::{Pod, Zeroable};
use iced::Rectangle;
use iced::advanced::graphics::Viewport;
use iced_wgpu::primitive::{Primitive, Storage};
use std::collections::HashMap;
use std::sync::Arc;

pub const SPECTROGRAM_PALETTE_SIZE: usize = 5;

#[derive(Debug, Clone)]
pub struct SpectrogramParams {
    pub bounds: Rectangle,
    pub texture_width: u32,
    pub texture_height: u32,
    pub column_count: u32,
    pub latest_column: u32,
    pub base_data: Option<Arc<[f32]>>,
    pub column_updates: Vec<SpectrogramColumnUpdate>,
    pub palette: [[f32; 4]; SPECTROGRAM_PALETTE_SIZE],
    pub background: [f32; 4],
}

#[derive(Debug, Clone)]
pub struct SpectrogramColumnUpdate {
    pub column_index: u32,
    pub values: Arc<[f32]>,
}

#[derive(Debug)]
pub struct SpectrogramPrimitive {
    params: SpectrogramParams,
}

impl SpectrogramPrimitive {
    pub fn new(params: SpectrogramParams) -> Self {
        Self { params }
    }

    fn key(&self) -> usize {
        self as *const Self as usize
    }

    fn build_vertices(&self, viewport: &Viewport) -> Vec<Vertex> {
        let width = viewport.logical_size().width.max(1.0);
        let height = viewport.logical_size().height.max(1.0);
        let clip = ClipTransform::new(width, height);
        let bounds = self.params.bounds;

        let left = bounds.x;
        let right = bounds.x + bounds.width.max(1.0);
        let top = bounds.y;
        let bottom = bounds.y + bounds.height.max(1.0);

        let vertices = [
            Vertex {
                position: clip.to_clip(left, top),
                tex_coords: [0.0, 0.0],
            },
            Vertex {
                position: clip.to_clip(right, top),
                tex_coords: [1.0, 0.0],
            },
            Vertex {
                position: clip.to_clip(right, bottom),
                tex_coords: [1.0, 1.0],
            },
            Vertex {
                position: clip.to_clip(left, top),
                tex_coords: [0.0, 0.0],
            },
            Vertex {
                position: clip.to_clip(right, bottom),
                tex_coords: [1.0, 1.0],
            },
            Vertex {
                position: clip.to_clip(left, bottom),
                tex_coords: [0.0, 1.0],
            },
        ];

        vertices.to_vec()
    }
}

impl Primitive for SpectrogramPrimitive {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        storage: &mut Storage,
        _bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        if !storage.has::<Pipeline>() {
            storage.store(Pipeline::new(device, format));
        }

        let params = &self.params;
        let Some(pipeline) = storage.get_mut::<Pipeline>() else {
            return;
        };

        if params.texture_width == 0 || params.texture_height == 0 {
            pipeline.prepare_instance(device, queue, self.key(), &[], params);
            return;
        }

        let vertices = self.build_vertices(viewport);
        pipeline.prepare_instance(device, queue, self.key(), &vertices, params);
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

        let Some(resources) = instance.resources() else {
            return;
        };

        if instance.vertex_count() == 0 {
            return;
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Spectrogram pass"),
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

        pass.set_pipeline(pipeline.render_pipeline());
        pass.set_bind_group(0, resources.bind_group(), &[]);
        pass.set_vertex_buffer(0, instance.vertex_buffer_slice());
        pass.draw(0..instance.vertex_count(), 0..1);
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    tex_coords: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SpectrogramUniforms {
    dimensions: [f32; 2],
    latest_column: u32,
    column_count: u32,
    background: [f32; 4],
    palette: [[f32; 4]; SPECTROGRAM_PALETTE_SIZE],
}

impl SpectrogramUniforms {
    fn new(params: &SpectrogramParams) -> Self {
        Self {
            dimensions: [params.texture_width as f32, params.texture_height as f32],
            latest_column: params.latest_column,
            column_count: params.column_count,
            background: params.background,
            palette: params.palette,
        }
    }
}

#[derive(Clone, Copy)]
struct ClipTransform {
    scale_x: f32,
    scale_y: f32,
}

impl ClipTransform {
    fn new(width: f32, height: f32) -> Self {
        Self {
            scale_x: 2.0 / width,
            scale_y: 2.0 / height,
        }
    }

    fn to_clip(&self, x: f32, y: f32) -> [f32; 2] {
        [x * self.scale_x - 1.0, 1.0 - y * self.scale_y]
    }
}

struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    instances: HashMap<usize, Instance>,
}

impl Pipeline {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Spectrogram shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/spectrogram.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Spectrogram bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Spectrogram pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Spectrogram pipeline"),
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
                cull_mode: None,
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
            bind_group_layout,
            instances: HashMap::new(),
        }
    }

    fn prepare_instance(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: usize,
        vertices: &[Vertex],
        params: &SpectrogramParams,
    ) {
        let entry = self
            .instances
            .entry(key)
            .or_insert_with(|| Instance::new(device));

        entry.update_vertices(device, queue, vertices);
        entry.update_resources(device, queue, &self.bind_group_layout, params);
    }

    fn instance(&self, key: usize) -> Option<&Instance> {
        self.instances.get(&key)
    }

    fn render_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }
}

impl Vertex {
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
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
                    format: wgpu::VertexFormat::Float32x2,
                },
            ],
        }
    }
}

struct Instance {
    vertex_buffer: wgpu::Buffer,
    capacity: wgpu::BufferAddress,
    vertex_count: u32,
    resources: Option<GpuResources>,
}

impl Instance {
    fn new(device: &wgpu::Device) -> Self {
        let buffer = create_vertex_buffer(device, 1);
        Self {
            vertex_buffer: buffer,
            capacity: 1,
            vertex_count: 0,
            resources: None,
        }
    }

    fn update_vertices(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, vertices: &[Vertex]) {
        if vertices.is_empty() {
            self.vertex_count = 0;
            return;
        }

        let required = (vertices.len() * std::mem::size_of::<Vertex>()) as wgpu::BufferAddress;
        if required > self.capacity {
            let new_capacity = required.next_power_of_two().max(1);
            self.vertex_buffer = create_vertex_buffer(device, new_capacity);
            self.capacity = new_capacity;
        }

        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(vertices));
        self.vertex_count = vertices.len() as u32;
    }

    fn update_resources(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        params: &SpectrogramParams,
    ) {
        if params.texture_width == 0 || params.texture_height == 0 {
            self.resources = None;
            return;
        }

        let needs_new = self
            .resources
            .as_ref()
            .map(|resources| resources.size != (params.texture_width, params.texture_height))
            .unwrap_or(true);

        if needs_new {
            self.resources = Some(GpuResources::new(
                device,
                layout,
                params.texture_width,
                params.texture_height,
            ));
        }

        if let Some(resources) = self.resources.as_mut() {
            resources.write(queue, params);
        }
    }

    fn vertex_buffer_slice(&self) -> wgpu::BufferSlice<'_> {
        self.vertex_buffer.slice(0..self.used_bytes())
    }

    fn used_bytes(&self) -> wgpu::BufferAddress {
        self.vertex_count as wgpu::BufferAddress
            * std::mem::size_of::<Vertex>() as wgpu::BufferAddress
    }

    fn vertex_count(&self) -> u32 {
        self.vertex_count
    }

    fn resources(&self) -> Option<&GpuResources> {
        self.resources.as_ref()
    }
}
struct GpuResources {
    uniform_buffer: wgpu::Buffer,
    data_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    size: (u32, u32),
}

impl GpuResources {
    fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, width: u32, height: u32) -> Self {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Spectrogram uniform buffer"),
            size: std::mem::size_of::<SpectrogramUniforms>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let element_count = (width.max(1) as usize) * (height.max(1) as usize);
        let data_size = (element_count * std::mem::size_of::<f32>()) as wgpu::BufferAddress;
        let data_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Spectrogram data buffer"),
            size: data_size.max(4),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Spectrogram bind group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &uniform_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &data_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
            ],
        });

        Self {
            uniform_buffer,
            data_buffer,
            bind_group,
            size: (width, height),
        }
    }

    fn write(&mut self, queue: &wgpu::Queue, params: &SpectrogramParams) {
        let (width, height) = self.size;
        let element_count = (width as usize) * (height as usize);
        if element_count == 0 {
            return;
        }

        if let Some(base) = &params.base_data {
            debug_assert_eq!(element_count, base.len());
            queue.write_buffer(&self.data_buffer, 0, bytemuck::cast_slice(base.as_ref()));
        }

        if height > 0 {
            let stride_bytes =
                (height as usize * std::mem::size_of::<f32>()) as wgpu::BufferAddress;
            for update in &params.column_updates {
                if update.values.is_empty() {
                    continue;
                }

                debug_assert_eq!(update.values.len(), height as usize);
                let column = update.column_index.min(width.saturating_sub(1));
                let offset = stride_bytes * column as wgpu::BufferAddress;
                queue.write_buffer(
                    &self.data_buffer,
                    offset,
                    bytemuck::cast_slice(update.values.as_ref()),
                );
            }
        }

        let uniforms = SpectrogramUniforms::new(params);
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }
}

fn create_vertex_buffer(device: &wgpu::Device, size: wgpu::BufferAddress) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Spectrogram vertex buffer"),
        size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
