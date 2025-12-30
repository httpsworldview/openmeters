//! Spectrogram rendering pipeline for GPU-accelerated time-frequency visualization.

use bytemuck::{Pod, Zeroable};
use iced::Rectangle;
use iced::advanced::graphics::Viewport;
use iced_wgpu::primitive::{self, Primitive};
use iced_wgpu::wgpu;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::ui::render::common::{
    CacheTracker, ClipTransform, InstanceBuffer, create_shader_module, write_texture_region,
};

pub const SPECTROGRAM_PALETTE_SIZE: usize = 5;
pub const PALETTE_LUT_SIZE: u32 = 256;

// capacity is power of two for wrapping
const FLAG_POW2: u32 = 1;

const MAX_TEXTURE_DIM: u32 = 8192;

macro_rules! extent3d {
    ($w:expr, $h:expr) => {
        wgpu::Extent3d {
            width: ($w).max(1),
            height: ($h).max(1),
            depth_or_array_layers: 1,
        }
    };
}

// public API

#[derive(Debug, Clone)]
pub struct SpectrogramParams {
    pub instance_key: u64,
    pub bounds: Rectangle,
    pub texture_width: u32,
    pub texture_height: u32,
    pub column_count: u32,
    pub latest_column: u32,
    pub base_data: Option<Arc<Vec<f32>>>,
    pub column_updates: Vec<SpectrogramColumnUpdate>,
    pub palette: [[f32; 4]; SPECTROGRAM_PALETTE_SIZE],
    pub background: [f32; 4],
    pub contrast: f32,
    pub uv_y_range: [f32; 2],
    pub screen_height: f32,
}

#[derive(Debug, Clone)]
pub struct SpectrogramColumnUpdate {
    pub column_index: u32,
    pub values: Arc<ColumnBuffer>,
}

// col buffer pool

#[derive(Clone, Debug, Default)]
pub struct ColumnBufferPool(Arc<Mutex<Vec<Vec<f32>>>>);

impl ColumnBufferPool {
    pub fn acquire(&self, len: usize) -> Vec<f32> {
        let mut pool = self.0.lock().unwrap();
        pool.iter()
            .rposition(|b| b.capacity() >= len)
            .map(|i| {
                let mut b = pool.swap_remove(i);
                b.clear();
                b.resize(len, 0.0);
                b
            })
            .unwrap_or_else(|| vec![0.0; len])
    }

    pub fn release(&self, mut buf: Vec<f32>) {
        if buf.capacity() <= 16_384 {
            buf.clear();
            let mut pool = self.0.lock().unwrap();
            if pool.len() < 64 {
                pool.push(buf);
            }
        }
    }
}

#[derive(Debug)]
pub struct ColumnBuffer {
    pool: ColumnBufferPool,
    data: Option<Vec<f32>>,
}

impl ColumnBuffer {
    pub fn new(data: Vec<f32>, pool: ColumnBufferPool) -> Self {
        Self {
            pool,
            data: Some(data),
        }
    }
    pub fn as_slice(&self) -> &[f32] {
        self.data.as_deref().unwrap_or(&[])
    }
}

impl Drop for ColumnBuffer {
    fn drop(&mut self) {
        if let Some(d) = self.data.take() {
            self.pool.release(d);
        }
    }
}

// primitive

#[derive(Debug)]
pub struct SpectrogramPrimitive {
    params: SpectrogramParams,
}

impl SpectrogramPrimitive {
    pub fn new(params: SpectrogramParams) -> Self {
        Self { params }
    }
    fn key(&self) -> u64 {
        self.params.instance_key
    }

    fn build_quad(&self, vp: &Viewport) -> [Vertex; 6] {
        let c = ClipTransform::from_viewport(vp);
        let b = self.params.bounds;
        let (l, r, t, bt) = (b.x, b.x + b.width.max(1.0), b.y, b.y + b.height.max(1.0));
        [
            Vertex::new(c.to_clip(l, t), [0.0, 0.0]),
            Vertex::new(c.to_clip(r, t), [1.0, 0.0]),
            Vertex::new(c.to_clip(r, bt), [1.0, 1.0]),
            Vertex::new(c.to_clip(l, t), [0.0, 0.0]),
            Vertex::new(c.to_clip(r, bt), [1.0, 1.0]),
            Vertex::new(c.to_clip(l, bt), [0.0, 1.0]),
        ]
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
        let p = &self.params;
        let verts = (p.texture_width > 0 && p.texture_height > 0).then(|| self.build_quad(vp));
        pipeline.prepare(device, queue, self.key(), verts.as_ref(), p);
    }

    fn render(
        &self,
        pipeline: &Self::Pipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip: &Rectangle<u32>,
    ) {
        let Some(inst) = pipeline.instances.get(&self.key()) else {
            return;
        };
        let Some(res) = inst.resources.as_ref() else {
            return;
        };
        if inst.vertices.vertex_count == 0 {
            return;
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Spectrogram pass"),
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
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &res.bind_group, &[]);
        pass.set_vertex_buffer(
            0,
            inst.vertices
                .vertex_buffer
                .slice(0..inst.vertices.used_bytes()),
        );
        pass.draw(0..inst.vertices.vertex_count, 0..1);
    }
}

