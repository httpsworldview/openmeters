// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::dsp::AudioBlock;
use crate::util::audio::{
    BAND_SPLITS_HZ, Channel, DB_FLOOR, DEFAULT_SAMPLE_RATE, power_to_db, sample_rates_differ,
    sanitize_sample_rate,
};

pub const MIN_SCROLL_SPEED: f32 = 10.0;
pub const MAX_SCROLL_SPEED: f32 = 1000.0;
pub const MAX_COLUMN_CAPACITY: usize = 8_192;

const DEFAULT_SCROLL_SPEED: f32 = 300.0;
pub const DEFAULT_BAND_DB_FLOOR: f32 = -60.0;
const MIN_RUNTIME_SCROLL_SPEED: f32 = 1.0;
pub(super) const WAVEFORM_CHANNELS: [Channel; 4] =
    [Channel::Left, Channel::Right, Channel::Mid, Channel::Side];
pub(super) const DERIVED_CHANNELS: usize = WAVEFORM_CHANNELS.len();
const REFERENCE_SAMPLE_RATE: f32 = 44_100.0;
const BAND_COLOR_WINDOW_AT_44K1: usize = 2048;
const BAND_SLOW_WINDOW_AT_44K1: usize = 16_384;
const BAND_FILTER_Q: f32 = 0.71;
const BAND_COLOR_GAINS: [f32; NUM_BANDS] = [1.0, 0.7, 2.0];
pub(super) const WAVEFORM_SILENCE_AMPLITUDE: f32 = 1.584_893_1e-5;
const MAX_TRACKER_SAMPLE_RATE: f32 = 1_000_000.0;

pub const NUM_BANDS: usize = 3;
pub const MIN_BAND_DB_FLOOR: f32 = -96.0;
pub const MAX_BAND_DB_FLOOR: f32 = -12.0;

#[derive(Debug, Clone, Copy)]
pub struct WaveformConfig {
    pub sample_rate: f32,
    pub scroll_speed: f32,
    pub max_columns: usize,
    pub analyze_bands: bool,
    pub track_history: bool,
}

impl Default for WaveformConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            scroll_speed: DEFAULT_SCROLL_SPEED,
            max_columns: MAX_COLUMN_CAPACITY,
            analyze_bands: true,
            track_history: false,
        }
    }
}

impl WaveformConfig {
    fn normalized(mut self) -> Self {
        self.sample_rate = sanitize_sample_rate(self.sample_rate);
        self.scroll_speed = crate::util::finite_positive(self.scroll_speed)
            .map_or(DEFAULT_SCROLL_SPEED, |speed| speed.max(MIN_RUNTIME_SCROLL_SPEED));
        self.max_columns = self.max_columns.clamp(1, MAX_COLUMN_CAPACITY);
        self.track_history &= self.analyze_bands;
        self
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WaveColumn {
    pub min: f32,
    pub max: f32,
    pub color_bands: [f32; NUM_BANDS],
    pub rms_fast_db: [f32; NUM_BANDS],
    pub rms_slow_db: [f32; NUM_BANDS],
}

impl Default for WaveColumn {
    fn default() -> Self {
        Self {
            min: 0.0,
            max: 0.0,
            color_bands: [0.0; NUM_BANDS],
            rms_fast_db: [DB_FLOOR; NUM_BANDS],
            rms_slow_db: [DB_FLOOR; NUM_BANDS],
        }
    }
}

pub(super) type WaveFrame = [WaveColumn; DERIVED_CHANNELS];

#[derive(Debug, Clone, Copy, Default)]
pub struct WaveformPreview {
    pub progress: f32,
    pub columns: Option<WaveFrame>,
}

#[derive(Debug)]
pub struct WaveformUpdate<'a> {
    pub reset: bool,
    pub columns: &'a [WaveFrame],
    pub preview: WaveformPreview,
}

fn window_len(samples_at_reference_rate: usize, sample_rate: f32) -> usize {
    let sample_rate = sample_rate.min(MAX_TRACKER_SAMPLE_RATE);
    ((samples_at_reference_rate as f32 * sample_rate / REFERENCE_SAMPLE_RATE).round() as usize)
        .max(1)
}

#[derive(Clone, Copy)]
enum FilterKind {
    LowPass,
    HighPass,
}

struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn new(kind: FilterKind, sample_rate: f32, frequency: f32) -> Self {
        let w0 = core::f32::consts::TAU * (frequency / sample_rate).clamp(1.0e-6, 0.49);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * BAND_FILTER_Q);
        let (b0, b1, b2) = match kind {
            FilterKind::LowPass => ((1.0 - cos) * 0.5, 1.0 - cos, (1.0 - cos) * 0.5),
            FilterKind::HighPass => ((1.0 + cos) * 0.5, -(1.0 + cos), (1.0 + cos) * 0.5),
        };
        let inv_a0 = 1.0 / (1.0 + alpha);
        Self {
            b0: b0 * inv_a0,
            b1: b1 * inv_a0,
            b2: b2 * inv_a0,
            a1: -2.0 * cos * inv_a0,
            a2: (1.0 - alpha) * inv_a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0.mul_add(x, self.z1);
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        if y.is_finite() {
            y
        } else {
            self.z1 = 0.0;
            self.z2 = 0.0;
            0.0
        }
    }
}

