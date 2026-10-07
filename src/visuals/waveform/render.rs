// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use iced::Rectangle;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::util::{
    audio::{DB_FLOOR, power_to_db, sanitize_negative_db},
    color::{rgba_with_alpha, sample_rgba_gradient},
};
use crate::visuals::options::{WaveformColorMode, WaveformHistoryMode};
use crate::visuals::render::common::{
    ClipTransform, GeometryScratch, extend_filled_line, quad_instance,
};
use crate::visuals::render::common::{SdfPipeline, sdf_primitive};
use crate::visuals::waveform::processor::{
    DEFAULT_BAND_DB_FLOOR, MAX_COLUMN_CAPACITY, NUM_BANDS, WAVEFORM_SILENCE_AMPLITUDE, WaveColumn,
    WaveFrame, WaveformPreview,
};

pub(super) const COLUMN_WIDTH_PIXELS: f32 = 1.0;

const BAND_LINE_WIDTH: f32 = 1.5;
const BAND_FILL_ALPHA: f32 = 0.15;
const MIN_COLUMN_HEIGHT_PIXELS: f32 = 1.0;
const LOUDNESS_QUIET_DB: f32 = -36.0;

#[derive(Debug)]
pub struct WaveformParams {
    pub bounds: Rectangle,
    pub lane: usize,
    pub data: Arc<Mutex<VecDeque<WaveFrame>>>,
    pub preview: WaveformPreview,
    pub color_mode: WaveformColorMode,
    pub history_mode: WaveformHistoryMode,
    pub band_db_floor: f32,
    pub palette: [[f32; 4]; NUM_BANDS],
    pub key: u64,
}

impl WaveformParams {
    fn column_color(&self, column: WaveColumn) -> [f32; 4] {
        match self.color_mode {
            WaveformColorMode::Frequency => self.band_mix_color(column.color_bands),
            WaveformColorMode::Loudness => {
                let peak = column.min.abs().max(column.max.abs());
                let db = power_to_db(peak * peak, DB_FLOOR);
                sample_rgba_gradient(
                    &self.palette,
                    if db.is_finite() {
                        (db - LOUDNESS_QUIET_DB) / -LOUDNESS_QUIET_DB
                    } else {
                        0.0
                    },
                )
            }
            WaveformColorMode::Static => self.palette[0],
        }
    }

    fn band_mix_color(&self, bands: [f32; NUM_BANDS]) -> [f32; 4] {
        let mut out = [0.0; 4];
        let mut total = 0.0;
        for (&band, color) in bands.iter().zip(&self.palette) {
            let weight = crate::util::finite_positive(band).unwrap_or(0.0);
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

    // Cover both extrema, including fractional pixels at a lane's clip edge.
    let half_pixel = MIN_COLUMN_HEIGHT_PIXELS * 0.5;
    Some((
        center_y - max * amplitude_scale - half_pixel,
        center_y - min * amplitude_scale + half_pixel,
    ))
}

impl WaveformParams {
    fn build_vertices(&self, scratch: &mut GeometryScratch) {
        let params = self;
        let data = crate::util::unpoison(params.data.lock());
        let columns = ((params.bounds.width / COLUMN_WIDTH_PIXELS).ceil() as usize)
            .clamp(1, MAX_COLUMN_CAPACITY)
            .min(data.len());
        let start = data.len().saturating_sub(columns);
        let preview_columns = params
            .preview
            .columns
            .filter(|_| params.preview.progress > 0.0);

        let clip = ClipTransform::from_bounds(params.bounds);
        let col_width = COLUMN_WIDTH_PIXELS;
        let right_edge = params.bounds.x + params.bounds.width;

        let center_y = params.bounds.center_y();
        let amplitude_scale = params.bounds.height * 0.5;
        let history = match params.history_mode {
            WaveformHistoryMode::Off => None,
            WaveformHistoryMode::RmsFast => Some(0),
            WaveformHistoryMode::RmsSlow => Some(1),
        }
        .filter(|_| columns >= 2);
        let floor = sanitize_negative_db(params.band_db_floor, DEFAULT_BAND_DB_FLOOR);

        let vertices = &mut scratch.instances;
        vertices.reserve(columns + 1 + usize::from(history.is_some()) * NUM_BANDS * columns * 2);

        let scroll_offset = if preview_columns.is_some() {
            params.preview.progress * col_width
        } else {
            0.0
        };

        let column_x = |i: usize| -> f32 {
            let dist_steps = (columns - 1 - i) as f32;
            right_edge - dist_steps * col_width - scroll_offset - col_width
        };
        let push_column = |vertices: &mut Vec<_>, x0, x1, column: WaveColumn| {
            if let Some((y0, y1)) = sample_y_span(center_y, amplitude_scale, column.min, column.max)
            {
                let color = params.column_color(column);
                vertices.push(quad_instance(x0, y0, x1, y1, clip, color));
            }
        };

        for (i, frame) in data.range(start..start + columns).enumerate() {
            let column = frame[params.lane];
            let x = column_x(i);
            push_column(vertices, x, x + col_width, column);
        }

        if let Some(preview_columns) = preview_columns {
            let start_x = right_edge - scroll_offset;
            push_column(vertices, start_x, right_edge, preview_columns[params.lane]);
        }

        if let Some(history) = history {
            // Put the floor, including its antialias fringe, below the lane's clip edge.
            let band_height = params.bounds.height + BAND_LINE_WIDTH * 0.5 + 1.0;
            let baseline = params.bounds.y + band_height;
            let pts = &mut scratch.points;
            for (band, &color) in params.palette.iter().enumerate() {
                let fill_color = rgba_with_alpha(color, color[3] * BAND_FILL_ALPHA);

                pts.clear();
                pts.reserve(columns + 1);
                pts.extend(
                    data.range(start..start + columns)
                        .enumerate()
                        .map(|(i, frame)| {
                            let column = frame[params.lane];
                            let db = column.rms_db[history][band];
                            let level = ((db - floor) / -floor).clamp(0.0, 1.0);
                            (column_x(i), baseline - level * band_height)
                        }),
                );
                let last_y = pts.last().expect("nonempty waveform history").1;
                pts.push((right_edge, last_y));
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

sdf_primitive!(
    WaveformParams,
    SdfPipeline,
    "Waveform",
    |self| self.key,
    None
);
