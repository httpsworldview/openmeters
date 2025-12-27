use iced::Rectangle;
use iced::advanced::graphics::Viewport;
use iced_wgpu::primitive::{self, Primitive};
use iced_wgpu::wgpu;

use crate::dsp::stereometer::BandCorrelation;
use crate::ui::render::common::{
    ClipTransform, SdfPipeline, SdfVertex, dot_vertices, gradient_quad_vertices, line_vertices,
    quad_vertices,
};
use crate::ui::settings::{CorrelationMeterMode, StereometerMode};
use crate::ui::theme::{CORRELATION_METER_PALETTE as CORR_PAL, color_to_rgba};

const CORR_W: f32 = 28.0;
const CORR_PAD: f32 = 4.0;
const BAND_GAP: f32 = 2.0;

#[derive(Debug, Clone)]
pub struct StereometerParams {
    pub key: u64,
    pub bounds: Rectangle,
    pub points: Vec<(f32, f32)>,
    pub trace_color: [f32; 4],
    pub mode: StereometerMode,
    pub rotation: i8,
    pub flip: bool,
    pub correlation_meter: CorrelationMeterMode,
    pub corr_trail: Vec<f32>,
    pub band_trail: Vec<BandCorrelation>,
}

#[derive(Debug)]
pub struct StereometerPrimitive(StereometerParams);

impl StereometerPrimitive {
    pub fn new(params: StereometerParams) -> Self {
        Self(params)
    }

    fn vectorscope_bounds(&self) -> Rectangle {
        let margin = if self.0.correlation_meter == CorrelationMeterMode::Off {
            0.0
        } else {
            CORR_W + CORR_PAD
        };
        Rectangle {
            x: self.0.bounds.x + margin,
            y: self.0.bounds.y,
            width: (self.0.bounds.width - margin).max(0.0),
            height: self.0.bounds.height,
        }
    }

    fn build_vertices(&self, viewport: &Viewport) -> Vec<SdfVertex> {
        let clip = ClipTransform::from_viewport(viewport);
        let mut v = Vec::new();

        self.build_correlation_meter(&mut v, clip);
        self.build_vectorscope(&mut v, clip);
        v
    }

    fn build_correlation_meter(&self, v: &mut Vec<SdfVertex>, clip: ClipTransform) {
        let mode = self.0.correlation_meter;
        if mode == CorrelationMeterMode::Off {
            return;
        }

        let (bars, gap) = if mode == CorrelationMeterMode::SingleBand {
            (1, 0.0)
        } else {
            (3, BAND_GAP)
        };
        let bar_w = (CORR_W - gap * (bars - 1) as f32) / bars as f32;
        let bounds = Rectangle {
            x: self.0.bounds.x,
            y: self.0.bounds.y + 16.0,
            width: CORR_W,
            height: (self.0.bounds.height - 32.0).max(0.0),
        };
        let (cy, half_h) = (bounds.y + bounds.height * 0.5, bounds.height * 0.5);
        let val_y = |val: f32| cy - val.clamp(-1.0, 1.0) * half_h;
        let bg = color_to_rgba(CORR_PAL[0]);

        for band in 0..bars {
            let bx = bounds.x + band as f32 * (bar_w + gap);

            // Background + center line
            v.extend(quad_vertices(
                bx,
                bounds.y,
                bx + bar_w,
                bounds.y + bounds.height,
                clip,
                bg,
            ));
            v.extend(quad_vertices(
                bx,
                cy - 0.5,
                bx + bar_w,
                cy + 0.5,
                clip,
                [0.5, 0.5, 0.5, 1.0],
            ));

            // Trail data accessor
            let (trail_len, color_idx) = if mode == CorrelationMeterMode::SingleBand {
                (self.0.corr_trail.len(), 1)
            } else {
                (self.0.band_trail.len(), band + 3)
            };
            let get = |i: usize| {
                if mode == CorrelationMeterMode::SingleBand {
                    self.0.corr_trail.get(i).copied().unwrap_or(0.0)
                } else {
                    self.0
                        .band_trail
                        .get(i)
                        .map(|b| [b.low, b.mid, b.high][band])
                        .unwrap_or(0.0)
                }
            };

            // Render trail as gradient quads
            if trail_len > 1 {
                let (y_min, y_max) = (bounds.y as i32, (bounds.y + bounds.height) as i32);
                let height = (y_max - y_min + 1).max(0) as usize;
                let mut alpha = vec![0.0f32; height];

                // Accumulate max alpha per scanline
                for j in 0..trail_len - 1 {
                    let a = (1.0 - (j + 1) as f32 / trail_len as f32).powf(2.4);
                    if a <= 0.0 {
                        continue;
                    }
                    let (y0, y1) = (val_y(get(j)), val_y(get(j + 1)));
                    let (top, bot) = (y0.min(y1) as i32, (y0.max(y1) + 2.0) as i32);
                    for sy in top.max(y_min)..=bot.min(y_max) {
                        let idx = (sy - y_min) as usize;
                        if idx < height {
                            alpha[idx] = alpha[idx].max(a);
                        }
                    }
                }

                // Emit gradient quads for non-zero runs
                let base = color_to_rgba(
                    CORR_PAL[if color_idx == 1 && get(0) < 0.0 {
                        2
                    } else {
                        color_idx
                    }],
                );
                let mut i = 0;
                while i < height {
                    if alpha[i] <= 0.0 {
                        i += 1;
                        continue;
                    }
                    let start = i;
                    while i < height && alpha[i] > 0.0 {
                        i += 1;
                    }
                    for k in start..i.saturating_sub(1) {
                        let (y0, y1) = ((y_min + k as i32) as f32, (y_min + k as i32 + 1) as f32);
                        let (mut c0, mut c1) = (base, base);
                        c0[3] *= alpha[k];
                        c1[3] *= alpha[k + 1];
                        v.extend(gradient_quad_vertices(
                            bx + 1.0,
                            y0,
                            bx + bar_w - 1.0,
                            y1,
                            clip,
                            c0,
                            c1,
                        ));
                    }
                }
            }

            // Current value indicator
            if trail_len > 0 {
                let val = get(0);
                let y = val_y(val);
                let c = color_to_rgba(
                    CORR_PAL[if color_idx == 1 && val < 0.0 {
                        2
                    } else {
                        color_idx
                    }],
                );
                v.extend(quad_vertices(
                    bx + 1.0,
                    y - 1.0,
                    bx + bar_w - 1.0,
                    y + 1.0,
                    clip,
                    c,
                ));
            }
        }
    }

