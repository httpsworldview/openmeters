use crate::audio::meter_tap::MeterFormat;
use crate::dsp::waveform::{
    DEFAULT_COLUMN_CAPACITY, MAX_COLUMN_CAPACITY, MIN_COLUMN_CAPACITY, WaveformConfig,
    WaveformPreview, WaveformProcessor as CoreWaveformProcessor, WaveformSnapshot,
};
use crate::dsp::{AudioBlock, AudioProcessor, Reconfigurable};
use crate::ui::render::waveform::{PreviewSample, WaveformParams, WaveformPrimitive};
use crate::ui::settings::{ChannelMode, WaveformColorMode, WaveformSettings};
use crate::ui::theme;
use crate::util::audio::project_channel_data;
use crate::visualization_widget;
use iced::Color;
use std::cell::Cell;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

const COLUMN_WIDTH_PIXELS: f32 = 1.0;

type SampleColorData = (Arc<[[f32; 2]]>, Arc<[[f32; 4]]>);

#[derive(Debug, Clone)]
pub(crate) struct WaveformProcessor {
    inner: CoreWaveformProcessor,
}

impl WaveformProcessor {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            inner: CoreWaveformProcessor::new(WaveformConfig {
                sample_rate,
                ..Default::default()
            }),
        }
    }

    pub fn sync_capacity(&mut self, desired: usize) {
        let target = desired.clamp(MIN_COLUMN_CAPACITY, MAX_COLUMN_CAPACITY);
        let mut config = self.inner.config();
        if config.max_columns != target {
            config.max_columns = target;
            self.inner.update_config(config);
        }
    }

    pub fn ingest(&mut self, samples: &[f32], format: MeterFormat) -> Option<WaveformSnapshot> {
        if samples.is_empty() {
            return None;
        }
        let sample_rate = format.sample_rate.max(1.0);
        let mut config = self.inner.config();
        if (config.sample_rate - sample_rate).abs() > f32::EPSILON {
            config.sample_rate = sample_rate;
            self.inner.update_config(config);
        }
        self.inner.process_block(&AudioBlock::now(
            samples,
            format.channels.max(1),
            sample_rate,
        ))
    }

    pub fn update_config(&mut self, config: WaveformConfig) {
        self.inner.update_config(config);
    }

    pub fn config(&self) -> WaveformConfig {
        self.inner.config()
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
}

impl WaveformState {
    pub fn new() -> Self {
        let defaults = WaveformSettings::default();
        static NEXT_KEY: AtomicU64 = AtomicU64::new(1);
        Self {
            snapshot: WaveformSnapshot::default(),
            style: WaveformStyle::default(),
            desired_columns: Cell::new(DEFAULT_COLUMN_CAPACITY),
            key: NEXT_KEY.fetch_add(1, Ordering::Relaxed),
            channel_mode: defaults.channel_mode,
            color_mode: defaults.color_mode,
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

    pub fn set_palette(&mut self, palette: &[Color]) {
        self.style.try_update_palette(palette);
    }

    pub fn palette(&self) -> &[Color; 6] {
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

        Some(WaveformParams {
            bounds,
            channels,
            column_width: COLUMN_WIDTH_PIXELS,
            columns: visible,
            samples,
            colors,
            preview_samples,
            preview_progress,
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
                colors.push(theme::color_to_rgba(self.style.sample_color(intensity)));
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
                    color: theme::color_to_rgba(self.style.sample_color(intensity)),
                }
            })
            .collect();

        (Arc::from(result), preview.progress.clamp(0.0, 1.0))
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

        let remap = |data: &[f32], samples_per_ch| {
            project_channel_data(mode, data, samples_per_ch, channels)
        };

        let preview = &source.preview;
        let preview_valid =
            preview.min_values.len() >= channels && preview.max_values.len() >= channels;

        WaveformSnapshot {
            channels: mode.output_channels(channels),
            columns,
            min_values: remap(&source.min_values, columns),
            max_values: remap(&source.max_values, columns),
            frequency_normalized: remap(&source.frequency_normalized, columns),
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
    pub palette: [Color; 6],
}

impl Default for WaveformStyle {
    fn default() -> Self {
        Self {
            fill_alpha: 1.0,
            vertical_padding: 8.0,
            channel_gap: 12.0,
            amplitude_scale: 1.0,
            palette: theme::waveform::COLORS,
        }
    }
}

impl WaveformStyle {
    fn sample_color(&self, intensity: f32) -> Color {
        theme::sample_gradient(&self.palette, intensity)
    }

    fn try_update_palette(&mut self, palette: &[Color]) -> bool {
        let palette_changed = palette.len() == 6 && !theme::palettes_equal(&self.palette, palette);
        if palette_changed {
            self.palette.copy_from_slice(palette);
        }
        palette_changed
    }
}

visualization_widget!(
    Waveform,
    WaveformState,
    WaveformPrimitive,
    |state, bounds| state.visual_params(bounds),
    |params| WaveformPrimitive::new(params)
);
