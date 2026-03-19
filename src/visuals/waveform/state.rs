// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{
    DEFAULT_COLUMN_CAPACITY, MAX_COLUMN_CAPACITY, MIN_COLUMN_CAPACITY, NUM_BANDS, WaveformConfig,
    WaveformPreview, WaveformProcessor as CoreWaveformProcessor, WaveformSnapshot,
};
use super::render::{PreviewSample, WaveformParams, WaveformPrimitive};
use crate::persistence::settings::{ChannelMode, WaveformColorMode, WaveformSettings};
use crate::util::color;
use crate::visuals::palettes;
use crate::visuals::palettes::waveform::GRADIENT_STOPS;
use crate::visuals::project_channel_data;
use crate::{vis_processor, visualization_widget};
use iced::Color;
use std::cell::Cell;
use std::sync::Arc;

const COLUMN_WIDTH_PIXELS: f32 = 1.0;

type SampleColorData = (Arc<[[f32; 2]]>, Arc<[[f32; 4]]>);

vis_processor!(
    WaveformProcessor,
    CoreWaveformProcessor,
    WaveformConfig,
    WaveformSnapshot
);

impl WaveformProcessor {
    pub fn sync_capacity(&mut self, desired: usize) {
        let target = desired.clamp(MIN_COLUMN_CAPACITY, MAX_COLUMN_CAPACITY);
        let mut config = self.config();
        if config.max_columns != target {
            config.max_columns = target;
            self.update_config(config);
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WaveformState {
    snapshot: WaveformSnapshot,
    style: WaveformStyle,
    desired_columns: Cell<usize>,
    key: u64,
    channel_mode: ChannelMode,
    color_mode: WaveformColorMode,
    show_peak_history: bool,
}

impl WaveformState {
    pub fn new() -> Self {
        let defaults = WaveformSettings::default();
        Self {
            snapshot: WaveformSnapshot::default(),
            style: WaveformStyle::default(),
            desired_columns: Cell::new(DEFAULT_COLUMN_CAPACITY),
            key: crate::visuals::next_key(),
            channel_mode: defaults.channel_mode,
            color_mode: defaults.color_mode,
            show_peak_history: defaults.show_peak_history,
        }
    }

    pub fn apply_snapshot(&mut self, snapshot: &WaveformSnapshot) {
        self.snapshot = Self::project(snapshot, self.channel_mode);
    }

    pub fn set_channel_mode(&mut self, mode: ChannelMode) {
        if self.channel_mode != mode {
            self.channel_mode = mode;
            self.snapshot = Self::project(&self.snapshot, mode);
        }
    }

    pub fn channel_mode(&self) -> ChannelMode {
        self.channel_mode
    }

    pub fn set_color_mode(&mut self, mode: WaveformColorMode) {
        self.color_mode = mode;
    }

    pub fn color_mode(&self) -> WaveformColorMode {
        self.color_mode
    }

    pub fn set_show_peak_history(&mut self, show: bool) {
        self.show_peak_history = show;
    }

    pub fn show_peak_history(&self) -> bool {
        self.show_peak_history
    }

    pub fn set_palette(&mut self, palette: &[Color; 9]) {
        if !color::palettes_equal(&self.style.palette, palette) {
            self.style.palette = *palette;
        }
    }

    pub fn palette(&self) -> &[Color; 9] {
        &self.style.palette
    }

    pub fn desired_columns(&self) -> usize {
        self.desired_columns.get()
    }

    pub fn visual_params(&self, bounds: iced::Rectangle) -> Option<WaveformParams> {
        if !self.has_renderable_data(bounds.width) {
            return None;
        }

        let channels = self.snapshot.channels.max(1);
        let total_columns = self.snapshot.columns;
        let needed =
            ((bounds.width / COLUMN_WIDTH_PIXELS).ceil() as usize).clamp(1, MAX_COLUMN_CAPACITY);
        self.desired_columns.set(needed);

        let visible = needed.min(total_columns);
        let start = total_columns.saturating_sub(needed);
        let (samples, colors) = self.build_sample_data(channels, total_columns, start, visible);
        let (preview_samples, preview_progress) = self.build_preview(channels);

        let band_levels = if self.show_peak_history {
            self.build_band_levels(channels, total_columns, start, visible)
        } else {
            Arc::from([])
        };

        Some(WaveformParams {
            bounds,
            channels,
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

    fn has_renderable_data(&self, width: f32) -> bool {
        if width <= 0.0 || self.snapshot.columns == 0 {
            return false;
        }
        let expected_len = self.snapshot.columns * self.snapshot.channels.max(1);
        self.snapshot.min_values.len() == expected_len
            && self.snapshot.max_values.len() == expected_len
            && self.snapshot.frequency_normalized.len() == expected_len
    }

    fn color_intensity(&self, min_sample: f32, max_sample: f32, frequency: f32) -> f32 {
        match self.color_mode {
            WaveformColorMode::Frequency => frequency,
            WaveformColorMode::Loudness => (max_sample - min_sample).abs().min(1.0),
            WaveformColorMode::Static => 0.0,
        }
    }

    fn build_sample_data(
        &self,
        channels: usize,
        total_columns: usize,
        start: usize,
        visible: usize,
    ) -> SampleColorData {
        let mut samples = Vec::with_capacity(visible * channels);
        let mut colors = Vec::with_capacity(visible * channels);

        for channel in 0..channels {
            let base = channel * total_columns;
            for col in start..(start + visible) {
                let idx = base + col;
                let (min, max) = (self.snapshot.min_values[idx], self.snapshot.max_values[idx]);
                let freq = self.snapshot.frequency_normalized[idx];
                let intensity = self.color_intensity(min, max, freq);

                samples.push([min.min(max), min.max(max)]);
                colors.push(color::color_to_rgba(self.style.sample_color(intensity)));
            }
        }
        (Arc::from(samples), Arc::from(colors))
    }

    fn build_preview(&self, channels: usize) -> (Arc<[PreviewSample]>, f32) {
        let preview = &self.snapshot.preview;
        let valid = preview.progress > 0.0
            && preview.min_values.len() >= channels
            && preview.max_values.len() >= channels;

        if !valid {
            return (Arc::from([]), 0.0);
        }

        let result: Vec<_> = (0..channels)
            .map(|ch| {
                let (min, max) = (preview.min_values[ch], preview.max_values[ch]);
                let freq = self.latest_frequency_for_channel(ch);
                let intensity = self.color_intensity(min, max, freq);

                PreviewSample {
                    min: min.min(max).clamp(-1.0, 1.0),
                    max: min.max(max).clamp(-1.0, 1.0),
                    color: color::color_to_rgba(self.style.sample_color(intensity)),
                }
            })
            .collect();

        (Arc::from(result), preview.progress.clamp(0.0, 1.0))
    }

    fn build_band_levels(
        &self,
        channels: usize,
        total_columns: usize,
        start: usize,
        visible: usize,
    ) -> Arc<[f32]> {
        let expected = channels * NUM_BANDS * total_columns;
        if self.snapshot.band_levels.len() < expected {
            return Arc::from([]);
        }
        let mut out = Vec::with_capacity(channels * NUM_BANDS * visible);
        for channel in 0..channels {
            for band in 0..NUM_BANDS {
                let base = (channel * NUM_BANDS + band) * total_columns;
                out.extend_from_slice(
                    &self.snapshot.band_levels[base + start..base + start + visible],
                );
            }
        }
        Arc::from(out)
    }

    fn latest_frequency_for_channel(&self, channel: usize) -> f32 {
        let cols = self.snapshot.columns;
        self.snapshot
            .frequency_normalized
            .get(channel * cols..(channel + 1) * cols)
            .and_then(|r| r.iter().rev().copied().find(|&v| v.is_finite() && v > 0.0))
            .unwrap_or(0.0)
    }

    fn project(source: &WaveformSnapshot, mode: ChannelMode) -> WaveformSnapshot {
        let (channels, columns) = (source.channels.max(1), source.columns);
        let expected = channels * columns;
        let valid = columns > 0
            && source.min_values.len() >= expected
            && source.max_values.len() >= expected
            && source.frequency_normalized.len() >= expected;

        if !valid {
            return WaveformSnapshot::default();
        }

        let remap = |data: &[f32], stride| project_channel_data(mode, data, stride, channels);

        let preview = &source.preview;
        let preview_valid =
            preview.min_values.len() >= channels && preview.max_values.len() >= channels;

        WaveformSnapshot {
            channels: mode.output_channels(channels),
            columns,
            min_values: remap(&source.min_values, columns),
            max_values: remap(&source.max_values, columns),
            frequency_normalized: remap(&source.frequency_normalized, columns),
            band_levels: remap(&source.band_levels, NUM_BANDS * columns),
            column_spacing_seconds: source.column_spacing_seconds,
            scroll_position: source.scroll_position,
            preview: if preview_valid {
                WaveformPreview {
                    progress: preview.progress,
                    min_values: remap(&preview.min_values, 1),
                    max_values: remap(&preview.max_values, 1),
                }
            } else {
                WaveformPreview::default()
            },
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WaveformStyle {
    pub fill_alpha: f32,
    pub vertical_padding: f32,
    pub channel_gap: f32,
    pub amplitude_scale: f32,
    pub palette: [Color; 9],
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
        color::sample_gradient(&self.palette[..GRADIENT_STOPS], intensity)
    }

    fn band_colors(&self) -> [[f32; 4]; NUM_BANDS] {
        std::array::from_fn(|i| color::color_to_rgba(self.palette[GRADIENT_STOPS + i]))
    }
}

visualization_widget!(Waveform, WaveformState, WaveformPrimitive);
