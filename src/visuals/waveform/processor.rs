// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::dsp::AudioBlock;
use crate::util::audio::{
    BAND_SPLITS_HZ, DEFAULT_SAMPLE_RATE, WindowKind, apply_window, power_to_db, sample_rates_differ,
    sanitize_negative_db, sanitize_sample_rate, window_coefficients,
};
use realfft::{RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex32;
use std::sync::Arc;

pub const MIN_SCROLL_SPEED: f32 = 10.0;
pub const MAX_SCROLL_SPEED: f32 = 1000.0;
pub const MAX_COLUMN_CAPACITY: usize = 8_192;

const DEFAULT_SCROLL_SPEED: f32 = 300.0;
const DEFAULT_BAND_DB_FLOOR: f32 = -60.0;
const MIN_RUNTIME_SCROLL_SPEED: f32 = 1.0;
const FREQUENCY_FFT_SIZE: usize = 2048;

const MIN_FREQ_HZ: f32 = 20.0;
const MAX_FREQ_HZ: f32 = 5_000.0;

// EMA coefficient for smoothing the spectral centroid.
// lower = more smoothing, higher = more responsive.
const CENTROID_EMA_ALPHA: f32 = 0.4;

const BAND_EMA_ALPHA: f32 = 0.35;

pub const NUM_BANDS: usize = 3;
pub const MIN_BAND_DB_FLOOR: f32 = -96.0;
pub const MAX_BAND_DB_FLOOR: f32 = -12.0;

#[derive(Debug, Clone, Copy)]
pub struct WaveformConfig {
    pub sample_rate: f32,
    pub scroll_speed: f32,
    pub max_columns: usize,
    pub band_db_floor: f32,
}

impl Default for WaveformConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            scroll_speed: DEFAULT_SCROLL_SPEED,
            max_columns: MAX_COLUMN_CAPACITY,
            band_db_floor: DEFAULT_BAND_DB_FLOOR,
        }
    }
}

impl WaveformConfig {
    fn normalized(mut self) -> Self {
        self.sample_rate = sanitize_sample_rate(self.sample_rate);
        self.scroll_speed = crate::util::finite_positive(self.scroll_speed)
            .map(|speed| speed.max(MIN_RUNTIME_SCROLL_SPEED))
            .unwrap_or(DEFAULT_SCROLL_SPEED);
        self.band_db_floor = sanitize_negative_db(self.band_db_floor, DEFAULT_BAND_DB_FLOOR);
        self.max_columns = self.max_columns.clamp(1, MAX_COLUMN_CAPACITY);
        self
    }
    fn samples_per_column(&self) -> usize {
        let speed = self.scroll_speed.max(MIN_RUNTIME_SCROLL_SPEED);
        ((self.sample_rate / speed).round() as usize).max(1)
    }
}

#[derive(Debug, Clone, Default)]
pub struct WaveformPreview {
    pub progress: f32,
    pub min_values: Vec<f32>,
    pub max_values: Vec<f32>,
}

#[derive(Debug, Clone, Default)]
pub struct WaveformSnapshot {
    pub channels: usize,
    pub columns: usize,
    pub min_values: Vec<f32>,
    pub max_values: Vec<f32>,
    pub frequency_normalized: Vec<f32>,
    pub band_levels: Vec<f32>,
    pub column_spacing_seconds: f32,
    pub scroll_position: f32,
    pub preview: WaveformPreview,
}

fn sample_extrema(samples: &[f32]) -> (f32, f32) {
    let (min, max) = samples
        .iter()
        .copied()
        .filter(|sample| sample.is_finite())
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), sample| {
            (min.min(sample), max.max(sample))
        });
    if min.is_finite() {
        (min, max)
    } else {
        (0.0, 0.0)
    }
}

struct FrequencyAnalyzer {
    fft: Arc<dyn RealToComplex<f32>>,
    size: usize,
    input_buffer: Vec<f32>,
    output_spectrum: Vec<Complex32>,
    scratch: Vec<Complex32>,
    sample_history: Vec<f32>,
    bin_hz: f32,
    window_power_sum: f32,
    smoothed: f32,
    smoothed_bands: [f32; NUM_BANDS],
    band_bin_ranges: [(usize, usize); NUM_BANDS],
}