struct ThreeBand {
    low: Biquad,
    mid_hp: Biquad,
    mid_lp: Biquad,
    high: Biquad,
}

impl ThreeBand {
    fn new(sample_rate: f32) -> Self {
        let [low_split, high_split] = BAND_SPLITS_HZ;
        Self {
            low: Biquad::new(FilterKind::LowPass, sample_rate, low_split),
            mid_hp: Biquad::new(FilterKind::HighPass, sample_rate, low_split),
            mid_lp: Biquad::new(FilterKind::LowPass, sample_rate, high_split),
            high: Biquad::new(FilterKind::HighPass, sample_rate, high_split),
        }
    }

    fn process(&mut self, x: f32) -> [f32; NUM_BANDS] {
        [
            self.low.process(x),
            self.mid_lp.process(self.mid_hp.process(x)),
            self.high.process(x),
        ]
    }
}

struct BandWindow {
    values: Vec<[f32; NUM_BANDS]>,
    sums: [f32; NUM_BANDS],
    index: usize,
    len: usize,
}

impl BandWindow {
    fn new(len: usize) -> Self {
        Self {
            values: vec![[0.0; NUM_BANDS]; len.max(1)],
            sums: [0.0; NUM_BANDS],
            index: 0,
            len: 0,
        }
    }

    fn push(&mut self, values: [f32; NUM_BANDS]) {
        let values = values.map(|value| if value.is_finite() { value } else { 0.0 });
        let old = if self.len < self.values.len() {
            self.len += 1;
            [0.0; NUM_BANDS]
        } else {
            self.values[self.index]
        };
        self.values[self.index] = values;
        self.index += 1;
        if self.index == self.values.len() {
            self.index = 0;
        }
        for band in 0..NUM_BANDS {
            self.sums[band] += values[band] - old[band];
        }
    }

    fn means(&self) -> [f32; NUM_BANDS] {
        if self.len == 0 {
            [0.0; NUM_BANDS]
        } else {
            self.sums.map(|sum| sum / self.len as f32)
        }
    }
}

struct BandTracker {
    filters: ThreeBand,
    color: BandWindow,
    fast: Option<BandWindow>,
    slow: Option<BandWindow>,
}

impl BandTracker {
    fn new(sample_rate: f32, track_history: bool) -> Self {
        let color_len = window_len(BAND_COLOR_WINDOW_AT_44K1, sample_rate);
        Self {
            filters: ThreeBand::new(sample_rate),
            color: BandWindow::new(color_len),
            fast: track_history.then(|| BandWindow::new(color_len)),
            slow: track_history
                .then(|| BandWindow::new(window_len(BAND_SLOW_WINDOW_AT_44K1, sample_rate))),
        }
    }

    fn process(&mut self, sample: f32) {
        let bands = self.filters.process(sample);
        self.color.push(std::array::from_fn(|band| {
            bands[band].abs() * BAND_COLOR_GAINS[band]
        }));
        if let (Some(fast), Some(slow)) = (&mut self.fast, &mut self.slow) {
            let power = bands.map(|value| value * value);
            fast.push(power);
            slow.push(power);
        }
    }
}

fn derived_frame(frame: &[f32]) -> [f32; DERIVED_CHANNELS] {
    let left = frame[0];
    let right = frame.get(1).copied().unwrap_or(left);
    let side = if frame.len() > 1 { (left - right) * 0.5 } else { 0.0 };
    [left, right, frame.iter().sum::<f32>() / frame.len() as f32, side]
}

fn process_bands(
    trackers: &mut [BandTracker; DERIVED_CHANNELS],
    derived: [f32; DERIVED_CHANNELS],
    finite: [bool; DERIVED_CHANNELS],
) {
    for channel in 0..DERIVED_CHANNELS {
        trackers[channel].process(if finite[channel] { derived[channel] } else { 0.0 });
    }
}

