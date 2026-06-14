// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::dsp::AudioBlock;
use crate::util::audio::{
    BAND_SPLITS_HZ, Channel, DB_FLOOR, DEFAULT_SAMPLE_RATE, power_to_db,
    project_interleaved_frame, sample_rates_differ, sanitize_sample_rate,
};
use std::sync::Arc;

pub const MIN_SCROLL_SPEED: f32 = 10.0;
pub const MAX_SCROLL_SPEED: f32 = 1000.0;
pub const MAX_COLUMN_CAPACITY: usize = 8_192;

const DEFAULT_SCROLL_SPEED: f32 = 300.0;
pub const DEFAULT_BAND_DB_FLOOR: f32 = -60.0;
const MIN_RUNTIME_SCROLL_SPEED: f32 = 1.0;
pub(crate) const WAVEFORM_CHANNELS: [Channel; 4] =
    [Channel::Left, Channel::Right, Channel::Mid, Channel::Side];
const DERIVED_CHANNELS: usize = WAVEFORM_CHANNELS.len();
const REFERENCE_SAMPLE_RATE: f32 = 44_100.0;
const BAND_COLOR_WINDOW_AT_44K1: usize = 2048;
const BAND_SLOW_WINDOW_AT_44K1: usize = 16_384;
const BAND_FILTER_Q: f32 = 0.71;
const BAND_COLOR_GAINS: [f32; NUM_BANDS] = [1.0, 0.7, 2.0];
pub(crate) const WAVEFORM_SILENCE_AMPLITUDE: f32 = 1.584_893_1e-5;
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
}

impl Default for WaveformConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            scroll_speed: DEFAULT_SCROLL_SPEED,
            max_columns: MAX_COLUMN_CAPACITY,
            analyze_bands: true,
        }
    }
}

