//! Scrolling waveform with peak frequency-based coloring.

use super::spectrogram::WindowKind;
use super::{AudioBlock, AudioProcessor, ProcessorUpdate, Reconfigurable};
use crate::util::audio::DEFAULT_SAMPLE_RATE;
use realfft::{RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex32;
use std::sync::Arc;

pub const MIN_SCROLL_SPEED: f32 = 10.0;
pub const MAX_SCROLL_SPEED: f32 = 1000.0;
pub const MIN_COLUMN_CAPACITY: usize = 512;
pub const MAX_COLUMN_CAPACITY: usize = 16_384;
pub const DEFAULT_COLUMN_CAPACITY: usize = 4_096;

// fixed because scroll speed alters timebase
// which affects sample accumulation, etc.
const FREQUENCY_FFT_SIZE: usize = 2048;

// Frequency range for mapping to log
// higher than 5kHz is saturated to the top end
// of the color scale
const MIN_FREQ_HZ: f32 = 20.0;
const MAX_FREQ_HZ: f32 = 5_000.0;

// max allowed sweep rate of normalized frequency
// lower = longer smoothing
// helps avoid rapid color changes, but might be a hack
// as I'm not sure what the best approach is here
const MAX_SLEW_RATE: f32 = 0.01;

#[derive(Debug, Clone, Copy)]
pub struct WaveformConfig {
    pub sample_rate: f32,
    pub scroll_speed: f32,
    pub max_columns: usize,
}

impl Default for WaveformConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            scroll_speed: 300.0,
            max_columns: DEFAULT_COLUMN_CAPACITY,
        }
    }
}

impl WaveformConfig {
    fn normalized(mut self) -> Self {
        self.sample_rate = self.sample_rate.max(1.0);
        self.scroll_speed = self.scroll_speed.clamp(MIN_SCROLL_SPEED, MAX_SCROLL_SPEED);
        self.max_columns = self
            .max_columns
            .clamp(MIN_COLUMN_CAPACITY, MAX_COLUMN_CAPACITY);
        self
    }
    fn samples_per_column(&self) -> usize {
        (self.sample_rate / self.scroll_speed).round() as usize
    }
}

#[derive(Debug, Clone, Default)]
pub struct WaveformPreview {
    pub progress: f32,
    pub min_values: Vec<f32>,
    pub max_values: Vec<f32>,
}

impl WaveformPreview {
    fn clear(&mut self) {
        self.min_values.clear();
        self.max_values.clear();
    }

    fn resize(&mut self, channel_count: usize) {
        self.min_values.resize(channel_count, 0.0);
        self.max_values.resize(channel_count, 0.0);
    }

    fn set_channel(&mut self, channel: usize, min: f32, max: f32) {
        self.min_values[channel] = min;
        self.max_values[channel] = max;
    }
}

#[derive(Debug, Clone, Default)]
pub struct WaveformSnapshot {
    pub channels: usize,
    pub columns: usize,
    pub min_values: Vec<f32>,
    pub max_values: Vec<f32>,
    pub frequency_normalized: Vec<f32>,
    pub column_spacing_seconds: f32,
    pub scroll_position: f32,
    pub preview: WaveformPreview,
}

/// Converts sentinel extrema values to zero for display.
#[inline]
fn clamp_extrema(min: f32, max: f32) -> (f32, f32) {
    (
        if min == f32::MAX { 0.0 } else { min },
        if max == f32::MIN { 0.0 } else { max },
    )
}

#[derive(Clone)]
struct FrequencyAnalyzer {
    fft: Arc<dyn RealToComplex<f32>>,
    size: usize,
    input_buffer: Vec<f32>,
    output_spectrum: Vec<Complex32>,
    scratch: Vec<Complex32>,
    sample_history: Vec<f32>,
    bin_hz: f32,
    smoothed_frequency: f32,
}

