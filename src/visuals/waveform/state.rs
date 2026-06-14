// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{
    DEFAULT_BAND_DB_FLOOR, MAX_COLUMN_CAPACITY, NUM_BANDS, WAVEFORM_CHANNELS,
    WAVEFORM_SILENCE_AMPLITUDE, WaveColumn, WaveformSnapshot,
};
use super::render::{PreviewSample, WaveformParams, WaveformPrimitive};
use crate::persistence::settings::WaveformSettings;
use crate::util::{
    audio::{Channel, power_to_db, sanitize_negative_db},
    color::{color_to_rgba, sample_gradient},
};
use crate::visuals::options::{WaveformColorMode, WaveformHistoryMode};
use crate::visuals::palettes;
use iced::Color;
use std::sync::Arc;

const COLUMN_WIDTH_PIXELS: f32 = 1.0;
const LOUDNESS_QUIET_DB: f32 = -36.0;
type SampleBuffers = (Arc<[[f32; 2]]>, Arc<[[f32; 4]]>);

#[derive(Debug, Clone)]
pub(crate) struct WaveformState {
    raw_snapshot: WaveformSnapshot,
    pub(crate) style: WaveformStyle,
    key: u64,
    pub(crate) channel_1: Channel,
    pub(crate) channel_2: Channel,
    pub(crate) color_mode: WaveformColorMode,
    pub(crate) history_mode: WaveformHistoryMode,
    pub(crate) band_db_floor: f32,
}

impl WaveformState {
    pub fn new() -> Self {
        let defaults = WaveformSettings::default();
        Self {
            raw_snapshot: WaveformSnapshot::default(),
            style: WaveformStyle::default(),
            key: crate::visuals::next_key(),
            channel_1: defaults.channel_1,
            channel_2: defaults.channel_2,
            color_mode: defaults.color_mode,
            history_mode: defaults.history_mode,
            band_db_floor: defaults.band_db_floor,
        }
    }

    pub fn apply_snapshot(&mut self, snapshot: WaveformSnapshot) {
        self.raw_snapshot = snapshot;
    }

    pub fn set_channels(&mut self, channel_1: Channel, channel_2: Channel) {
        (self.channel_1, self.channel_2) = (channel_1, channel_2);
    }

    pub fn set_palette(&mut self, palette: &[Color; NUM_BANDS]) {
        self.style.palette = *palette;
    }

    pub fn visual_params(&self, bounds: iced::Rectangle) -> Option<WaveformParams> {
        let snapshot = &self.raw_snapshot;
        let total_columns = snapshot.columns;
        let (lanes, selected_channels) = self.selected_lanes(snapshot.channels);
        let expected = total_columns * snapshot.channels;
        if bounds.width <= 0.0
            || total_columns == 0
            || selected_channels == 0
            || snapshot.data.len() < expected
        {
            return None;
        }

        let needed =
            ((bounds.width / COLUMN_WIDTH_PIXELS).ceil() as usize).clamp(1, MAX_COLUMN_CAPACITY);
        let visible = needed.min(total_columns);
        let start = total_columns.saturating_sub(needed);
        let lanes = &lanes[..selected_channels];
        let (samples, colors) = self.build_sample_data(lanes, start, visible);
        let (preview_samples, preview_progress) = self.build_preview(lanes);
        let band_levels = self.build_history_levels(lanes, start, visible);

        Some(WaveformParams {
            bounds,
            channels: selected_channels,
            column_width: COLUMN_WIDTH_PIXELS,
            columns: visible,
            samples,
            colors,
            preview_samples,
            preview_progress,
            band_levels,
            band_colors: self.style.band_colors(),
            fill_alpha: self.style.fill_alpha,
            vertical_padding: self.style.vertical_padding,
            channel_gap: self.style.channel_gap,
            amplitude_scale: self.style.amplitude_scale,
            key: self.key,
        })
    }

    fn selected_lanes(&self, available_lanes: usize) -> ([usize; 2], usize) {
        let mut lanes = [0; 2];
        let mut len = 0;
        for lane in [self.channel_1, self.channel_2]
            .into_iter()
            .filter_map(|channel| WAVEFORM_CHANNELS.iter().position(|&source| source == channel))
            .filter(|&lane| lane < available_lanes)
        {
            lanes[len] = lane;
            len += 1;
        }
        (lanes, len)
    }

    fn column(&self, lane: usize, col: usize) -> WaveColumn {
        self.raw_snapshot.data[lane * self.raw_snapshot.columns + col]
    }