    fn build_vectorscope(&self, v: &mut Vec<SdfVertex>, clip: ClipTransform) {
        let vs = self.vectorscope_bounds();
        let (cx, cy) = (vs.x + vs.width * 0.5, vs.y + vs.height * 0.5);
        let theta = (self.0.rotation as f32) * std::f32::consts::FRAC_PI_4;
        let (sin_t, cos_t) = theta.sin_cos();
        let radius = ((vs.width.min(vs.height) * 0.5) - 2.0) / (sin_t.abs() + cos_t.abs());
        let [cr, cg, cb, ca] = self.0.trace_color;
        let flip = self.0.flip;

        let xform = |l: f32, r: f32| {
            let (l, r) = if flip { (r, l) } else { (l, r) };
            (
                cx + (l * cos_t + r * sin_t).clamp(-1., 1.) * radius,
                cy + (l * sin_t - r * cos_t).clamp(-1., 1.) * radius,
            )
        };

        let n = self.0.points.len();
        match self.0.mode {
            StereometerMode::DotCloud => {
                let nf = n as f32;
                for (i, &(l, r)) in self.0.points.iter().enumerate() {
                    let (px, py) = xform(l, r);
                    v.extend(dot_vertices(
                        px,
                        py,
                        1.5,
                        0.75,
                        [cr, cg, cb, ca * (i + 1) as f32 / nf],
                        clip,
                    ));
                }
            }
            StereometerMode::Lissajous if n >= 2 => {
                let nm1 = (n - 1) as f32;
                for i in 0..n - 1 {
                    let p0 = xform(self.0.points[i].0, self.0.points[i].1);
                    let p1 = xform(self.0.points[i + 1].0, self.0.points[i + 1].1);
                    let (t0, t1) = (i as f32 / nm1, (i + 1) as f32 / nm1);
                    v.extend(line_vertices(
                        p0,
                        p1,
                        [cr, cg, cb, ca * t0],
                        [cr, cg, cb, ca * t1],
                        1.5,
                        1.0,
                        clip,
                    ));
                }
            }
            _ => {}
        }
    }
}

impl Primitive for StereometerPrimitive {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _: &Rectangle,
        viewport: &Viewport,
    ) {
        pipeline.inner.prepare_instance(
            device,
            queue,
            "Stereometer",
            self.0.key,
            &self.build_vertices(viewport),
        );
    }

    fn render(
        &self,
        pipeline: &Self::Pipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip: &Rectangle<u32>,
    ) {
        let Some(inst) = pipeline.inner.instance(self.0.key) else {
            return;
        };
        if inst.vertex_count == 0 {
            return;
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Stereometer"),
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
        pass.set_pipeline(&pipeline.inner.pipeline);
        pass.set_vertex_buffer(0, inst.vertex_buffer.slice(0..inst.used_bytes()));
        pass.draw(0..inst.vertex_count, 0..1);
    }
}

#[derive(Debug)]
pub struct Pipeline {
    inner: SdfPipeline<u64>,
}

impl primitive::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        Self {
            inner: SdfPipeline::new(
                device,
                format,
                "Stereometer",
                wgpu::PrimitiveTopology::TriangleList,
            ),
        }
    }
}
