use iced::Rectangle;
use iced::advanced::graphics::Viewport;
use iced_wgpu::primitive::{Primitive, Storage};
use iced_wgpu::wgpu;
use std::collections::HashMap;
use std::sync::Arc;

use crate::ui::render::common::{ClipTransform, InstanceBuffer, SimplePipeline, SimpleVertex};
use crate::ui::render::geometry::compute_normals;

#[derive(Debug, Clone, Copy)]
pub struct PreviewSample {
    pub min: f32,
    pub max: f32,
    pub color: [f32; 4],
}

#[derive(Debug, Clone)]
pub struct WaveformParams {
    pub bounds: Rectangle,
    pub channels: usize,
    pub column_width: f32,
    pub columns: usize,
    pub samples: Arc<Vec<[f32; 2]>>,
    pub colors: Arc<Vec<[f32; 4]>>,
    pub preview_samples: Arc<Vec<PreviewSample>>,
    pub preview_progress: f32,
    pub fill_alpha: f32,
    pub line_alpha: f32,
    pub vertical_padding: f32,
    pub channel_gap: f32,
    pub amplitude_scale: f32,
    pub stroke_width: f32,
    pub instance_key: u64,
}

impl WaveformParams {
    pub fn preview_active(&self) -> bool {
        self.preview_progress > 0.0 && self.preview_samples.len() >= self.channels
    }
}

#[derive(Debug)]
pub struct WaveformPrimitive {
    params: WaveformParams,
}

impl WaveformPrimitive {
    pub fn new(params: WaveformParams) -> Self {
        Self { params }
    }

    fn key(&self) -> u64 {
        self.params.instance_key
    }

