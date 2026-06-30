// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use iced::Rectangle;
use iced::advanced::graphics::Viewport;
use std::{collections::VecDeque, sync::Arc};

use crate::util::{
    audio::{DB_FLOOR, power_to_db, sanitize_negative_db},
    color::{rgba_with_alpha, sample_rgba_gradient},
};
use crate::visuals::options::{WaveformColorMode, WaveformHistoryMode};
use crate::visuals::render::common::sdf_primitive;
use crate::visuals::render::common::{
    ChannelLayout, ClipTransform, GeometryScratch, extend_filled_line, quad_vertices,
};
use crate::visuals::waveform::processor::{
    DEFAULT_BAND_DB_FLOOR, NUM_BANDS, WAVEFORM_SILENCE_AMPLITUDE, WaveColumn, WaveFrame,
    WaveformPreview,
};

const BAND_LINE_WIDTH: f32 = 1.5;
const BAND_FILL_ALPHA: f32 = 0.15;
const MIN_COLUMN_HEIGHT_PIXELS: f32 = 1.0;
const LOUDNESS_QUIET_DB: f32 = -36.0;

#[derive(Debug)]
pub struct WaveformParams {
    pub bounds: Rectangle,
    pub lanes: [usize; 2],
    pub channels: usize,
    pub column_width: f32,
    pub columns: usize,
    pub data: Arc<VecDeque<WaveFrame>>,
    pub preview: WaveformPreview,
    pub color_mode: WaveformColorMode,
    pub history_mode: WaveformHistoryMode,
    pub band_db_floor: f32,
    pub palette: [[f32; 4]; NUM_BANDS],
    pub fill_alpha: f32,
    pub vertical_padding: f32,
    pub channel_gap: f32,
    pub amplitude_scale: f32,
    pub key: u64,
}

impl WaveformParams {
    fn preview_active(&self) -> bool {
        self.channels > 0 && self.preview.progress > 0.0 && self.preview.columns.is_some()
    }

    fn column_color(&self, column: WaveColumn) -> [f32; 4] {
        match self.color_mode {
            WaveformColorMode::Frequency => self.band_mix_color(column.color_bands),
            WaveformColorMode::Loudness => {
                let peak = column.min.abs().max(column.max.abs());
                let db = power_to_db(peak * peak, DB_FLOOR);
                sample_rgba_gradient(&self.palette, if db.is_finite() {
                    (db - LOUDNESS_QUIET_DB) / -LOUDNESS_QUIET_DB
                } else {
                    0.0
                })
            }
            WaveformColorMode::Static => self.palette[0],
        }
    }

    fn band_mix_color(&self, bands: [f32; NUM_BANDS]) -> [f32; 4] {
        let mut out = [0.0; 4];
        let mut total = 0.0;
        for (weight, color) in bands
            .map(|v| crate::util::finite_positive(v).unwrap_or(0.0))
            .into_iter()
            .zip(self.palette.iter())
        {
            total += weight;
            for i in 0..4 {
                out[i] += color[i] * weight;
            }
        }
        let brightness = out[0].max(out[1]).max(out[2]);
        if total <= f32::EPSILON || brightness <= WAVEFORM_SILENCE_AMPLITUDE {
            return [0.0; 4];
        }
        let inv_brightness = brightness.recip();
        [
            (out[0] * inv_brightness).clamp(0.0, 1.0),
            (out[1] * inv_brightness).clamp(0.0, 1.0),
            (out[2] * inv_brightness).clamp(0.0, 1.0),
            (out[3] / total).clamp(0.0, 1.0),
        ]
    }
}

fn sample_y_span(center_y: f32, amplitude_scale: f32, min: f32, max: f32) -> Option<(f32, f32)> {
    let (lo, hi) = (min.min(max), min.max(max));
    let (min, max) = (lo.clamp(-1.0, 1.0), hi.clamp(-1.0, 1.0));
    if min.abs().max(max.abs()) < WAVEFORM_SILENCE_AMPLITUDE {
        return None;
    }

    let (mut y0, mut y1) = (
        center_y - max * amplitude_scale,
        center_y - min * amplitude_scale,
    );
    if (y1 - y0).abs() < MIN_COLUMN_HEIGHT_PIXELS {
        let mid = (y0 + y1) * 0.5;
        y0 = mid - MIN_COLUMN_HEIGHT_PIXELS * 0.5;
        y1 = mid + MIN_COLUMN_HEIGHT_PIXELS * 0.5;
    }
    Some((y0.min(y1), y0.max(y1)))
}