    fn build_sample_data(&self, lanes: &[usize], start: usize, visible: usize) -> SampleBuffers {
        let mut samples = Vec::with_capacity(visible * lanes.len());
        let mut colors = Vec::with_capacity(visible * lanes.len());

        for &lane in lanes {
            for col in start..(start + visible) {
                let column = self.column(lane, col);
                samples.push([column.min, column.max]);
                colors.push(color_to_rgba(self.column_color(column)));
            }
        }
        (Arc::from(samples), Arc::from(colors))
    }

    fn column_color(&self, column: WaveColumn) -> Color {
        match self.color_mode {
            WaveformColorMode::Frequency => self.style.band_mix_color(column.color_bands),
            WaveformColorMode::Loudness => self.style.sample_color(if column.peak_db.is_finite() {
                ((column.peak_db - LOUDNESS_QUIET_DB) / -LOUDNESS_QUIET_DB).clamp(0.0, 1.0)
            } else {
                0.0
            }),
            WaveformColorMode::Static => self.style.sample_color(0.0),
        }
    }

    fn build_preview(&self, lanes: &[usize]) -> (Arc<[PreviewSample]>, f32) {
        let preview = &self.raw_snapshot.preview;
        if preview.progress <= 0.0 || preview.columns.len() < self.raw_snapshot.channels {
            return (Arc::from([]), 0.0);
        }

        let result: Vec<_> = lanes
            .iter()
            .map(|&lane| {
                let column = preview.columns[lane];
                PreviewSample {
                    min: column.min,
                    max: column.max,
                    color: color_to_rgba(self.column_color(column)),
                }
            })
            .collect();

        (Arc::from(result), preview.progress.clamp(0.0, 1.0))
    }

    fn build_history_levels(&self, lanes: &[usize], start: usize, visible: usize) -> Arc<[f32]> {
        let history: fn(WaveColumn) -> [f32; NUM_BANDS] = match self.history_mode {
            WaveformHistoryMode::Off => return Arc::from([]),
            WaveformHistoryMode::RmsFast => |column| column.rms_fast,
            WaveformHistoryMode::RmsSlow => |column| column.rms_slow,
        };
        let floor = sanitize_negative_db(self.band_db_floor, DEFAULT_BAND_DB_FLOOR);
        let mut out = Vec::with_capacity(lanes.len() * NUM_BANDS * visible);
        for &lane in lanes {
            for band in 0..NUM_BANDS {
                for col in start..(start + visible) {
                    let db = power_to_db(history(self.column(lane, col))[band], floor);
                    out.push(((db - floor) / -floor).clamp(0.0, 1.0));
                }
            }
        }
        Arc::from(out)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WaveformStyle {
    pub fill_alpha: f32,
    pub vertical_padding: f32,
    pub channel_gap: f32,
    pub amplitude_scale: f32,
    pub(crate) palette: [Color; NUM_BANDS],
}

impl Default for WaveformStyle {
    fn default() -> Self {
        Self {
            fill_alpha: 1.0,
            vertical_padding: 8.0,
            channel_gap: 12.0,
            amplitude_scale: 1.0,
            palette: palettes::waveform::COLORS,
        }
    }
}

impl WaveformStyle {
    fn sample_color(&self, intensity: f32) -> Color {
        sample_gradient(&self.palette, intensity)
    }

    fn band_mix_color(&self, bands: [f32; NUM_BANDS]) -> Color {
        let mut rgb = [0.0; 3];
        let mut alpha = 0.0;
        let mut total = 0.0;

        for (weight, color) in bands
            .map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 })
            .into_iter()
            .zip(self.palette.iter())
        {
            total += weight;
            rgb[0] += color.r * weight;
            rgb[1] += color.g * weight;
            rgb[2] += color.b * weight;
            alpha += color.a * weight;
        }

        let brightness = rgb[0].max(rgb[1]).max(rgb[2]);
        if total <= f32::EPSILON || brightness <= WAVEFORM_SILENCE_AMPLITUDE {
            return Color::TRANSPARENT;
        }

        let inv_brightness = brightness.recip();
        Color::from_rgba(
            (rgb[0] * inv_brightness).clamp(0.0, 1.0),
            (rgb[1] * inv_brightness).clamp(0.0, 1.0),
            (rgb[2] * inv_brightness).clamp(0.0, 1.0),
            (alpha / total).clamp(0.0, 1.0),
        )
    }

    fn band_colors(&self) -> [[f32; 4]; NUM_BANDS] {
        std::array::from_fn(|i| color_to_rgba(self.palette[i]))
    }
}

crate::visuals::visualization_widget!(Waveform, WaveformState, WaveformPrimitive);