pub struct WaveformProcessor {
    config: WaveformConfig,
    source_channels: usize,
    trackers: Option<[BandTracker; DERIVED_CHANNELS]>,
    column_phase: f32,
    current: [Option<(f32, f32, Option<f32>)>; DERIVED_CHANNELS],
    last_sample: [Option<f32>; DERIVED_CHANNELS],
    pending_columns: Vec<WaveFrame>,
    reset_pending: bool,
}

impl WaveformProcessor {
    pub fn new(config: WaveformConfig) -> Self {
        let config = config.normalized();
        Self {
            config,
            source_channels: 2,
            trackers: Self::trackers(config),
            column_phase: 0.0,
            current: [None; DERIVED_CHANNELS],
            last_sample: [None; DERIVED_CHANNELS],
            pending_columns: Vec::new(),
            reset_pending: true,
        }
    }

    pub fn config(&self) -> WaveformConfig {
        self.config
    }

    fn rebuild(&mut self) {
        self.column_phase = 0.0;
        self.last_sample = [None; DERIVED_CHANNELS];
        self.pending_columns.clear();
        self.reset_column();
        self.reset_trackers();
        self.reset_pending = true;
    }

    fn trackers(config: WaveformConfig) -> Option<[BandTracker; DERIVED_CHANNELS]> {
        config
            .analyze_bands
            .then(|| std::array::from_fn(|_| BandTracker::new(config.sample_rate, config.track_history)))
    }

    fn reset_trackers(&mut self) {
        self.trackers = Self::trackers(self.config);
    }

    fn fit_pending_capacity(&mut self) {
        let target = self.config.max_columns;
        if self.pending_columns.capacity() < target {
            self.pending_columns
                .reserve_exact(target.saturating_sub(self.pending_columns.len()));
        } else if self.pending_columns.capacity() > target.saturating_mul(2) {
            self.pending_columns.shrink_to(target);
        }
    }

    fn reset_column(&mut self) {
        self.current = [None; DERIVED_CHANNELS];
    }

    fn column_for(&self, channel: usize) -> WaveColumn {
        let (min, max) = self.current[channel].map_or((0.0, 0.0), |(mut min, mut max, _)| {
            if let Some(last) = self.last_sample[channel] {
                min = min.min(last);
                max = max.max(last);
            }
            (min, max)
        });
        let mut column = WaveColumn {
            min,
            max,
            ..WaveColumn::default()
        };
        if let Some(trackers) = &self.trackers {
            let tracker = &trackers[channel];
            column.color_bands = tracker.color.means();
            if self.config.track_history {
                column.rms_fast_db = tracker
                    .fast
                    .as_ref()
                    .map(BandWindow::means)
                    .unwrap_or_default()
                    .map(|power| power_to_db(power, DB_FLOOR));
                column.rms_slow_db = tracker
                    .slow
                    .as_ref()
                    .map(BandWindow::means)
                    .unwrap_or_default()
                    .map(|power| power_to_db(power, DB_FLOOR));
            }
        }
        column
    }

    fn emit_column(&mut self) {
        let columns = std::array::from_fn(|channel| self.column_for(channel));
        for channel in 0..DERIVED_CHANNELS {
            if let Some((_, _, Some(last))) = self.current[channel] {
                self.last_sample[channel] = Some(last);
            }
        }
        self.pending_columns.push(columns);
        self.reset_column();
    }

    fn ingest_samples(&mut self, samples: &[f32], channels: usize) {
        let step = (self.config.scroll_speed / self.config.sample_rate).clamp(0.0, 1.0);
        for frame in samples.chunks_exact(channels) {
            let derived = derived_frame(frame);
            self.ingest_frame(derived, derived.map(f32::is_finite), step);
        }
    }

    fn ingest_frame(&mut self, derived: [f32; DERIVED_CHANNELS], finite: [bool; DERIVED_CHANNELS], step: f32) {
        if let Some(trackers) = &mut self.trackers {
            process_bands(trackers, derived, finite);
        }
        self.ingest_derived(derived, finite, step);
    }