fn with_fill_alpha(color: [f32; 4], alpha: f32) -> [f32; 4] {
    rgba_with_alpha(color, color[3] * alpha)
}

impl WaveformPrimitive {
    fn build_vertices(&self, viewport: &Viewport, scratch: &mut GeometryScratch) {
        let params = &self.params;
        let data = &params.data;
        let (channels, columns) = (params.channels.max(1), params.columns.min(data.len()));
        let start = data.len().saturating_sub(columns);
        let preview_active = params.preview_active();

        if columns == 0 && !preview_active {
            return;
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
        let history: Option<fn(WaveColumn) -> [f32; NUM_BANDS]> = match params.history_mode {
            WaveformHistoryMode::Off => None,
            WaveformHistoryMode::RmsFast => Some(|column| column.rms_fast_db),
            WaveformHistoryMode::RmsSlow => Some(|column| column.rms_slow_db),
        };
        let history_active = history.is_some() && columns >= 2;
        let floor = sanitize_negative_db(params.band_db_floor, DEFAULT_BAND_DB_FLOOR);

        let vertices = &mut scratch.vertices;
        vertices.reserve(
            channels * (columns + 1) * 6
                + usize::from(history_active) * channels * NUM_BANDS * columns * 12,
        );

        let static_color = (params.color_mode == WaveformColorMode::Static)
            .then(|| with_fill_alpha(params.palette[0], params.fill_alpha));

        let preview_columns = preview_active.then_some(params.preview.columns).flatten();
        let scroll_offset = if preview_columns.is_some() {
            params.preview.progress * col_width
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
                let column = data[start + i][params.lanes[ch]];
                let x = column_x(i);
                if let Some((y0, y1)) = sample_y_span(
                    center_y,
                    layout.amplitude_scale,
                    column.min,
                    column.max,
                ) {
                    vertices.extend(quad_vertices(
                        x,
                        y0,
                        x + col_width,
                        y1,
                        clip,
                        static_color
                            .unwrap_or_else(|| with_fill_alpha(params.column_color(column), params.fill_alpha)),
                    ));
                }
            }

            if let Some(preview_columns) = preview_columns {
                let raw_last_x = right_edge - preview_width - scroll_offset;
                let start_x = raw_last_x.floor();
                let end_x = right_edge;

                let ps = preview_columns[params.lanes[ch]];
                if let Some((y0, y1)) =
                    sample_y_span(center_y, layout.amplitude_scale, ps.min, ps.max)
                {
                    vertices.extend(quad_vertices(
                        start_x,
                        y0,
                        end_x,
                        y1,
                        clip,
                        static_color
                            .unwrap_or_else(|| with_fill_alpha(params.column_color(ps), params.fill_alpha)),
                    ));
                }
            }

            if let Some(history) = history.filter(|_| history_active) {
                let baseline = center_y + layout.channel_height * 0.5;
                let band_height = layout.channel_height;
                let pts = &mut scratch.points;
                for (band, &color) in params.palette.iter().enumerate() {
                    let fill_color = with_fill_alpha(color, BAND_FILL_ALPHA);

                    pts.clear();
                    pts.reserve(columns + 1);
                    pts.extend((0..columns).map(|i| {
                        let column = data[start + i][params.lanes[ch]];
                        let db = history(column)[band].max(floor);
                        let level = ((db - floor) / -floor).clamp(0.0, 1.0);
                        (column_x(i), baseline - level * band_height)
                    }));
                    if let Some(&last) = pts.last() {
                        pts.push((right_edge, last.1));
                    }
                    extend_filled_line(
                        vertices,
                        pts,
                        baseline,
                        BAND_LINE_WIDTH,
                        color,
                        fill_color,
                        clip,
                    );
                }
            }
        }
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