impl FrequencyAnalyzer {
    fn new(sample_rate: f32) -> Self {
        let size = FREQUENCY_FFT_SIZE;
        let fft = RealFftPlanner::new().plan_fft_forward(size);
        let bin_hz = sample_rate / size as f32;
        let spectrum_len = size / 2 + 1;
        let [s0, s1] = BAND_SPLITS_HZ.map(|hz| ((hz / bin_hz).round() as usize).min(spectrum_len));
        let band_bin_ranges: [(usize, usize); NUM_BANDS] = [(0, s0), (s0, s1), (s1, spectrum_len)];
        Self {
            scratch: fft.make_scratch_vec(),
            input_buffer: vec![0.0; size],
            output_spectrum: fft.make_output_vec(),
            sample_history: Vec::with_capacity(size),
            bin_hz,
            window_power_sum: 1.0,
            smoothed: 0.1,
            smoothed_bands: [0.0; NUM_BANDS],
            band_bin_ranges,
            size,
            fft,
        }
    }

    fn analyze(&mut self, samples: &[f32]) -> f32 {
        if samples.is_empty() { return self.smoothed; }

        self.sample_history.extend_from_slice(samples);
        if self.sample_history.len() > self.size {
            self.sample_history
                .drain(..self.sample_history.len() - self.size);
        }

        if self.sample_history.len() < self.size / 4 { return self.smoothed; }

        self.apply_hann_window();

        self.output_spectrum.fill(Complex32::default());
        if self
            .fft
            .process_with_scratch(
                &mut self.input_buffer,
                &mut self.output_spectrum,
                &mut self.scratch,
            )
            .is_err()
        {
            return self.smoothed;
        }

        let raw = self.spectral_centroid();
        self.smoothed += CENTROID_EMA_ALPHA * (raw - self.smoothed);

        self.update_band_levels();

        self.smoothed
    }

    fn update_band_levels(&mut self) {
        let nyquist_bin = self.size / 2;
        let denom = (self.size as f32 * self.window_power_sum).max(f32::MIN_POSITIVE);
        for (&(lo_bin, hi_bin), smoothed) in
            self.band_bin_ranges.iter().zip(&mut self.smoothed_bands)
        {
            let rms = if lo_bin < hi_bin {
                let power: f32 = self.output_spectrum[lo_bin..hi_bin]
                    .iter()
                    .enumerate()
                    .map(|(offset, c)| {
                        let bin = lo_bin + offset;
                        let one_sided = if bin == 0 || bin == nyquist_bin { 1.0 } else { 2.0 };
                        one_sided * c.norm_sqr()
                    })
                    .sum();
                (power / denom).sqrt()
            } else {
                0.0
            };
            *smoothed += BAND_EMA_ALPHA * (rms - *smoothed);
        }
    }

    fn apply_hann_window(&mut self) {
        let n = self.sample_history.len().min(self.size);
        self.input_buffer[n..].fill(0.0);
        self.input_buffer[..n].copy_from_slice(&self.sample_history[..n]);
        let window = window_coefficients(WindowKind::Hann, n);
        self.window_power_sum = window
            .iter()
            .map(|&weight| weight * weight)
            .sum::<f32>()
            .max(f32::MIN_POSITIVE);
        apply_window(&mut self.input_buffer[..n], &window);
    }

    fn spectral_centroid(&self) -> f32 {
        let min_bin = (MIN_FREQ_HZ / self.bin_hz).ceil() as usize;
        let max_bin = ((MAX_FREQ_HZ / self.bin_hz).floor() as usize)
            .min(self.output_spectrum.len().saturating_sub(1));

        if min_bin > max_bin { return 0.5; }

        let (weighted_sum, power_sum) = self.output_spectrum[min_bin..=max_bin]
            .iter()
            .enumerate()
            .fold((0.0_f64, 0.0_f64), |(ws, ps), (i, c)| {
                let hz = (min_bin + i) as f64 * self.bin_hz as f64;
                let power = c.norm_sqr() as f64;
                (ws + hz * power, ps + power)
            });

        if power_sum <= f64::EPSILON { return 0.5; }

        Self::hz_to_normalized((weighted_sum / power_sum) as f32)
    }