impl std::fmt::Debug for FrequencyAnalyzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrequencyAnalyzer")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl FrequencyAnalyzer {
    fn new(sample_rate: f32) -> Self {
        let size = FREQUENCY_FFT_SIZE;
        let fft = RealFftPlanner::new().plan_fft_forward(size);
        Self {
            scratch: vec![Complex32::default(); fft.get_scratch_len()],
            input_buffer: vec![0.0; size],
            output_spectrum: vec![Complex32::default(); size / 2 + 1],
            sample_history: Vec::with_capacity(size),
            bin_hz: sample_rate / size as f32,
            smoothed_frequency: 0.5,
            size,
            fft,
        }
    }

    fn reconfigure(&mut self, sample_rate: f32) {
        self.bin_hz = sample_rate / self.size as f32;
        self.smoothed_frequency = 0.5;
        self.sample_history.clear();
    }

    fn analyze(&mut self, samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return self.smoothed_frequency;
        }

        self.sample_history.extend_from_slice(samples);
        if self.sample_history.len() > self.size {
            self.sample_history
                .drain(..self.sample_history.len() - self.size);
        }

        if self.sample_history.len() < self.size / 4 {
            return self.smoothed_frequency;
        }

        self.apply_hann_window();

        if self.compute_fft().is_err() {
            return self.smoothed_frequency;
        }

        let raw = self.find_peak_frequency();
        let delta = (raw - self.smoothed_frequency).clamp(-MAX_SLEW_RATE, MAX_SLEW_RATE);
        self.smoothed_frequency += delta;
        self.smoothed_frequency
    }

    fn apply_hann_window(&mut self) {
        self.input_buffer.fill(0.0);
        let n = self.sample_history.len().min(self.size);
        let window = WindowKind::Hann.coefficients(n);
        for (i, (&sample, &w)) in self.sample_history.iter().zip(window.iter()).enumerate() {
            self.input_buffer[i] = sample * w;
        }
    }

    fn compute_fft(&mut self) -> Result<(), ()> {
        self.output_spectrum.fill(Complex32::default());
        self.fft
            .process_with_scratch(
                &mut self.input_buffer,
                &mut self.output_spectrum,
                &mut self.scratch,
            )
            .map_err(|_| ())
    }

    /// Finds the dominant frequency bin and returns a normalized [0,1] value.
    fn find_peak_frequency(&self) -> f32 {
        let min_bin = (MIN_FREQ_HZ / self.bin_hz).ceil() as usize;
        let max_bin =
            ((MAX_FREQ_HZ / self.bin_hz).floor() as usize).min(self.output_spectrum.len());

        if min_bin >= max_bin {
            return 0.5;
        }

        let (peak_bin, peak_mag) = self.output_spectrum[min_bin..max_bin]
            .iter()
            .enumerate()
            .map(|(i, c)| (min_bin + i, c.norm()))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or((min_bin, 0.0));

        if peak_mag <= f32::EPSILON {
            return 0.5;
        }

        Self::hz_to_normalized(self.interpolate_peak(peak_bin))
    }

    fn interpolate_peak(&self, bin: usize) -> f32 {
        let base_hz = bin as f32 * self.bin_hz;
        if bin == 0 || bin >= self.output_spectrum.len() - 1 {
            return base_hz;
        }

        let (y0, y1, y2) = (
            self.output_spectrum[bin - 1].norm(),
            self.output_spectrum[bin].norm(),
            self.output_spectrum[bin + 1].norm(),
        );
        let denom = y0 - 2.0 * y1 + y2;
        if denom.abs() < f32::EPSILON {
            return base_hz;
        }

        let offset = (0.5 * (y0 - y2) / denom).clamp(-0.5, 0.5);
        (bin as f32 + offset) * self.bin_hz
    }

    /// Maps Hz to [0,1] in log
    fn hz_to_normalized(hz: f32) -> f32 {
        // no ln() in `const` contexts cause of floating points :[
        const LOG_MIN: f32 = 2.995_732_3; // 20.0_f32.ln()
        const LOG_RANGE: f32 = 5.526_072_4; // (5000.0_f32.ln() - LOG_MIN)
        ((hz.max(MIN_FREQ_HZ).ln() - LOG_MIN) / LOG_RANGE).clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone)]
pub struct WaveformProcessor {
    config: WaveformConfig,
    snapshot: WaveformSnapshot,
    channel_count: usize,
    samples_per_column: usize,
    min_values: Vec<f32>,
    max_values: Vec<f32>,
    frequency_values: Vec<f32>,
    ring_head: usize,
    column_count: usize,
    total_columns_written: u64,
    sample_accumulators: Vec<Vec<f32>>,
    accumulator_min: Vec<f32>,
    accumulator_max: Vec<f32>,
    frequency_analyzer: FrequencyAnalyzer,
    has_pending_changes: bool,
}