    fn ingest_derived(
        &mut self,
        derived: [f32; DERIVED_CHANNELS],
        finite: [bool; DERIVED_CHANNELS],
        step: f32,
    ) {
        for channel in 0..DERIVED_CHANNELS {
            if finite[channel] {
                let sample = derived[channel];
                self.current[channel] = Some(match self.current[channel] {
                    Some((min, max, _)) => (min.min(sample), max.max(sample), Some(sample)),
                    None => (sample, sample, Some(sample)),
                });
            } else {
                if let Some((_, _, last)) = &mut self.current[channel] {
                    *last = None;
                }
                self.last_sample[channel] = None;
            }
        }
        self.column_phase += step;
        if self.column_phase >= 1.0 {
            self.emit_column();
            self.column_phase -= 1.0;
        }
    }

    fn cap_pending_columns(&mut self) {
        if self.pending_columns.len() > self.config.max_columns {
            self.pending_columns
                .drain(..self.pending_columns.len() - self.config.max_columns);
        }
    }

    fn preview(&self) -> WaveformPreview {
        let progress = self.column_phase.clamp(0.0, 1.0);
        WaveformPreview {
            progress,
            columns: (progress > 0.0).then(|| std::array::from_fn(|ch| self.column_for(ch))),
        }
    }

    pub fn process_block(&mut self, block: &AudioBlock<'_>) -> Option<WaveformUpdate<'_>> {
        if block.is_empty() {
            return None;
        }

        self.pending_columns.clear();

        let (channels, sample_rate) = (block.channels.max(1), block.sample_rate);
        if channels != self.source_channels || sample_rates_differ(self.config.sample_rate, sample_rate)
        {
            self.source_channels = channels;
            self.config.sample_rate = sanitize_sample_rate(sample_rate);
            self.rebuild();
        }

        self.ingest_samples(block.samples, channels);

        self.cap_pending_columns();
        let reset = self.reset_pending;
        let preview = self.preview();
        self.reset_pending = false;
        Some(WaveformUpdate {
            reset,
            columns: &self.pending_columns,
            preview,
        })
    }

    pub fn update_config(&mut self, config: WaveformConfig) {
        let normalized = config.normalized();
        let rebuild = sample_rates_differ(self.config.sample_rate, normalized.sample_rate);
        let reset_analysis = self.config.analyze_bands != normalized.analyze_bands
            || self.config.track_history != normalized.track_history;
        let resize_pending = self.config.max_columns != normalized.max_columns;
        self.config = normalized;
        if resize_pending {
            self.fit_pending_capacity();
        }
        if rebuild {
            self.rebuild();
        } else if reset_analysis {
            self.reset_trackers();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    const RATE: f32 = 48_000.0;

    fn block(samples: &[f32], channels: usize, sample_rate: f32) -> AudioBlock<'_> {
        AudioBlock::new(samples, channels, sample_rate)
    }

    fn config(scroll_speed: f32, max_columns: usize) -> WaveformConfig {
        WaveformConfig {
            sample_rate: RATE,
            scroll_speed,
            max_columns,
            ..Default::default()
        }
    }

    fn process<'a>(
        processor: &'a mut WaveformProcessor,
        samples: &[f32],
        channels: usize,
    ) -> WaveformUpdate<'a> {
        processor
            .process_block(&block(samples, channels, RATE))
            .expect("expected update")
    }

    fn column(update: &WaveformUpdate, channel: usize, col: usize) -> WaveColumn {
        update.columns[col][channel]
    }

    fn latest(update: &WaveformUpdate, channel: usize) -> WaveColumn {
        column(update, channel, update.columns.len() - 1)
    }

    fn band(update: &WaveformUpdate, channel: usize, band: usize) -> f32 {
        latest(update, channel).color_bands[band]
    }