    fn hz_to_normalized(hz: f32) -> f32 {
        if !hz.is_finite() { return 0.5; }
        let lo = MIN_FREQ_HZ.ln();
        let hi = MAX_FREQ_HZ.ln();
        let range = (hi - lo).max(f32::EPSILON);
        ((hz.clamp(MIN_FREQ_HZ, MAX_FREQ_HZ).ln() - lo) / range).clamp(0.0, 1.0)
    }
}

pub struct WaveformProcessor {
    config: WaveformConfig,
    snapshot: WaveformSnapshot,
    channel_count: usize,
    samples_per_column: usize,
    min_values: Vec<f32>,
    max_values: Vec<f32>,
    frequency_values: Vec<f32>,
    band_levels: Vec<f32>,
    ring_head: usize,
    column_count: usize,
    total_columns_written: u64,
    sample_accumulators: Vec<Vec<f32>>,
    frequency_analyzers: Vec<FrequencyAnalyzer>,
    has_pending_changes: bool,
}

impl WaveformProcessor {
    pub fn new(config: WaveformConfig) -> Self {
        let normalized_config = config.normalized();
        let mut processor = Self {
            samples_per_column: normalized_config.samples_per_column(),
            frequency_analyzers: Vec::new(),
            config: normalized_config,
            snapshot: WaveformSnapshot::default(),
            channel_count: 2,
            min_values: Vec::new(),
            max_values: Vec::new(),
            frequency_values: Vec::new(),
            band_levels: Vec::new(),
            ring_head: 0,
            column_count: 0,
            total_columns_written: 0,
            sample_accumulators: Vec::new(),
            has_pending_changes: false,
        };
        processor.allocate_buffers();
        processor
    }

    pub fn config(&self) -> WaveformConfig {
        self.config
    }

    fn allocate_buffers(&mut self) {
        let capacity = self.config.max_columns * self.channel_count;
        self.min_values.resize(capacity, 0.0);
        self.max_values.resize(capacity, 0.0);
        self.frequency_values.resize(capacity, 0.0);
        self.band_levels.resize(
            self.channel_count * NUM_BANDS * self.config.max_columns,
            0.0,
        );
        self.sample_accumulators = (0..self.channel_count)
            .map(|_| Vec::with_capacity(self.samples_per_column))
            .collect();
        self.frequency_analyzers = (0..self.channel_count)
            .map(|_| FrequencyAnalyzer::new(self.config.sample_rate))
            .collect();
    }

    fn rebuild(&mut self) {
        self.samples_per_column = self.config.samples_per_column();
        self.ring_head = 0;
        self.column_count = 0;
        self.total_columns_written = 0;
        self.has_pending_changes = false;
        self.allocate_buffers();
    }

    fn flush_ready_columns(&mut self) {
        let (max_columns, sample_count) = (self.config.max_columns, self.samples_per_column);
        let floor = self.config.band_db_floor;
        let ready_columns = self
            .sample_accumulators
            .iter()
            .map(|acc| acc.len() / sample_count)
            .min()
            .unwrap_or(0);

        for column in 0..ready_columns {
            let start = column * sample_count;
            let end = start + sample_count;
            for (channel, (acc, analyzer)) in self
                .sample_accumulators
                .iter()
                .zip(&mut self.frequency_analyzers)
                .enumerate()
            {
                let samples = &acc[start..end];
                let (clamped_min, clamped_max) = sample_extrema(samples);
                let ring_index = channel * max_columns + self.ring_head;

                self.min_values[ring_index] = clamped_min;
                self.max_values[ring_index] = clamped_max;
                self.frequency_values[ring_index] = analyzer.analyze(samples);

                for (band, &level) in analyzer.smoothed_bands.iter().enumerate() {
                    let band_index = (channel * NUM_BANDS + band) * max_columns + self.ring_head;
                    let db = power_to_db(level * level, floor);
                    self.band_levels[band_index] = ((db - floor) / -floor).clamp(0.0, 1.0);
                }
            }

            self.ring_head = (self.ring_head + 1) % max_columns;
            self.column_count = (self.column_count + 1).min(max_columns);
            self.total_columns_written = self.total_columns_written.saturating_add(1);
            self.has_pending_changes = true;
        }

        if ready_columns > 0 {
            let drain = ready_columns * sample_count;
            for acc in &mut self.sample_accumulators {
                acc.drain(..drain);
            }
        }
    }

