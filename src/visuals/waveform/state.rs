// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{
    DEFAULT_BAND_DB_FLOOR, MAX_COLUMN_CAPACITY, NUM_BANDS, WAVEFORM_CHANNELS,
    WAVEFORM_SILENCE_AMPLITUDE, WaveColumn, WaveFrame, WaveformPreview, WaveformUpdate,
};
use super::render::{PreviewSample, WaveformParams, WaveformPrimitive};
use crate::persistence::settings::WaveformSettings;
use crate::util::{
    audio::{Channel, DB_FLOOR, power_to_db, sanitize_negative_db},
    color::{color_to_rgba, sample_gradient},
};
use crate::visuals::options::{WaveformColorMode, WaveformHistoryMode};
use crate::visuals::palettes;
use iced::Color;
use std::{cell::Cell, collections::VecDeque};

const COLUMN_WIDTH_PIXELS: f32 = 1.0;
const INITIAL_VIEW_COLUMNS: usize = 512;
const LOUDNESS_QUIET_DB: f32 = -36.0;
type SampleBuffers = (Vec<[f32; 2]>, Vec<[f32; 4]>);

#[derive(Debug, Clone)]
pub(crate) struct WaveformState {
    data: VecDeque<WaveFrame>,
    preview: WaveformPreview,
    view_columns: Cell<usize>,
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
            data: VecDeque::new(),
            preview: WaveformPreview::default(),
            view_columns: Cell::new(INITIAL_VIEW_COLUMNS),
            style: WaveformStyle::default(),
            key: crate::visuals::next_key(),
            channel_1: defaults.channel_1,
            channel_2: defaults.channel_2,
            color_mode: defaults.color_mode,
            history_mode: defaults.history_mode,
            band_db_floor: defaults.band_db_floor,
        }
    }

    pub fn apply_snapshot(&mut self, update: WaveformUpdate) {
        let max_columns = self.view_columns.get().clamp(1, MAX_COLUMN_CAPACITY);
        self.configure_ring(max_columns, update.reset);
        self.preview = update.preview;
        for columns in update.columns {
            self.push_column(columns, max_columns);
        }
    }

    pub(crate) fn view_columns(&self) -> usize {
        self.view_columns.get()
    }

    pub fn set_channels(&mut self, channel_1: Channel, channel_2: Channel) {
        (self.channel_1, self.channel_2) = (channel_1, channel_2);
    }

    pub fn set_palette(&mut self, palette: &[Color; NUM_BANDS]) {
        self.style.palette = *palette;
    }

    pub fn visual_params(&self, bounds: iced::Rectangle) -> Option<WaveformParams> {
        let needed = ((bounds.width / COLUMN_WIDTH_PIXELS).ceil() as usize)
            .clamp(1, MAX_COLUMN_CAPACITY);
        if bounds.width > 0.0 {
            self.view_columns.set(needed);
        }

        let total_columns = self.data.len();
        let (lanes, selected_channels) = self.selected_lanes();
        if bounds.width <= 0.0 || total_columns == 0 || selected_channels == 0 {
            return None;
        }

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

    fn configure_ring(&mut self, max_columns: usize, reset: bool) {
        if reset {
            self.data.clear();
        }
        let drop = self.data.len().saturating_sub(max_columns);
        self.data.drain(..drop);
        if self.data.capacity() < max_columns {
            self.data.reserve(max_columns - self.data.capacity());
        } else if self.data.capacity() > max_columns.saturating_mul(2) {
            self.data.shrink_to(max_columns);
        }
    }

    fn push_column(&mut self, columns: WaveFrame, max_columns: usize) {
        if self.data.len() == max_columns {
            self.data.pop_front();
        }
        self.data.push_back(columns);
    }

    fn selected_lanes(&self) -> ([usize; 2], usize) {
        let mut lanes = [0; 2];
        let mut len = 0;
        for lane in [self.channel_1, self.channel_2]
            .into_iter()
            .filter_map(|channel| WAVEFORM_CHANNELS.iter().position(|&source| source == channel))
        {
            lanes[len] = lane;
            len += 1;
        }
        (lanes, len)
    }

    fn column(&self, lane: usize, col: usize) -> WaveColumn {
        self.data[col][lane]
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
        (samples, colors)
    }

    fn column_color(&self, column: WaveColumn) -> Color {
        match self.color_mode {
            WaveformColorMode::Frequency => self.style.band_mix_color(column.color_bands),
            WaveformColorMode::Loudness => {
                let peak = column.min.abs().max(column.max.abs());
                let peak_db = power_to_db(peak * peak, DB_FLOOR);
                self.style.sample_color(if peak_db.is_finite() {
                    ((peak_db - LOUDNESS_QUIET_DB) / -LOUDNESS_QUIET_DB).clamp(0.0, 1.0)
                } else {
                    0.0
                })
            }
            WaveformColorMode::Static => self.style.sample_color(0.0),
        }
    }

    fn build_preview(&self, lanes: &[usize]) -> (Vec<PreviewSample>, f32) {
        let preview = &self.preview;
        let Some(columns) = preview.columns else {
            return (Vec::new(), 0.0);
        };
        if preview.progress <= 0.0 {
            return (Vec::new(), 0.0);
        }

        let result = lanes
            .iter()
            .map(|&lane| {
                let column = columns[lane];
                PreviewSample {
                    min: column.min,
                    max: column.max,
                    color: color_to_rgba(self.column_color(column)),
                }
            })
            .collect();

        (result, preview.progress.clamp(0.0, 1.0))
    }

    fn build_history_levels(&self, lanes: &[usize], start: usize, visible: usize) -> Vec<f32> {
        let history: fn(WaveColumn) -> [f32; NUM_BANDS] = match self.history_mode {
            WaveformHistoryMode::Off => return Vec::new(),
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
        out
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