impl WaveformConfig {
    fn normalized(mut self) -> Self {
        self.sample_rate = sanitize_sample_rate(self.sample_rate);
        self.scroll_speed = crate::util::finite_positive(self.scroll_speed)
            .map(|speed| speed.max(MIN_RUNTIME_SCROLL_SPEED))
            .unwrap_or(DEFAULT_SCROLL_SPEED);
        self.max_columns = self.max_columns.clamp(1, MAX_COLUMN_CAPACITY);
        self
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WaveColumn {
    pub min: f32,
    pub max: f32,
    pub peak_db: f32,
    pub color_bands: [f32; NUM_BANDS],
    pub rms_fast: [f32; NUM_BANDS],
    pub rms_slow: [f32; NUM_BANDS],
}

#[derive(Debug, Clone, Default)]
pub struct WaveformPreview {
    pub progress: f32,
    pub columns: Vec<WaveColumn>,
}

#[derive(Debug, Clone, Default)]
pub struct WaveformSnapshot {
    pub channels: usize,
    pub columns: usize,
    pub data: Arc<[WaveColumn]>,
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
            self.low.process(x).abs(),
            self.mid_lp.process(self.mid_hp.process(x)).abs(),
            self.high.process(x).abs(),
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
        self.index = (self.index + 1) % self.values.len();
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
    fast: BandWindow,
    slow: BandWindow,
}

impl BandTracker {
    fn new(sample_rate: f32) -> Self {
        let color_len = window_len(BAND_COLOR_WINDOW_AT_44K1, sample_rate);
        Self {
            filters: ThreeBand::new(sample_rate),
            color: BandWindow::new(color_len),
            fast: BandWindow::new(color_len),
            slow: BandWindow::new(window_len(BAND_SLOW_WINDOW_AT_44K1, sample_rate)),
        }
    }

    fn process(&mut self, sample: f32) {
        let bands = self.filters.process(sample);
        let color = std::array::from_fn(|band| bands[band] * BAND_COLOR_GAINS[band]);
        let power = bands.map(|value| value * value);
        self.color.push(color);
        self.fast.push(power);
        self.slow.push(power);
    }
}

pub struct WaveformProcessor {
    config: WaveformConfig,
    snapshot: WaveformSnapshot,
    source_channels: usize,
    ring: Vec<WaveColumn>,
    trackers: [BandTracker; DERIVED_CHANNELS],
    ring_head: usize,
    column_count: usize,
    column_phase: f32,
    current: [Option<(f32, f32, Option<f32>)>; DERIVED_CHANNELS],
    last_sample: [Option<f32>; DERIVED_CHANNELS],
    has_pending_changes: bool,
}

impl WaveformProcessor {
    pub fn new(config: WaveformConfig) -> Self {
        let config = config.normalized();
        Self {
            config,
            snapshot: WaveformSnapshot::default(),
            source_channels: 2,
            ring: vec![WaveColumn::default(); config.max_columns * DERIVED_CHANNELS],
            trackers: std::array::from_fn(|_| BandTracker::new(config.sample_rate)),
            ring_head: 0,
            column_count: 0,
            column_phase: 0.0,
            current: [None; DERIVED_CHANNELS],
            last_sample: [None; DERIVED_CHANNELS],
            has_pending_changes: false,
        }
    }

    pub fn config(&self) -> WaveformConfig {
        self.config
    }

    fn rebuild(&mut self) {
        self.snapshot = WaveformSnapshot::default();
        self.ring_head = 0;
        self.column_count = 0;
        self.column_phase = 0.0;
        self.last_sample = [None; DERIVED_CHANNELS];
        self.has_pending_changes = false;
        self.reset_column();
        self.ring
            .resize(self.config.max_columns * DERIVED_CHANNELS, WaveColumn::default());
        self.reset_trackers();
    }

    fn reset_trackers(&mut self) {
        self.trackers = std::array::from_fn(|_| BandTracker::new(self.config.sample_rate));
    }

    fn reset_column(&mut self) {
        self.current = [None; DERIVED_CHANNELS];
    }

    fn column_for(&self, channel: usize) -> WaveColumn {
        let (min, max) = self.current[channel]
            .map(|(mut min, mut max, _)| {
                if let Some(last) = self.last_sample[channel] {
                    min = min.min(last);
                    max = max.max(last);
                }
                (min, max)
            })
            .unwrap_or((0.0, 0.0));
        let peak = min.abs().max(max.abs());
        let mut column = WaveColumn {
            min,
            max,
            peak_db: power_to_db(peak * peak, DB_FLOOR),
            ..WaveColumn::default()
        };
        if self.config.analyze_bands {
            let tracker = &self.trackers[channel];
            column.color_bands = tracker.color.means();
            column.rms_fast = tracker.fast.means();
            column.rms_slow = tracker.slow.means();
        }
        column
    }

    fn emit_column(&mut self) {
        let max_columns = self.config.max_columns;
        for channel in 0..DERIVED_CHANNELS {
            self.ring[channel * max_columns + self.ring_head] = self.column_for(channel);
            if let Some((_, _, Some(last))) = self.current[channel] {
                self.last_sample[channel] = Some(last);
            }
        }

        self.ring_head = (self.ring_head + 1) % max_columns;
        self.column_count = (self.column_count + 1).min(max_columns);
        self.has_pending_changes = true;
        self.reset_column();
    }

    fn ingest_samples(&mut self, samples: &[f32], channels: usize) {
        let step = (self.config.scroll_speed / self.config.sample_rate).clamp(0.0, 1.0);
        for frame in samples.chunks_exact(channels) {
            for (channel, source) in WAVEFORM_CHANNELS.into_iter().enumerate() {
                let sample = project_interleaved_frame(frame, channels, source).unwrap_or(0.0);
                let finite = sample.is_finite();
                let sample = if finite { sample } else { 0.0 };
                if self.config.analyze_bands {
                    self.trackers[channel].process(sample);
                }
                if finite {
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
    }

    fn sync_ring_to_snapshot(&mut self) {
        let (max_columns, visible) = (self.config.max_columns, self.column_count);
        let mut data = Vec::with_capacity(DERIVED_CHANNELS * visible);
        self.snapshot.channels = DERIVED_CHANNELS;
        self.snapshot.columns = visible;

        if visible > 0 {
            let start = if visible < max_columns { 0 } else { self.ring_head };
            for src in self.ring.chunks_exact(max_columns) {
                if start == 0 {
                    data.extend_from_slice(&src[..visible]);
                } else {
                    data.extend_from_slice(&src[start..]);
                    data.extend_from_slice(&src[..start]);
                }
            }
        }

        self.snapshot.data = Arc::from(data);
        self.has_pending_changes = false;
    }

    fn sync_preview(&mut self) {
        let progress = self.column_phase.clamp(0.0, 1.0);
        let columns: Option<[WaveColumn; DERIVED_CHANNELS]> =
            (progress > 0.0).then(|| std::array::from_fn(|ch| self.column_for(ch)));
        self.snapshot.preview.progress = progress;
        self.snapshot.preview.columns.clear();
        if let Some(columns) = columns {
            self.snapshot.preview.columns.extend(columns);
        }
    }

    pub fn process_block(&mut self, block: &AudioBlock<'_>) -> Option<WaveformSnapshot> {
        if block.is_empty() {
            return None;
        }

        let (channels, sample_rate) = (block.channels.max(1), block.sample_rate);
        if channels != self.source_channels || sample_rates_differ(self.config.sample_rate, sample_rate)
        {
            self.source_channels = channels;
            self.config.sample_rate = sanitize_sample_rate(sample_rate);
            self.rebuild();
        }

        self.ingest_samples(block.samples, channels);

        if self.has_pending_changes {
            self.sync_ring_to_snapshot();
        }

        self.sync_preview();

        Some(self.snapshot.clone())
    }

    pub fn update_config(&mut self, config: WaveformConfig) {
        let normalized = config.normalized();
        let rebuild = sample_rates_differ(self.config.sample_rate, normalized.sample_rate)
            || self.config.max_columns != normalized.max_columns;
        let reset_analysis = self.config.analyze_bands != normalized.analyze_bands;
        self.config = normalized;
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
        AudioBlock::now(samples, channels, sample_rate)
    }

    fn config(scroll_speed: f32, max_columns: usize) -> WaveformConfig {
        WaveformConfig {
            sample_rate: RATE,
            scroll_speed,
            max_columns,
            ..Default::default()
        }
    }

    fn extract(update: Option<WaveformSnapshot>) -> WaveformSnapshot {
        update.expect("expected snapshot")
    }

    fn column(snapshot: &WaveformSnapshot, channel: usize, col: usize) -> WaveColumn {
        snapshot.data[channel * snapshot.columns + col]
    }

    fn band(snapshot: &WaveformSnapshot, channel: usize, band: usize) -> f32 {
        column(snapshot, channel, snapshot.columns - 1).color_bands[band]
    }

    #[test]
    fn derives_mid_and_side_before_extrema() {
        let mut processor = WaveformProcessor::new(config(RATE / 2.0, 8));
        let snapshot = extract(processor.process_block(&block(&[1.0, 0.0, 0.0, 1.0], 2, RATE)));

        assert_eq!(snapshot.channels, DERIVED_CHANNELS);
        assert_eq!(snapshot.columns, 1);
        assert_eq!(column(&snapshot, 2, 0).max, 0.5);
        assert_eq!(column(&snapshot, 2, 0).min, 0.5);
        assert_eq!(column(&snapshot, 3, 0).max, 0.5);
        assert_eq!(column(&snapshot, 3, 0).min, -0.5);
    }

    #[test]
    fn mono_maps_to_left_right_mid_with_silent_side() {
        let mut processor = WaveformProcessor::new(config(RATE / 2.0, 8));
        let snapshot = extract(processor.process_block(&block(&[0.25, -0.5], 1, RATE)));

        for channel in 0..3 {
            assert_eq!(column(&snapshot, channel, 0).min, -0.5);
            assert_eq!(column(&snapshot, channel, 0).max, 0.25);
        }
        assert_eq!(column(&snapshot, 3, 0).min, 0.0);
        assert_eq!(column(&snapshot, 3, 0).max, 0.0);
    }

    #[test]
    fn multichannel_mid_averages_all_sources() {
        let mut processor = WaveformProcessor::new(config(RATE / 2.0, 8));
        let snapshot = extract(processor.process_block(&block(
            &[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            4,
            RATE,
        )));

        assert_eq!(column(&snapshot, 2, 0).min, 0.25);
        assert_eq!(column(&snapshot, 2, 0).max, 0.75);
    }

    #[test]
    fn previous_sample_continuity_catches_column_boundary_steps() {
        let mut processor = WaveformProcessor::new(config(RATE / 2.0, 8));
        let snapshot = extract(processor.process_block(&block(&[0.0, 0.0, 1.0, 1.0], 1, RATE)));

        assert_eq!(snapshot.columns, 2);
        assert_eq!(column(&snapshot, 0, 1).min, 0.0);
        assert_eq!(column(&snapshot, 0, 1).max, 1.0);
    }

    #[test]
    fn non_finite_samples_are_sanitized_and_break_column_continuity() {
        let mut processor = WaveformProcessor::new(config(RATE, 8));
        let snapshot = extract(processor.process_block(&block(
            &[0.0, f32::NAN, f32::INFINITY, 1.0],
            1,
            RATE,
        )));

        assert_eq!(snapshot.columns, 4);
        assert_eq!(column(&snapshot, 0, 3).min, 1.0);
        assert_eq!(column(&snapshot, 0, 3).max, 1.0);
        assert!(snapshot.data.iter().all(|c| c.min.is_finite() && c.max.is_finite()));
        assert!(snapshot
            .data
            .iter()
            .flat_map(|c| c.color_bands)
            .all(|v| v.is_finite()));
    }

    #[test]
    fn disabled_band_analysis_emits_zero_band_data() {
        let mut processor = WaveformProcessor::new(config(RATE, 128));
        extract(processor.process_block(&block(&[1.0; 32], 1, RATE)));

        let mut updated = processor.config();
        updated.analyze_bands = false;
        processor.update_config(updated);
        let snapshot = extract(processor.process_block(&block(&[0.0], 1, RATE)));
        let latest = column(&snapshot, 0, snapshot.columns - 1);

        assert_eq!(latest.color_bands, [0.0; NUM_BANDS]);
        assert_eq!(latest.rms_fast, [0.0; NUM_BANDS]);
        assert_eq!(latest.rms_slow, [0.0; NUM_BANDS]);
    }

    #[test]
    fn bands_follow_sine_frequency() {
        fn latest_bands_for(freq: f32) -> [f32; NUM_BANDS] {
            let mut processor = WaveformProcessor::new(config(200.0, 512));
            let samples: Vec<f32> = (0..RATE as usize)
                .map(|n| (2.0 * PI * freq * n as f32 / RATE).sin() * 0.8)
                .collect();
            let snapshot = extract(processor.process_block(&block(&samples, 1, RATE)));
            std::array::from_fn(|b| band(&snapshot, 0, b))
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
        let mut processor = WaveformProcessor::new(config(100.0, 512));
        let mut samples = vec![0.0; RATE as usize];
        samples.extend(vec![1.0; BAND_COLOR_WINDOW_AT_44K1]);
        let snapshot = extract(processor.process_block(&block(&samples, 1, RATE)));
        let latest = column(&snapshot, 0, snapshot.columns - 1);

        assert!(latest.rms_fast[0] > latest.rms_slow[0]);
    }

    #[test]
    fn config_updates_preserve_existing_columns_when_capacity_is_unchanged() {
        let mut processor = WaveformProcessor::new(WaveformConfig {
            analyze_bands: false,
            ..config(RATE / 2.0, 8)
        });
        let before = extract(processor.process_block(&block(&[0.1, 0.1, 0.2, 0.2], 1, RATE)));

        let mut updated = processor.config();
        updated.scroll_speed = RATE;
        updated.analyze_bands = true;
        processor.update_config(updated);
        let after = extract(processor.process_block(&block(&[0.3], 1, RATE)));

        assert_eq!(before.columns, 2);
        assert_eq!(column(&after, 0, 0).max, 0.1);
        assert_eq!(column(&after, 0, 1).max, 0.2);
        assert_eq!(column(&after, 0, 2).max, 0.3);
    }

    #[test]
    fn fractional_timing_matches_requested_average_speed() {
        let rate = 1_000.0;
        let speed = 333.0;
        let mut processor = WaveformProcessor::new(WaveformConfig {
            sample_rate: rate,
            scroll_speed: speed,
            max_columns: 4_000,
            ..Default::default()
        });
        let samples = vec![0.0; (rate as usize) * 10];
        let snapshot = extract(processor.process_block(&block(&samples, 1, rate)));

        assert!((snapshot.columns as isize - 3330).abs() <= 1);
    }

    #[test]
    fn snapshot_retains_latest_columns_after_history_exceeds_capacity() {
        let mut processor = WaveformProcessor::new(config(RATE / 2.0, 4));
        let snapshot = extract(processor.process_block(&block(
            &[0.1, 0.1, 0.2, 0.2, 0.3, 0.3, 0.4, 0.4, 0.5, 0.5],
            1,
            RATE,
        )));

        assert_eq!(snapshot.columns, 4);
        assert_eq!(
            (0..4).map(|i| column(&snapshot, 0, i).max).collect::<Vec<_>>(),
            [0.2, 0.3, 0.4, 0.5]
        );
    }
}