    fn build_vertices(&self, viewport: &Viewport) -> Vec<SimpleVertex> {
        let channels = self.params.channels.max(1);
        let columns = self.params.columns;
        let total_samples = channels * columns;
        if (columns > 0
            && (self.params.samples.len() < total_samples
                || self.params.colors.len() < total_samples))
            || (columns == 0 && !self.params.preview_active())
        {
            return Vec::new();
        }

        let bounds = self.params.bounds;
        let clip = ClipTransform::from_viewport(viewport);

        let column_width = self.params.column_width.max(0.5);
        let preview_width = if self.params.preview_active() {
            column_width
        } else {
            0.0
        };
        let right_edge = bounds.x + bounds.width;

        let vertical_padding = self.params.vertical_padding.max(0.0);
        let channel_gap = self.params.channel_gap.max(0.0);
        let usable_height = (bounds.height
            - vertical_padding * 2.0
            - channel_gap * (channels.saturating_sub(1) as f32))
            .max(1.0);
        let channel_height = usable_height / channels as f32;
        let amplitude_scale = channel_height * 0.5 * self.params.amplitude_scale.max(0.01);
        let half_stroke = self.params.stroke_width.max(0.5) * 0.5;

        let mut vertices = Vec::with_capacity(channels * (columns + 1) * 6);

        let append_strip = |dest: &mut Vec<SimpleVertex>, strip: Vec<SimpleVertex>| {
            if strip.is_empty() {
                return;
            }
            if !dest.is_empty()
                && let Some(last) = dest.last().copied()
            {
                let mut iter = strip.into_iter();
                let first = iter.next().unwrap();
                dest.push(last);
                dest.push(last);
                dest.push(first);
                dest.push(first);
                dest.extend(iter);
                return;
            }
            dest.extend(strip);
        };

        for channel in 0..channels {
            let top = bounds.y + vertical_padding + channel as f32 * (channel_height + channel_gap);
            let center = top + channel_height * 0.5;

            let mut area_vertices = Vec::with_capacity((columns + 1) * 2);
            for index in 0..columns {
                let sample_index = channel * columns + index;
                let pair = self.params.samples[sample_index];
                let mut min_value = pair[0];
                let mut max_value = pair[1];
                if min_value > max_value {
                    std::mem::swap(&mut min_value, &mut max_value);
                }
                min_value = min_value.clamp(-1.0, 1.0);
                max_value = max_value.clamp(-1.0, 1.0);

                let x =
                    (right_edge - preview_width - column_width * ((columns - 1 - index) as f32))
                        .round();
                let top_y = center - max_value * amplitude_scale;
                let bottom_y = center - min_value * amplitude_scale;
                let color = self
                    .params
                    .colors
                    .get(channel * columns + index)
                    .copied()
                    .unwrap_or([1.0; 4]);
                let fill_color = [color[0], color[1], color[2], self.params.fill_alpha];

                area_vertices.push(SimpleVertex {
                    position: clip.to_clip(x, top_y),
                    color: fill_color,
                });
                area_vertices.push(SimpleVertex {
                    position: clip.to_clip(x, bottom_y),
                    color: fill_color,
                });
            }

            if self.params.preview_active() {
                let sample = self.params.preview_samples[channel];
                let mut min_value = sample.min;
                let mut max_value = sample.max;
                if min_value > max_value {
                    std::mem::swap(&mut min_value, &mut max_value);
                }
                min_value = min_value.clamp(-1.0, 1.0);
                max_value = max_value.clamp(-1.0, 1.0);
                let x = right_edge.round();
                let top_y = center - max_value * amplitude_scale;
                let bottom_y = center - min_value * amplitude_scale;
                let preview_base = sample.color;
                let fill_color = [
                    preview_base[0],
                    preview_base[1],
                    preview_base[2],
                    self.params.fill_alpha,
                ];

                area_vertices.push(SimpleVertex {
                    position: clip.to_clip(x, top_y),
                    color: fill_color,
                });
                area_vertices.push(SimpleVertex {
                    position: clip.to_clip(x, bottom_y),
                    color: fill_color,
                });
            }

            append_strip(&mut vertices, area_vertices);

            let mut positions = Vec::with_capacity(columns + 1);
            let mut line_colors = Vec::with_capacity(columns + 1);
            for index in 0..columns {
                let sample_index = channel * columns + index;
                let pair = self.params.samples[sample_index];
                let min_value = pair[0].clamp(-1.0, 1.0);
                let max_value = pair[1].clamp(-1.0, 1.0);
                let average = 0.5 * (min_value + max_value);
                let x =
                    (right_edge - preview_width - column_width * ((columns - 1 - index) as f32))
                        .round();
                let y = center - average * amplitude_scale;
                positions.push((x, y));
                line_colors.push(
                    self.params
                        .colors
                        .get(channel * columns + index)
                        .copied()
                        .unwrap_or([1.0; 4]),
                );
            }

            if self.params.preview_active() {
                let sample = self.params.preview_samples[channel];
                let min_value = sample.min.clamp(-1.0, 1.0);
                let max_value = sample.max.clamp(-1.0, 1.0);
                let average = 0.5 * (min_value + max_value);
                let x = right_edge.round();
                let y = center - average * amplitude_scale;
                positions.push((x, y));
                line_colors.push(sample.color);
            }

            if positions.len() < 2 {
                continue;
            }

            let normals = compute_normals(&positions);
            let mut line_vertices = Vec::with_capacity(positions.len() * 2);
            for ((position, normal), base) in
                positions.iter().zip(normals.iter()).zip(line_colors.iter())
            {
                let line_color = [base[0], base[1], base[2], self.params.line_alpha];
                let offset_x = normal.0 * half_stroke;
                let offset_y = normal.1 * half_stroke;

                let left = clip.to_clip(position.0 - offset_x, position.1 - offset_y);
                let right = clip.to_clip(position.0 + offset_x, position.1 + offset_y);

                line_vertices.push(SimpleVertex {
                    position: left,
                    color: line_color,
                });
                line_vertices.push(SimpleVertex {
                    position: right,
                    color: line_color,
                });
            }

            append_strip(&mut vertices, line_vertices);
        }

        vertices
    }
}

impl Primitive for WaveformPrimitive {
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

        let pipeline = storage
            .get_mut::<Pipeline>()
            .expect("pipeline must exist after storage check");

        let vertices = self.build_vertices(viewport);
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

        if instance.vertex_count == 0 {
            return;
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Waveform pass"),
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
        pass.set_pipeline(&pipeline.inner.pipeline);
        pass.set_vertex_buffer(0, instance.vertex_buffer.slice(0..instance.used_bytes()));
        pass.draw(0..instance.vertex_count, 0..1);
    }
}

#[derive(Debug)]
struct Pipeline {
    inner: SimplePipeline<u64>,
}

impl Pipeline {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Waveform shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/waveform.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Waveform pipeline layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Waveform pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[SimpleVertex::layout()],
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
                topology: wgpu::PrimitiveTopology::TriangleStrip,
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
            inner: SimplePipeline {
                pipeline,
                instances: HashMap::new(),
            },
        }
    }

    fn prepare_instance(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: u64,
        vertices: &[SimpleVertex],
    ) {
        self.inner
            .prepare_instance(device, queue, "Waveform vertex buffer", key, vertices);
    }

    fn instance(&self, key: u64) -> Option<&InstanceBuffer<SimpleVertex>> {
        self.inner.instance(key)
    }
}