impl WaveformProcessor {
    pub fn new(config: WaveformConfig) -> Self {
        let normalized_config = config.normalized();
        let mut processor = Self {
            samples_per_column: normalized_config.samples_per_column(),
            frequency_analyzer: FrequencyAnalyzer::new(normalized_config.sample_rate),
            config: normalized_config,
            snapshot: WaveformSnapshot::default(),
            channel_count: 2,
            min_values: Vec::new(),
            max_values: Vec::new(),
            frequency_values: Vec::new(),
            ring_head: 0,
            column_count: 0,
            total_columns_written: 0,
            sample_accumulators: Vec::new(),
            accumulator_min: Vec::new(),
            accumulator_max: Vec::new(),
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
        self.sample_accumulators = (0..self.channel_count)
            .map(|_| Vec::with_capacity(self.samples_per_column))
            .collect();
        self.accumulator_min = vec![f32::MAX; self.channel_count];
        self.accumulator_max = vec![f32::MIN; self.channel_count];
    }

    fn rebuild(&mut self) {
        self.samples_per_column = self.config.samples_per_column();
        self.frequency_analyzer.reconfigure(self.config.sample_rate);
        self.ring_head = 0;
        self.column_count = 0;
        self.total_columns_written = 0;
        self.has_pending_changes = false;
        self.allocate_buffers();
    }

    fn flush_accumulated_samples(&mut self) {
        let max_columns = self.config.max_columns;

        for channel in 0..self.channel_count {
            if self.sample_accumulators[channel].is_empty() {
                continue;
            }

            let (clamped_min, clamped_max) =
                clamp_extrema(self.accumulator_min[channel], self.accumulator_max[channel]);
            let ring_index = channel * max_columns + self.ring_head;

            self.min_values[ring_index] = clamped_min;
            self.max_values[ring_index] = clamped_max;
            self.frequency_values[ring_index] = self
                .frequency_analyzer
                .analyze(&self.sample_accumulators[channel]);
        }

        // Advance ring buffer
        self.ring_head = (self.ring_head + 1) % max_columns;
        self.column_count = (self.column_count + 1).min(max_columns);
        self.total_columns_written = self.total_columns_written.saturating_add(1);
        self.has_pending_changes = true;

        // Reset accumulators
        for acc in &mut self.sample_accumulators {
            acc.clear();
        }
        self.accumulator_min.fill(f32::MAX);
        self.accumulator_max.fill(f32::MIN);
    }

    fn ingest_samples(&mut self, samples: &[f32]) {
        for frame in samples.chunks_exact(self.channel_count) {
            for (channel, &sample) in frame.iter().enumerate() {
                self.accumulator_min[channel] = self.accumulator_min[channel].min(sample);
                self.accumulator_max[channel] = self.accumulator_max[channel].max(sample);
                self.sample_accumulators[channel].push(sample);
            }

            if self.sample_accumulators[0].len() >= self.samples_per_column {
                self.flush_accumulated_samples();
            }
        }
    }

    fn sync_ring_to_snapshot(&mut self) {
        let (channels, max_columns, visible_columns) = (
            self.channel_count,
            self.config.max_columns,
            self.column_count,
        );
        let size = visible_columns * channels;

        self.snapshot.min_values.resize(size, 0.0);
        self.snapshot.max_values.resize(size, 0.0);
        self.snapshot.frequency_normalized.resize(size, 0.0);
        self.snapshot.channels = channels;
        self.snapshot.columns = visible_columns;

        if visible_columns > 0 {
            let start = if self.column_count < max_columns {
                0
            } else {
                self.ring_head
            };
            for channel in 0..channels {
                for column in 0..visible_columns {
                    let src = channel * max_columns + (start + column) % max_columns;
                    let dst = channel * visible_columns + column;
                    self.snapshot.min_values[dst] = self.min_values[src];
                    self.snapshot.max_values[dst] = self.max_values[src];
                    self.snapshot.frequency_normalized[dst] = self.frequency_values[src];
                }
            }
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
        self.snapshot.preview.progress = progress;

        let has_data = self
            .sample_accumulators
            .first()
            .is_some_and(|a| !a.is_empty());
        if !has_data {
            self.snapshot.preview.clear();
            return;
        }

        self.snapshot.preview.resize(self.channel_count);

        for channel in 0..self.channel_count {
            let (min, max) =
                clamp_extrema(self.accumulator_min[channel], self.accumulator_max[channel]);
            self.snapshot.preview.set_channel(channel, min, max);
        }
    }
}

impl AudioProcessor for WaveformProcessor {
    type Output = WaveformSnapshot;

    fn process_block(&mut self, block: &AudioBlock<'_>) -> ProcessorUpdate<Self::Output> {
        if block.frame_count() == 0 {
            return ProcessorUpdate::None;
        }

        let (channels, sample_rate) = (block.channels.max(1), block.sample_rate.max(1.0));
        let needs_reconfigure = channels != self.channel_count
            || (self.config.sample_rate - sample_rate).abs() > f32::EPSILON;

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

        ProcessorUpdate::Snapshot(self.snapshot.clone())
    }

    fn reset(&mut self) {
        self.snapshot = WaveformSnapshot::default();
        self.rebuild();
    }
}

impl Reconfigurable<WaveformConfig> for WaveformProcessor {
    fn update_config(&mut self, config: WaveformConfig) {
        let normalized = config.normalized();
        let changed = self.config.sample_rate != normalized.sample_rate
            || self.config.scroll_speed != normalized.scroll_speed
            || self.config.max_columns != normalized.max_columns;

        if changed {
            self.config = normalized;
            self.rebuild();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;
    use std::time::Instant;

    fn block(samples: &[f32], channels: usize, sample_rate: f32) -> AudioBlock<'_> {
        AudioBlock::new(samples, channels, sample_rate, Instant::now())
    }

    fn extract_snapshot(update: ProcessorUpdate<WaveformSnapshot>) -> WaveformSnapshot {
        match update {
            ProcessorUpdate::Snapshot(snapshot) => snapshot,
            _ => panic!("expected snapshot"),
        }
    }

    #[test]
    fn downsampling_produces_min_max_pairs() {
        let config = WaveformConfig {
            sample_rate: 48_000.0,
            scroll_speed: 120.0,
            ..Default::default()
        };
        let mut processor = WaveformProcessor::new(config);
        let samples: Vec<f32> = (0..processor.samples_per_column)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.25 })
            .collect();
        let snapshot = extract_snapshot(processor.process_block(&block(&samples, 1, 48_000.0)));
        assert_eq!(snapshot.columns, 1);
        assert!((snapshot.max_values[0] - 0.5).abs() < 1e-3);
        assert!((snapshot.min_values[0] + 0.25).abs() < 1e-3);
    }

    #[test]
    fn peak_frequency_tracks_fundamental() {
        let config = WaveformConfig {
            sample_rate: 48_000.0,
            scroll_speed: 200.0,
            ..Default::default()
        };
        let samples_per_column = config.samples_per_column();

        // higher frequency -> higher normalized value
        let mut results = Vec::new();
        for &frequency in &[100.0, 440.0, 1000.0, 5000.0] {
            let mut processor = WaveformProcessor::new(config);
            let samples: Vec<f32> = (0..samples_per_column * 60)
                .map(|n| (2.0 * PI * frequency * n as f32 / 48_000.0).sin())
                .collect();
            let normalized =
                extract_snapshot(processor.process_block(&block(&samples, 1, 48_000.0)))
                    .frequency_normalized
                    .last()
                    .copied()
                    .unwrap_or(0.5);
            results.push((frequency, normalized));
        }

        // higher frequency -> higher normalized value
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
    fn scroll_speed_does_not_affect_frequency_detection() {
        // Test that the same frequency produces consistent results across scroll speeds
        let frequency = 440.0;
        let mut results = Vec::new();

        for &scroll_speed in &[50.0, 100.0, 200.0, 500.0] {
            let config = WaveformConfig {
                sample_rate: 48_000.0,
                scroll_speed,
                ..Default::default()
            };
            let samples_per_column = config.samples_per_column();
            let mut processor = WaveformProcessor::new(config);
            let samples: Vec<f32> = (0..samples_per_column * 60)
                .map(|n| (2.0 * PI * frequency * n as f32 / 48_000.0).sin())
                .collect();
            let normalized =
                extract_snapshot(processor.process_block(&block(&samples, 1, 48_000.0)))
                    .frequency_normalized
                    .last()
                    .copied()
                    .unwrap_or(0.5);
            results.push((scroll_speed, normalized));
        }

        // All results should be within 1% of each other
        let avg: f32 = results.iter().map(|(_, n)| n).sum::<f32>() / results.len() as f32;
        for (speed, normalized) in &results {
            let deviation = (normalized - avg).abs() / avg;
            assert!(
                deviation < 0.01,
                "scroll_speed {speed} produced {normalized:.3}, deviates {:.1}% from avg {avg:.3}",
                deviation * 100.0
            );
        }
    }

    #[test]
    fn ring_buffer_wraps_correctly() {
        let config = WaveformConfig {
            sample_rate: 48_000.0,
            scroll_speed: 200.0,
            max_columns: MIN_COLUMN_CAPACITY,
        };
        let mut processor = WaveformProcessor::new(config);
        for batch in 0..MIN_COLUMN_CAPACITY + 10 {
            processor.process_block(&block(
                &vec![((batch + 1) as f32 * 0.001).min(1.0); processor.samples_per_column],
                1,
                48_000.0,
            ));
        }
        assert_eq!(
            processor.snapshot.columns, MIN_COLUMN_CAPACITY,
            "ring buffer should cap at max_columns"
        );
    }
}