// gpu types

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    tex_coords: [f32; 2],
}

impl Vertex {
    const fn new(position: [f32; 2], tex_coords: [f32; 2]) -> Self {
        Self {
            position,
            tex_coords,
        }
    }
    const SIZE: wgpu::BufferAddress = std::mem::size_of::<Self>() as wgpu::BufferAddress;
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: Self::SIZE,
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

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, PartialEq)]
struct Uniforms {
    dims_wrap_flags: [f32; 4],
    latest_count: [u32; 4],
    style: [f32; 4],
    background: [f32; 4],
}

impl Uniforms {
    fn from_params(p: &SpectrogramParams) -> Self {
        let cap = p.texture_width;
        let pow2 = cap > 0 && cap.is_power_of_two();
        Self {
            dims_wrap_flags: [
                cap as f32,
                p.texture_height as f32,
                f32::from_bits(if pow2 { cap - 1 } else { 0 }),
                f32::from_bits(if pow2 { FLAG_POW2 } else { 0 }),
            ],
            latest_count: [p.latest_column, p.column_count, 0, 0],
            style: [
                p.contrast.max(0.01),
                p.uv_y_range[0],
                p.uv_y_range[1],
                p.screen_height.max(1.0),
            ],
            background: p.background,
        }
    }
}

// pipeline

pub struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    instances: HashMap<u64, Instance>,
    cache: CacheTracker,
}

impl primitive::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = create_shader_module(
            device,
            "Spectrogram shader",
            include_str!("shaders/spectrogram.wgsl"),
        );

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Spectrogram BGL"),
            entries: &[
                bgl_entry(
                    0,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                bgl_entry(
                    1,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                bgl_entry(
                    2,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D1,
                        multisampled: false,
                    },
                ),
                bgl_entry(
                    3,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
            ],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Spectrogram pipeline"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: None,
                    bind_group_layouts: &[&layout],
                    push_constant_ranges: &[],
                }),
            ),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Vertex::layout()],
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
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            layout,
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
        verts: Option<&[Vertex; 6]>,
        params: &SpectrogramParams,
    ) {
        let (frame, prune) = self.cache.advance();
        let inst = self
            .instances
            .entry(key)
            .or_insert_with(|| Instance::new(device));
        inst.last_used = frame;
        inst.update(device, queue, &self.layout, verts, params);
        if let Some(t) = prune {
            self.instances.retain(|_, i| i.last_used >= t);
        }
    }
}

fn bgl_entry(binding: u32, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty,
        count: None,
    }
}

// instance

struct Instance {
    vertices: InstanceBuffer<Vertex>,
    resources: Option<Resources>,
    last_used: u64,
}

impl Instance {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            vertices: InstanceBuffer::new(device, "Spectrogram VB", Vertex::SIZE),
            resources: None,
            last_used: 0,
        }
    }

    fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        verts: Option<&[Vertex; 6]>,
        p: &SpectrogramParams,
    ) {
        match verts {
            Some(v) => {
                self.vertices
                    .ensure_capacity(device, "Spectrogram VB", Vertex::SIZE * 6);
                self.vertices.write(queue, v);
            }
            None => self.vertices.vertex_count = 0,
        }

        if p.texture_width == 0 || p.texture_height == 0 {
            self.resources = None;
            return;
        }

        let res = self.resources.get_or_insert_with(|| {
            Resources::new(device, layout, p.texture_width, p.texture_height)
        });
        res.sync(device, queue, layout, p);
    }
}

// gpu resources

struct Resources {
    uniform_buf: wgpu::Buffer,
    magnitude_tex: wgpu::Texture,
    magnitude_view: wgpu::TextureView,
    magnitude_cap: (u32, u32),
    palette_tex: wgpu::Texture,
    palette_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    bind_group: wgpu::BindGroup,
    uniform_cache: Uniforms,
    palette_cache: [[f32; 4]; SPECTROGRAM_PALETTE_SIZE],
}