    fn ingest_samples(&mut self, samples: &[f32]) {
        for frame in samples.chunks_exact(self.channel_count) {
            for (acc, &sample) in self.sample_accumulators.iter_mut().zip(frame) {
                acc.push(sample);
            }
        }
        self.flush_ready_columns();
    }

    fn sync_ring_to_snapshot(&mut self) {
        let (channels, max_columns, visible_columns) = (
            self.channel_count,
            self.config.max_columns,
            self.column_count,
        );
        let size = visible_columns * channels;
        let band_size = channels * NUM_BANDS * visible_columns;

        self.snapshot.min_values.resize(size, 0.0);
        self.snapshot.max_values.resize(size, 0.0);
        self.snapshot.frequency_normalized.resize(size, 0.0);
        self.snapshot.band_levels.resize(band_size, 0.0);
        self.snapshot.channels = channels;
        self.snapshot.columns = visible_columns;

        if visible_columns > 0 {
            let start = if visible_columns < max_columns {
                0
            } else {
                self.ring_head
            };
            let copy = |src: &[f32], dst: &mut [f32], lanes| {
                for (src_lane, dst_lane) in src
                    .chunks_exact(max_columns)
                    .take(lanes)
                    .zip(dst.chunks_exact_mut(visible_columns))
                {
                    if start == 0 {
                        dst_lane.copy_from_slice(&src_lane[..visible_columns]);
                    } else {
                        let first = max_columns - start;
                        dst_lane[..first].copy_from_slice(&src_lane[start..]);
                        dst_lane[first..].copy_from_slice(&src_lane[..start]);
                    }
                }
            };
            copy(&self.min_values, &mut self.snapshot.min_values, channels);
            copy(&self.max_values, &mut self.snapshot.max_values, channels);
            copy(
                &self.frequency_values,
                &mut self.snapshot.frequency_normalized,
                channels,
            );
            copy(
                &self.band_levels,
                &mut self.snapshot.band_levels,
                channels * NUM_BANDS,
            );
        }

        self.snapshot.column_spacing_seconds = 1.0 / self.config.scroll_speed;
        self.has_pending_changes = false;
    }

    fn accumulator_progress(&self) -> f32 {
        self.sample_accumulators.first().map_or(0.0, |a| {
            (a.len() as f32 / self.samples_per_column.max(1) as f32).clamp(0.0, 1.0)
        })
    }

    fn sync_preview(&mut self) {
        let progress = self.accumulator_progress();
        let preview = &mut self.snapshot.preview;
        preview.progress = progress;

        if self.sample_accumulators.first().is_none_or(Vec::is_empty) {
            preview.min_values.clear();
            preview.max_values.clear();
            return;
        }

        preview.min_values.resize(self.channel_count, 0.0);
        preview.max_values.resize(self.channel_count, 0.0);

        for (channel, samples) in self.sample_accumulators.iter().enumerate() {
            let (min, max) = sample_extrema(samples);
            preview.min_values[channel] = min;
            preview.max_values[channel] = max;
        }
    }
    pub fn process_block(&mut self, block: &AudioBlock<'_>) -> Option<WaveformSnapshot> {
        if block.is_empty() { return None; }

        let (channels, sample_rate) = (block.channels, block.sample_rate);
        let rate_changed = sample_rates_differ(self.config.sample_rate, sample_rate);
        let needs_reconfigure = channels != self.channel_count || rate_changed;

        if needs_reconfigure {
            self.channel_count = channels;
            self.config.sample_rate = sample_rate;
            self.rebuild();
        }

        self.ingest_samples(block.samples);

        if self.has_pending_changes {
            self.sync_ring_to_snapshot();
        }

        self.sync_preview();
        self.snapshot.scroll_position =
            self.total_columns_written as f32 + self.accumulator_progress();

        Some(self.snapshot.clone())
    }

