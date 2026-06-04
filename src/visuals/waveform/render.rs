// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use iced::Rectangle;
use iced::advanced::graphics::Viewport;
use std::sync::Arc;

use crate::visuals::render::common::sdf_primitive;
use crate::util::color::rgba_with_alpha;
use crate::visuals::render::common::{
    ChannelLayout, ClipTransform, SdfVertex, extend_filled_line, quad_vertices,
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
        self.channels > 0
            && self.preview_progress > 0.0
            && self.preview_samples.len() >= self.channels
    }
}

fn normalize_sample(min: f32, max: f32) -> (f32, f32) {
    let (lo, hi) = (min.min(max), min.max(max));
    (lo.clamp(-1.0, 1.0), hi.clamp(-1.0, 1.0))
}

impl WaveformPrimitive {
    fn build_vertices(&self, viewport: &Viewport) -> Vec<SdfVertex> {
        let params = &self.params;
        let (channels, columns) = (params.channels.max(1), params.columns);
        let total = channels * columns;
        let preview_active = params.preview_active();

        let valid = (columns == 0
            || (params.samples.len() >= total && params.colors.len() >= total))
            && (columns > 0 || preview_active);
        if !valid {
            return Vec::new();
        }

        let clip = ClipTransform::from_viewport(viewport);
        let col_width = params.column_width.max(0.5);
        let preview_width = if preview_active { col_width } else { 0.0 };
        let right_edge = params.bounds.x + params.bounds.width;

        let layout = ChannelLayout::new(
            params.bounds,
            channels,
            params.vertical_padding,
            params.channel_gap,
            params.amplitude_scale,
        );
        let band_expected = channels * NUM_BANDS * columns;

        let mut vertices = Vec::with_capacity(channels * (columns + 1) * 6);

        let scroll_offset = if preview_active {
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
            let center_y = layout.center_y(ch);

            for i in 0..columns {
                let idx = ch * columns + i;
                let x = column_x(i);
                let (min, max) = normalize_sample(params.samples[idx][0], params.samples[idx][1]);
                let color = rgba_with_alpha(params.colors[idx], params.fill_alpha);
                vertices.extend(quad_vertices(
                    x,
                    center_y - max * layout.amplitude_scale,
                    x + col_width,
                    center_y - min * layout.amplitude_scale,
                    clip,
                    color,
                ));
            }

            if preview_active {
                let raw_last_x = right_edge - preview_width - scroll_offset;
                let start_x = raw_last_x.floor();
                let end_x = right_edge;

                let ps = params.preview_samples[ch];
                let (min, max) = normalize_sample(ps.min, ps.max);
                vertices.extend(quad_vertices(
                    start_x,
                    center_y - max * layout.amplitude_scale,
                    end_x,
                    center_y - min * layout.amplitude_scale,
                    clip,
                    rgba_with_alpha(ps.color, params.fill_alpha),
                ));
            }

            if !params.band_levels.is_empty()
                && params.band_levels.len() >= band_expected
                && columns >= 2
            {
                let baseline = center_y + layout.channel_height * 0.5;
                let band_height = layout.channel_height;
                let mut pts = Vec::with_capacity(columns + 1);
                for band in 0..NUM_BANDS {
                    let band_base = (ch * NUM_BANDS + band) * columns;
                    let color = params.band_colors[band];
                    let fill_color = rgba_with_alpha(color, BAND_FILL_ALPHA);

                    pts.clear();
                    pts.extend((0..columns).map(|i| {
                        let level = params.band_levels[band_base + i].clamp(0.0, 1.0);
                        (column_x(i), baseline - level * band_height)
                    }));

                    if let Some(&last) = pts.last() {
                        pts.push((right_edge, last.1));
                    }

                    extend_filled_line(
                        &mut vertices,
                        &pts,
                        baseline,
                        BAND_LINE_WIDTH,
                        color,
                        fill_color,
                        clip,
                    );
                }
            }
        }

        vertices
    }
}

sdf_primitive!(
    WaveformPrimitive(WaveformParams),
    Pipeline,
    u64,
    "Waveform",
    TriangleList,
    |self| self.params.key
);