impl Resources {
    fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, w: u32, h: u32) -> Self {
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Spectrogram UB"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (magnitude_tex, magnitude_view, magnitude_cap) = create_magnitude(device, w, h);
        let (palette_tex, palette_view) = create_palette(device);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Spectrogram sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bind_group = make_bind_group(
            device,
            layout,
            &uniform_buf,
            &magnitude_view,
            &palette_view,
            &sampler,
        );

        Self {
            uniform_buf,
            magnitude_tex,
            magnitude_view,
            magnitude_cap,
            palette_tex,
            palette_view,
            sampler,
            bind_group,
            uniform_cache: Uniforms {
                dims_wrap_flags: [0.0; 4],
                latest_count: [0; 4],
                style: [0.0; 4],
                background: [0.0; 4],
            },
            palette_cache: [[0.0; 4]; SPECTROGRAM_PALETTE_SIZE],
        }
    }

    fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        p: &SpectrogramParams,
    ) {
        self.grow_magnitude(device, queue, layout, p.texture_width, p.texture_height);
        self.write_columns(queue, p);
        self.write_uniforms(queue, p);
        self.write_palette(queue, p);
    }

    fn grow_magnitude(
        &mut self,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        w: u32,
        h: u32,
    ) {
        let (tw, th) = (w.clamp(1, MAX_TEXTURE_DIM), h.clamp(1, MAX_TEXTURE_DIM));
        if tw <= self.magnitude_cap.0 && th <= self.magnitude_cap.1 {
            return;
        }

        let new_cap = (
            tw.max(self.magnitude_cap.0),
            th.max(self.magnitude_cap.1),
        );

        self.magnitude_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Spectrogram magnitude"),
            size: extent3d!(new_cap.1, new_cap.0),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[wgpu::TextureFormat::R32Float],
        });
        self.magnitude_view = self.magnitude_tex.create_view(&Default::default());
        self.magnitude_cap = new_cap;
        self.bind_group = make_bind_group(
            device,
            layout,
            &self.uniform_buf,
            &self.magnitude_view,
            &self.palette_view,
            &self.sampler,
        );
    }

    fn write_columns(&mut self, queue: &wgpu::Queue, p: &SpectrogramParams) {
        let (w, h) = (
            p.texture_width.min(self.magnitude_cap.0),
            p.texture_height.min(self.magnitude_cap.1),
        );
        if w == 0 || h == 0 {
            return;
        }

        if let Some(base) = &p.base_data {
            for (col, vals) in base.chunks(h as usize).enumerate().take(w as usize) {
                self.write_col(queue, col as u32, h, vals);
            }
        }
        for u in &p.column_updates {
            if !u.values.as_slice().is_empty() {
                self.write_col(queue, u.column_index.min(w - 1), h, u.values.as_slice());
            }
        }
    }

    fn write_col(&self, queue: &wgpu::Queue, col: u32, h: u32, vals: &[f32]) {
        write_texture_region(
            queue,
            &self.magnitude_tex,
            wgpu::Origin3d { x: 0, y: col, z: 0 },
            extent3d!(h, 1),
            h * 4,
            bytemuck::cast_slice(vals),
        );
    }

    fn write_uniforms(&mut self, queue: &wgpu::Queue, p: &SpectrogramParams) {
        let u = Uniforms::from_params(p);
        if u != self.uniform_cache {
            queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&u));
            self.uniform_cache = u;
        }
    }

    fn write_palette(&mut self, queue: &wgpu::Queue, p: &SpectrogramParams) {
        if p.palette == self.palette_cache {
            return;
        }
        let lut = build_palette_lut(&p.palette);
        write_texture_region(
            queue,
            &self.palette_tex,
            wgpu::Origin3d::ZERO,
            extent3d!(PALETTE_LUT_SIZE, 1),
            PALETTE_LUT_SIZE * 4,
            &lut,
        );
        self.palette_cache = p.palette;
    }
}

fn create_magnitude(
    device: &wgpu::Device,
    w: u32,
    h: u32,
) -> (wgpu::Texture, wgpu::TextureView, (u32, u32)) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Spectrogram magnitude"),
        size: extent3d!(h.max(1), w.max(1)),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[wgpu::TextureFormat::R32Float],
    });
    let view = tex.create_view(&Default::default());
    (tex, view, (w.max(1), h.max(1)))
}

fn create_palette(device: &wgpu::Device) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Spectrogram palette"),
        size: wgpu::Extent3d {
            width: PALETTE_LUT_SIZE,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D1,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[wgpu::TextureFormat::Rgba8Unorm],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D1),
        ..Default::default()
    });
    (tex, view)
}

fn make_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    ub: &wgpu::Buffer,
    mag: &wgpu::TextureView,
    pal: &wgpu::TextureView,
    sam: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Spectrogram BG"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: ub.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(mag),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(pal),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(sam),
            },
        ],
    })
}

fn build_palette_lut(palette: &[[f32; 4]; SPECTROGRAM_PALETTE_SIZE]) -> Vec<u8> {
    let n = PALETTE_LUT_SIZE as usize;
    (0..n)
        .flat_map(|i| {
            let t = i as f32 / (n - 1).max(1) as f32 * (SPECTROGRAM_PALETTE_SIZE - 1) as f32;
            let (lo, hi, f) = (
                t.floor() as usize,
                (t.floor() as usize + 1).min(SPECTROGRAM_PALETTE_SIZE - 1),
                t.fract(),
            );
            (0..4).map(move |c| {
                ((palette[lo][c] + (palette[hi][c] - palette[lo][c]) * f).clamp(0.0, 1.0) * 255.0)
                    .round() as u8
            })
        })
        .collect()
}