    pub fn update_config(&mut self, config: WaveformConfig) {
        let normalized = config.normalized();
        let rebuild = sample_rates_differ(self.config.sample_rate, normalized.sample_rate)
            || self.config.max_columns != normalized.max_columns;
        let old_scroll_speed = self.config.scroll_speed;
        let old_samples_per_column = self.samples_per_column;

        self.config = normalized;
        if rebuild {
            self.rebuild();
            return;
        }

        self.samples_per_column = self.config.samples_per_column();
        if self.samples_per_column != old_samples_per_column {
            self.flush_ready_columns();
        }
        self.has_pending_changes |= self.config.scroll_speed != old_scroll_speed;
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

    fn extract_snapshot(update: Option<WaveformSnapshot>) -> WaveformSnapshot {
        update.expect("expected snapshot")
    }

    #[test]
    fn normalization_bounds_runtime_values_without_enforcing_gui_ranges() {
        let invalid = WaveformConfig {
            sample_rate: f32::NAN,
            scroll_speed: 0.0,
            max_columns: 0,
            band_db_floor: f32::INFINITY,
        }
        .normalized();

        assert_eq!(invalid.sample_rate, DEFAULT_SAMPLE_RATE);
        assert_eq!(invalid.scroll_speed, DEFAULT_SCROLL_SPEED);
        assert_eq!(invalid.max_columns, 1);
        assert_eq!(invalid.band_db_floor, DEFAULT_BAND_DB_FLOOR);

        let too_many = WaveformConfig {
            max_columns: MAX_COLUMN_CAPACITY + 1,
            ..Default::default()
        }
        .normalized();
        assert_eq!(too_many.max_columns, MAX_COLUMN_CAPACITY);

        let mut slow = config(0.25, MAX_COLUMN_CAPACITY).normalized();
        assert_eq!(slow.scroll_speed, MIN_RUNTIME_SCROLL_SPEED);
        assert_eq!(slow.samples_per_column(), RATE as usize);

        slow.scroll_speed = MAX_SCROLL_SPEED * 2.0;
        slow.band_db_floor = MIN_BAND_DB_FLOOR * 2.0;
        let custom = slow.normalized();
        assert_eq!(custom.scroll_speed, MAX_SCROLL_SPEED * 2.0);
        assert_eq!(custom.band_db_floor, MIN_BAND_DB_FLOOR * 2.0);
    }

    fn centroid_for(frequency: f32, scroll_speed: f32) -> f32 {
        let config = config(scroll_speed, MAX_COLUMN_CAPACITY);
        let samples_per_column = config.samples_per_column();
        let mut processor = WaveformProcessor::new(config);
        let samples: Vec<f32> = (0..samples_per_column * 60)
            .map(|n| (2.0 * PI * frequency * n as f32 / RATE).sin())
            .collect();
        extract_snapshot(processor.process_block(&block(&samples, 1, RATE)))
            .frequency_normalized
            .last()
            .copied()
            .unwrap_or(0.5)
    }

    #[test]
    fn downsampling_produces_min_max_pairs() {
        let config = config(120.0, MAX_COLUMN_CAPACITY);
        let mut processor = WaveformProcessor::new(config);
        let samples: Vec<f32> = (0..processor.samples_per_column)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.25 })
            .collect();
        let snapshot = extract_snapshot(processor.process_block(&block(&samples, 1, RATE)));
        assert_eq!(snapshot.columns, 1);
        assert!((snapshot.max_values[0] - 0.5).abs() < f32::EPSILON);
        assert!((snapshot.min_values[0] + 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn extrema_ignore_non_finite_samples() {
        assert_eq!(sample_extrema(&[f32::NAN, f32::INFINITY]), (0.0, 0.0));
        assert_eq!(
            sample_extrema(&[f32::NAN, -0.5, f32::INFINITY, 0.25]),
            (-0.5, 0.25)
        );
    }

    #[test]
    fn frequency_analyzer_windows_partial_history() {
        let mut analyzer = FrequencyAnalyzer::new(RATE);
        analyzer.sample_history = vec![1.0; 5];
        analyzer.apply_hann_window();

        for (&actual, expected) in analyzer.input_buffer[..5].iter().zip([0.0, 0.5, 1.0, 0.5, 0.0]) {
            assert!((actual - expected).abs() < 1e-6);
        }
        analyzer.sample_history.truncate(1);
        analyzer.apply_hann_window();
        assert_eq!(analyzer.input_buffer[0], 1.0);
    }

    #[test]
    fn centroid_normalization_uses_declared_frequency_bounds() {
        assert!((FrequencyAnalyzer::hz_to_normalized(MIN_FREQ_HZ) - 0.0).abs() < 1e-6);
        assert!((FrequencyAnalyzer::hz_to_normalized(MAX_FREQ_HZ) - 1.0).abs() < 1e-6);
        assert!(
            (FrequencyAnalyzer::hz_to_normalized((MIN_FREQ_HZ * MAX_FREQ_HZ).sqrt()) - 0.5)
                .abs()
                < 1e-6
        );
        assert_eq!(FrequencyAnalyzer::hz_to_normalized(f32::NAN), 0.5);
    }

    #[test]
    fn band_levels_are_window_power_normalized_rms() {
        let mut analyzer = FrequencyAnalyzer::new(RATE);
        let freq = analyzer.bin_hz * 64.0;
        let samples: Vec<f32> = (0..FREQUENCY_FFT_SIZE)
            .map(|n| (2.0 * PI * freq * n as f32 / RATE).sin())
            .collect();

        for _ in 0..20 {
            analyzer.analyze(&samples);
        }

        let expected = std::f32::consts::FRAC_1_SQRT_2;
        assert!((analyzer.smoothed_bands[1] - expected).abs() < 0.02);
    }

    #[test]
    fn centroid_tracks_brightness() {
        let results: Vec<_> = [100.0, 440.0, 1000.0, 5000.0]
            .into_iter()
            .map(|frequency| (frequency, centroid_for(frequency, 200.0)))
            .collect();

        for window in results.windows(2) {
            let (low_hz, low_norm) = window[0];
            let (high_hz, high_norm) = window[1];
            assert!(
                high_norm > low_norm,
                "{high_hz:.0} Hz ({high_norm:.3}) should be > {low_hz:.0} Hz ({low_norm:.3})"
            );
        }
    }

    #[test]
    fn scroll_speed_does_not_affect_centroid() {
        let results: Vec<_> = [50.0, 100.0, 200.0, 500.0]
            .into_iter()
            .map(|scroll_speed| (scroll_speed, centroid_for(440.0, scroll_speed)))
            .collect();

        let avg: f32 = results.iter().map(|(_, n)| n).sum::<f32>() / results.len() as f32;
        for (speed, normalized) in &results {
            let dev_pct = (normalized - avg).abs() / avg * 100.0;
            assert!(
                dev_pct < 0.1,
                "scroll_speed {speed} produced {normalized:.6}, deviates {dev_pct:.3}% from avg {avg:.6}"
            );
        }
    }

    #[test]
    fn scroll_speed_update_preserves_history() {
        let config = config(100.0, 16);
        let mut processor = WaveformProcessor::new(config);
        let old_samples_per_column = config.samples_per_column();

        for value in [0.1, 0.2, 0.3, 0.4] {
            processor.process_block(&block(&vec![value; old_samples_per_column], 1, RATE));
        }
        let before = processor.snapshot.max_values.clone();

        let mut updated = processor.config();
        updated.scroll_speed = 400.0;
        processor.update_config(updated);

        assert_eq!(processor.snapshot.max_values, before);
        assert_eq!(processor.samples_per_column, updated.samples_per_column());

        let after = extract_snapshot(processor.process_block(&block(
            &vec![0.9; processor.samples_per_column],
            1,
            RATE,
        )));

        let mut expected = before;
        expected.push(0.9);
        assert_eq!(after.columns, expected.len());
        assert_eq!(after.max_values, expected);
    }

    #[test]
    fn snapshot_retains_latest_columns_after_history_exceeds_capacity() {
        let config = config(200.0, 512);
        let mut processor = WaveformProcessor::new(config);
        for batch in 0..512 + 10 {
            processor.process_block(&block(
                &vec![((batch + 1) as f32 * 0.001).min(1.0); processor.samples_per_column],
                1,
                RATE,
            ));
        }

        assert_eq!(
            processor.snapshot.columns, 512,
            "snapshot should cap at max_columns"
        );
        assert_eq!(processor.snapshot.max_values.len(), 512);
        let expected = (11..=522).map(|n| n as f32 * 0.001);
        for (idx, (actual, expected)) in processor
            .snapshot
            .max_values
            .iter()
            .zip(expected)
            .enumerate()
        {
            assert!(
                (*actual - expected).abs() < f32::EPSILON,
                "column {idx}: expected {expected}, got {actual}"
            );
        }
    }
}
