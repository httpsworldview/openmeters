use iced::advanced::graphics::Viewport;
use iced::Rectangle;
use std::sync::Arc;

use crate::sdf_primitive;
use crate::ui::render::common::{ClipTransform, SdfVertex};
use crate::ui::render::geometry::{self, append_strip, DEFAULT_FEATHER};

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
    pub samples: Arc<[[f32; 2]]>,
    pub colors: Arc<[[f32; 4]]>,
    pub preview_samples: Arc<[PreviewSample]>,
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
    fn preview_active(&self) -> bool {
        self.preview_progress > 0.0 && self.preview_samples.len() >= self.channels
    }
}

/// Normalize and clamp a min/max pair, ensuring min <= max.
#[inline]
fn normalize_sample(min: f32, max: f32) -> (f32, f32) {
    let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
    (lo.clamp(-1.0, 1.0), hi.clamp(-1.0, 1.0))
}

#[inline]
fn with_alpha(color: [f32; 4], alpha: f32) -> [f32; 4] {
    [color[0], color[1], color[2], alpha]
}

#[derive(Debug)]
pub struct WaveformPrimitive {
    params: WaveformParams,
}

impl WaveformPrimitive {
    pub fn new(params: WaveformParams) -> Self {
        Self { params }
    }

    fn build_vertices(&self, viewport: &Viewport) -> Vec<SdfVertex> {
        let params = &self.params;
        let (channels, columns) = (params.channels.max(1), params.columns);
        let total = channels * columns;

        // Validate data
        let valid = (columns == 0
            || (params.samples.len() >= total && params.colors.len() >= total))
            && (columns > 0 || params.preview_active());
        if !valid {
            return Vec::new();
        }

        let clip = ClipTransform::from_viewport(viewport);
        let col_width = params.column_width.max(0.5);
        let preview_width = if params.preview_active() {
            col_width
        } else {
            0.0
        };
        let right_edge = params.bounds.x + params.bounds.width;

        // Channel layout calculations
        let v_pad = params.vertical_padding.max(0.0);
        let gap = params.channel_gap.max(0.0);
        let usable_h =
            (params.bounds.height - v_pad * 2.0 - gap * (channels.saturating_sub(1) as f32))
                .max(1.0);
        let ch_height = usable_h / channels as f32;
        let amp_scale = ch_height * 0.5 * params.amplitude_scale.max(0.01);
        let stroke = params.stroke_width.max(0.5);

        let mut vertices = Vec::with_capacity(channels * (columns + 1) * 6);

        for ch in 0..channels {
            let center_y =
                params.bounds.y + v_pad + ch as f32 * (ch_height + gap) + ch_height * 0.5;

            // Build area fill vertices
            let mut area = Vec::with_capacity((columns + 1) * 2);
            for i in 0..columns {
                let idx = ch * columns + i;
                let (min, max) = normalize_sample(params.samples[idx][0], params.samples[idx][1]);
                let x = (right_edge - preview_width - col_width * (columns - 1 - i) as f32).round();
                let color = with_alpha(
                    params.colors.get(idx).copied().unwrap_or([1.0; 4]),
                    params.fill_alpha,
                );

                area.push(SdfVertex::solid(
                    clip.to_clip(x, center_y - max * amp_scale),
                    color,
                ));
                area.push(SdfVertex::solid(
                    clip.to_clip(x, center_y - min * amp_scale),
                    color,
                ));
            }

            if params.preview_active() {
                let ps = params.preview_samples[ch];
                let (min, max) = normalize_sample(ps.min, ps.max);
                let x = right_edge.round();
                let color = with_alpha(ps.color, params.fill_alpha);
                area.push(SdfVertex::solid(
                    clip.to_clip(x, center_y - max * amp_scale),
                    color,
                ));
                area.push(SdfVertex::solid(
                    clip.to_clip(x, center_y - min * amp_scale),
                    color,
                ));
            }
            append_strip(&mut vertices, area);

            // Build center line vertices
            let mut positions = Vec::with_capacity(columns + 1);
            let mut line_colors = Vec::with_capacity(columns + 1);

            for i in 0..columns {
                let idx = ch * columns + i;
                let (min, max) = normalize_sample(params.samples[idx][0], params.samples[idx][1]);
                let x = (right_edge - preview_width - col_width * (columns - 1 - i) as f32).round();
                let y = center_y - 0.5 * (min + max) * amp_scale;
                positions.push((x, y));
                line_colors.push(with_alpha(
                    params.colors.get(idx).copied().unwrap_or([1.0; 4]),
                    params.line_alpha,
                ));
            }

            if params.preview_active() {
                let ps = params.preview_samples[ch];
                let (min, max) = normalize_sample(ps.min, ps.max);
                positions.push((right_edge.round(), center_y - 0.5 * (min + max) * amp_scale));
                line_colors.push(with_alpha(ps.color, params.line_alpha));
            }

            if positions.len() >= 2 {
                let line = geometry::build_aa_line_strip_colored(
                    &positions,
                    &line_colors,
                    stroke,
                    DEFAULT_FEATHER,
                    &clip,
                );
                append_strip(&mut vertices, line);
            }
        }

        vertices
    }
}

sdf_primitive!(
    WaveformPrimitive,
    Pipeline,
    u64,
    "Waveform",
    TriangleStrip,
    |self| self.params.instance_key
);
