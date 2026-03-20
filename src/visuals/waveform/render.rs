// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use iced::Rectangle;
use iced::advanced::graphics::Viewport;
use std::sync::Arc;

use crate::sdf_primitive;
use crate::visuals::render::common::{
    ClipTransform, SdfVertex, baseline_segment_vertices, build_aa_line_list, quad_vertices,
};
use crate::visuals::waveform::processor::NUM_BANDS;

#[derive(Debug, Clone, Copy)]
pub struct PreviewSample {
    pub min: f32,
    pub max: f32,
    pub color: [f32; 4],
}

const BAND_LINE_WIDTH: f32 = 1.5;
const BAND_FILL_ALPHA: f32 = 0.15;

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
    /// Band levels for peak history overlay. Layout: `(channel * NUM_BANDS + band) * columns + col`.
    /// Empty if peak history is disabled.
    pub band_levels: Arc<[f32]>,
    pub band_colors: [[f32; 4]; NUM_BANDS],
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

        let column_x = |i: usize| -> f32 {
            let dist_steps = (columns - 1 - i) as f32;
            (right_edge - preview_width - dist_steps * col_width - scroll_offset - col_width)
                .floor()
        };

        for ch in 0..channels {
            let center_y =
                params.bounds.y + v_pad + ch as f32 * (ch_height + gap) + ch_height * 0.5;

            for i in 0..columns {
                let idx = ch * columns + i;
                let (min, max) = normalize_sample(params.samples[idx][0], params.samples[idx][1]);
                let x = column_x(i);

                let color = with_alpha(
                    params.colors.get(idx).copied().unwrap_or([1.0; 4]),
                    params.fill_alpha,
                );
                vertices.extend(quad_vertices(
                    x,
                    center_y - max * amp_scale,
                    x + col_width,
                    center_y - min * amp_scale,
                    clip,
                    color,
                ));
            }

            if params.preview_active() {
                let raw_last_x = right_edge - preview_width - scroll_offset;
                let start_x = raw_last_x.floor();
                let end_x = right_edge;

                let ps = params.preview_samples[ch];
                let (min, max) = normalize_sample(ps.min, ps.max);
                let color = with_alpha(ps.color, params.fill_alpha);
                vertices.extend(quad_vertices(
                    start_x,
                    center_y - max * amp_scale,
                    end_x,
                    center_y - min * amp_scale,
                    clip,
                    color,
                ));
            }

            // Peak history overlay -- confined to this channel's vertical region.
            let band_expected = params.channels * NUM_BANDS * columns;
            if !params.band_levels.is_empty()
                && params.band_levels.len() >= band_expected
                && columns >= 2
            {
                let baseline = center_y + ch_height * 0.5;
                let band_height = ch_height;
                let mut pts = Vec::with_capacity(columns + 1);
                for band in 0..NUM_BANDS {
                    let band_base = (ch * NUM_BANDS + band) * columns;
                    let color = params.band_colors[band];
                    let fill_color = with_alpha(color, BAND_FILL_ALPHA);

                    pts.clear();
                    pts.extend((0..columns).map(|i| {
                        let level = params.band_levels[band_base + i].clamp(0.0, 1.0);
                        (column_x(i), baseline - level * band_height)
                    }));

                    // Extend to the right edge so the overlay covers the preview region.
                    if let Some(&last) = pts.last() {
                        pts.push((right_edge, last.1));
                    }

                    for pair in pts.windows(2) {
                        vertices.extend(baseline_segment_vertices(
                            pair[0],
                            pair[1],
                            baseline,
                            clip,
                            [fill_color, fill_color],
                        ));
                    }

                    vertices.extend(build_aa_line_list(&pts, BAND_LINE_WIDTH, color, &clip));
                }
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
    TriangleList,
    |self| self.params.key
);