    #[test]
    fn channel_projection_feeds_extrema() {
        let mut processor = WaveformProcessor::new(config(RATE / 2.0, 8));
        let update = process(&mut processor, &[1.0, 0.0, 0.0, 1.0], 2);
        assert_eq!((column(&update, 2, 0).min, column(&update, 2, 0).max), (0.5, 0.5));
        assert_eq!((column(&update, 3, 0).min, column(&update, 3, 0).max), (-0.5, 0.5));

        let mut processor = WaveformProcessor::new(config(RATE / 2.0, 8));
        let update = process(&mut processor, &[0.25, -0.5], 1);
        for channel in 0..3 {
            assert_eq!((column(&update, channel, 0).min, column(&update, channel, 0).max), (-0.5, 0.25));
        }
        assert_eq!((column(&update, 3, 0).min, column(&update, 3, 0).max), (0.0, 0.0));

        let mut processor = WaveformProcessor::new(config(RATE / 2.0, 8));
        let update = process(
            &mut processor,
            &[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            4,
        );
        assert_eq!((column(&update, 2, 0).min, column(&update, 2, 0).max), (0.25, 0.75));
    }

    #[test]
    fn previous_sample_continuity_catches_column_boundary_steps() {
        let mut processor = WaveformProcessor::new(config(RATE / 2.0, 8));
        let update = process(&mut processor, &[0.0, 0.0, 1.0, 1.0], 1);

        assert_eq!(update.columns.len(), 2);
        assert_eq!(column(&update, 0, 1).min, 0.0);
        assert_eq!(column(&update, 0, 1).max, 1.0);
    }

    #[test]
    fn non_finite_samples_are_sanitized_and_break_column_continuity() {
        let mut processor = WaveformProcessor::new(config(RATE, 8));
        let update = process(&mut processor, &[0.0, f32::NAN, f32::INFINITY, 1.0], 1);

        assert_eq!(update.columns.len(), 4);
        assert_eq!(column(&update, 0, 3).min, 1.0);
        assert_eq!(column(&update, 0, 3).max, 1.0);
        assert!(update.columns.iter().flatten().all(|c| c.min.is_finite() && c.max.is_finite()));
        assert!(update
            .columns
            .iter()
            .flatten()
            .flat_map(|c| c.color_bands)
            .all(f32::is_finite));

        let mut processor = WaveformProcessor::new(config(RATE, 8));
        let update = process(&mut processor, &[f32::MAX, f32::MAX], 2);
        assert!(update.columns.iter().flatten().all(|c| c.min.is_finite() && c.max.is_finite()));
    }

    #[test]
    fn disabled_band_analysis_emits_zero_band_data() {
        let mut processor = WaveformProcessor::new(config(RATE, 128));
        let _ = process(&mut processor, &[1.0; 32], 1);

        let mut updated = processor.config();
        updated.analyze_bands = false;
        processor.update_config(updated);
        let latest = latest(&process(&mut processor, &[0.0], 1), 0);

        assert_eq!(latest.color_bands, [0.0; NUM_BANDS]);
        assert_eq!(latest.rms_fast_db, [DB_FLOOR; NUM_BANDS]);
        assert_eq!(latest.rms_slow_db, [DB_FLOOR; NUM_BANDS]);
    }

    #[test]
    fn bands_follow_sine_frequency() {
        fn latest_bands_for(freq: f32) -> [f32; NUM_BANDS] {
            let mut processor = WaveformProcessor::new(config(200.0, 512));
            let samples: Vec<f32> = (0..RATE as usize)
                .map(|n| (2.0 * PI * freq * n as f32 / RATE).sin() * 0.8)
                .collect();
            let update = process(&mut processor, &samples, 1);
            std::array::from_fn(|b| band(&update, 0, b))
        }

        let low = latest_bands_for(80.0);
        let mid = latest_bands_for(500.0);
        let high = latest_bands_for(5_000.0);

        assert!(low[0] > low[1] && low[0] > low[2], "low bands: {low:?}");
        assert!(mid[1] > mid[0] && mid[1] > mid[2], "mid bands: {mid:?}");
        assert!(high[2] > high[0] && high[2] > high[1], "high bands: {high:?}");
    }

    #[test]
    fn fast_rms_reacts_before_slow_rms() {
        let mut cfg = config(100.0, 512);
        cfg.track_history = true;
        let mut processor = WaveformProcessor::new(cfg);
        let mut samples = vec![0.0; RATE as usize];
        samples.extend(vec![1.0; BAND_COLOR_WINDOW_AT_44K1]);
        let latest = latest(&process(&mut processor, &samples, 1), 0);

        assert!(latest.rms_fast_db[0] > latest.rms_slow_db[0]);
    }

    #[test]
    fn fractional_timing_matches_requested_average_speed() {
        let mut processor = WaveformProcessor::new(WaveformConfig {
            sample_rate: 1_000.0,
            scroll_speed: 333.0,
            max_columns: 4_000,
            ..Default::default()
        });
        let samples = vec![0.0; 10_000];
        let update = processor.process_block(&block(&samples, 1, 1_000.0)).unwrap();

        assert!((update.columns.len() as isize - 3330).abs() <= 1);
    }

    #[test]
    fn update_payload_is_capped_to_configured_history() {
        let mut processor = WaveformProcessor::new(config(RATE, 4));
        let update = process(&mut processor, &[0.1, 0.2, 0.3, 0.4, 0.5], 1);

        assert_eq!(update.columns.len(), 4);
        assert_eq!(
            (0..4).map(|i| column(&update, 0, i).max).collect::<Vec<_>>(),
            [0.2, 0.3, 0.4, 0.5]
        );
    }
}
