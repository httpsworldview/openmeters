use iced::Rectangle;
use iced::advanced::graphics::Viewport;
use std::sync::Arc;

use crate::sdf_primitive;
use crate::ui::render::common::{ClipTransform, SdfVertex, append_strip};

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
    pub vertical_padding: f32,
    pub channel_gap: f32,
    pub amplitude_scale: f32,
    pub key: u64,
}

impl WaveformParams {
    fn preview_active(&self) -> bool {
        self.preview_progress > 0.0 && self.preview_samples.len() >= self.channels
    }
}

// Normalize and clamp a min/max pair, ensuring min <= max.
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

        let mut vertices = Vec::with_capacity(channels * (columns + 1) * 6);

        let scroll_offset = if params.preview_active() {
            params.preview_progress * col_width
        } else {
            0.0
        };

        for ch in 0..channels {
            let center_y =
                params.bounds.y + v_pad + ch as f32 * (ch_height + gap) + ch_height * 0.5;

            // Build independent pixel columns (discrete quads)
            let mut strip_builder = Vec::with_capacity((columns + 2) * 6);

            for i in 0..columns {
                let idx = ch * columns + i;
                let (min, max) = normalize_sample(params.samples[idx][0], params.samples[idx][1]);

                // Calculate float position for smooth scroll, then floor to snap to pixel grid
                // i=0 is oldest column (leftmost). i=columns-1 is newest history.
                // Newest history moves left from `right_edge - preview_width`
                let dist_steps = (columns - 1 - i) as f32;
                // Subtract col_width because raw_x represents the LEFT edge of the 1px column
                let raw_x =
                    right_edge - preview_width - dist_steps * col_width - scroll_offset - col_width;
                let x = raw_x.floor();
                let w = col_width;

                let color = with_alpha(
                    params.colors.get(idx).copied().unwrap_or([1.0; 4]),
                    params.fill_alpha,
                );

                let quad = vec![
                    SdfVertex::solid(clip.to_clip(x, center_y - max * amp_scale), color),
                    SdfVertex::solid(clip.to_clip(x, center_y - min * amp_scale), color),
                    SdfVertex::solid(clip.to_clip(x + w, center_y - max * amp_scale), color),
                    SdfVertex::solid(clip.to_clip(x + w, center_y - min * amp_scale), color),
                ];
                append_strip(&mut strip_builder, quad);
            }

            if params.preview_active() {
                // Preview connects to the right of the newest history column
                let raw_last_x = right_edge - preview_width - scroll_offset;
                let last_x = raw_last_x.floor();

                // Start where the last history column ends (visually)
                let start_x = last_x;
                // Stretch to component edge to ensure no background leaks through gap
                let end_x = right_edge;

                let ps = params.preview_samples[ch];
                let (min, max) = normalize_sample(ps.min, ps.max);
                let color = with_alpha(ps.color, params.fill_alpha);

                let quad = vec![
                    SdfVertex::solid(clip.to_clip(start_x, center_y - max * amp_scale), color),
                    SdfVertex::solid(clip.to_clip(start_x, center_y - min * amp_scale), color),
                    SdfVertex::solid(clip.to_clip(end_x, center_y - max * amp_scale), color),
                    SdfVertex::solid(clip.to_clip(end_x, center_y - min * amp_scale), color),
                ];
                append_strip(&mut strip_builder, quad);
            }
            append_strip(&mut vertices, strip_builder);
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
    |self| self.params.key
);
