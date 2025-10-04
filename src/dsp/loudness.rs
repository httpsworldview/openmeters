//! Loudness-related DSP utilities (LUFS, RMS, true-peak, etc.).

use super::{AudioBlock, AudioProcessor, ProcessorUpdate, Reconfigurable};
use std::collections::VecDeque;

const MIN_MEAN_SQUARE: f64 = 1e-12;
const LOG10_FACTOR: f32 = 10.0;
const DB_FACTOR: f32 = 20.0;

fn mean_square_to_lufs(mean_square: f64, floor: f32) -> f32 {
    let lufs = (LOG10_FACTOR as f64 * mean_square.log10()) as f32;
    lufs.max(floor)
}

fn peak_to_db(peak: f32, floor: f32) -> f32 {
    if peak <= f32::EPSILON {
        floor
    } else {
        (DB_FACTOR * peak.log10()).max(floor)
    }
}

fn window_length(sample_rate: f32, window_secs: f32) -> usize {
    (sample_rate * window_secs).max(1.0) as usize
}

#[derive(Debug, Clone)]
struct RollingMeanSquare {
    samples: VecDeque<f64>,
    capacity: usize,
    sum: f64,
}

impl RollingMeanSquare {
    fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "rolling window capacity must be positive");
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity,
            sum: 0.0,
        }
    }

    fn push(&mut self, value: f64) {
        if self.samples.len() == self.capacity {
            if let Some(oldest) = self.samples.pop_front() {
                self.sum -= oldest;
            }
        }

        self.samples.push_back(value);
        self.sum += value;
    }

    fn mean(&self) -> f64 {
        if self.samples.is_empty() {
            0.0
        } else {
            self.sum / self.samples.len() as f64
        }
    }

    fn reset(&mut self) {
        self.samples.clear();
        self.sum = 0.0;
    }
}

/// Rolling loudness statistics produced by the loudness processor.
#[derive(Debug, Clone, Default)]
pub struct LoudnessSnapshot {
    pub momentary_lufs: Vec<f32>,
    pub true_peak_db: Vec<f32>,
}

impl LoudnessSnapshot {
    fn with_channels(channels: usize, floor_lufs: f32) -> Self {
        Self {
            momentary_lufs: vec![floor_lufs; channels],
            true_peak_db: vec![floor_lufs; channels],
        }
    }
}

/// Configuration options for the loudness processor.
#[derive(Debug, Clone, Copy)]
pub struct LoudnessConfig {
    pub sample_rate: f32,
    /// Window size in seconds for the momentary measurement (default 0.4s).
    pub momentary_window: f32,
    /// Floor applied to LUFS/peak values to avoid `-inf`.
    pub floor_lufs: f32,
}

impl Default for LoudnessConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            momentary_window: 0.4,
            floor_lufs: -60.0,
        }
    }
}

/// Loudness processor that tracks per-channel LUFS and true-peak values.
#[derive(Debug, Clone)]
pub struct LoudnessProcessor {
    config: LoudnessConfig,
    windows: Vec<RollingMeanSquare>,
    peaks_linear: Vec<f32>,
    snapshot: LoudnessSnapshot,
}

impl LoudnessProcessor {
    pub fn new(config: LoudnessConfig) -> Self {
        Self {
            windows: Vec::new(),
            peaks_linear: Vec::new(),
            snapshot: LoudnessSnapshot::default(),
            config,
        }
    }

    pub fn config(&self) -> LoudnessConfig {
        self.config
    }

    pub fn snapshot(&self) -> &LoudnessSnapshot {
        &self.snapshot
    }

    fn ensure_state(&mut self, requested_channels: usize, sample_rate: f32) {
        let channels = requested_channels.max(1);

        let mut needs_rebuild = self.windows.len() != channels;

        if sample_rate.is_finite() && sample_rate > 0.0 {
            if (self.config.sample_rate - sample_rate).abs() > f32::EPSILON {
                self.config.sample_rate = sample_rate;
                needs_rebuild = true;
            }
        }

        if needs_rebuild {
            self.rebuild_state(channels);
        }
    }

    fn rebuild_state(&mut self, channels: usize) {
        let capacity = window_length(self.config.sample_rate, self.config.momentary_window);
        self.windows = (0..channels)
            .map(|_| RollingMeanSquare::new(capacity))
            .collect();
        self.peaks_linear = vec![0.0; channels];
        self.snapshot = LoudnessSnapshot::with_channels(channels, self.config.floor_lufs);
    }
}

impl AudioProcessor for LoudnessProcessor {
    type Output = LoudnessSnapshot;

    fn process_block(&mut self, block: &AudioBlock<'_>) -> ProcessorUpdate<Self::Output> {
        if block.channels == 0 || block.frame_count() == 0 {
            return ProcessorUpdate::None;
        }

        let channels = block.channels;
        self.ensure_state(channels, block.sample_rate);

        if self.windows.is_empty() {
            return ProcessorUpdate::None;
        }

        for peak in &mut self.peaks_linear {
            *peak = 0.0;
        }

        let mut frames = block.samples.chunks_exact(channels);
        for frame in frames.by_ref() {
            for (channel, &sample) in frame.iter().enumerate() {
                let linear = sample as f64;
                self.windows[channel].push(linear * linear);
                self.peaks_linear[channel] = self.peaks_linear[channel].max(sample.abs());
            }
        }

        // Ignore any remainder that doesn't form a full frame.
        let _ = frames.remainder();

        for (channel, window) in self.windows.iter().enumerate() {
            let mean_square = window.mean().max(MIN_MEAN_SQUARE);
            self.snapshot.momentary_lufs[channel] =
                mean_square_to_lufs(mean_square, self.config.floor_lufs);

            let peak_db = peak_to_db(self.peaks_linear[channel], self.config.floor_lufs);
            self.snapshot.true_peak_db[channel] = peak_db;
        }

        ProcessorUpdate::Snapshot(self.snapshot.clone())
    }

    fn reset(&mut self) {
        for window in &mut self.windows {
            window.reset();
        }
        for peak in &mut self.peaks_linear {
            *peak = 0.0;
        }

        let channels = self.windows.len();
        self.snapshot = if channels > 0 {
            LoudnessSnapshot::with_channels(channels, self.config.floor_lufs)
        } else {
            LoudnessSnapshot::default()
        };
    }
}

impl Reconfigurable<LoudnessConfig> for LoudnessProcessor {
    fn update_config(&mut self, config: LoudnessConfig) {
        self.config = config;
        let channels = self.windows.len();
        if channels > 0 {
            self.rebuild_state(channels);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn rolling_mean_square_tracks_average() {
        let mut window = RollingMeanSquare::new(4);
        window.push(1.0);
        window.push(9.0);
        assert!((window.mean() - 5.0).abs() < f64::EPSILON);

        window.push(16.0);
        window.push(25.0);
        window.push(36.0);
        // Now should hold 9,16,25,36
        assert!((window.mean() - 21.5).abs() < f64::EPSILON);
    }

    #[test]
    fn processor_estimates_rms_loudness() {
        let mut processor = LoudnessProcessor::new(LoudnessConfig::default());
        let samples = vec![0.5; 48_000 * 2];
        let block = AudioBlock::new(&samples, 2, 48_000.0, Instant::now());
        let snapshot = match processor.process_block(&block) {
            ProcessorUpdate::Snapshot(snapshot) => snapshot,
            ProcessorUpdate::None => panic!("expected snapshot"),
        };
        // 0.5 RMS -> -6 dBFS
        for value in snapshot.momentary_lufs {
            assert!((value + 6.0).abs() < 0.5);
        }
    }

    #[test]
    fn processor_tracks_peak() {
        let mut processor = LoudnessProcessor::new(LoudnessConfig::default());
        let mut samples = vec![0.0; 1024 * 2];
        samples[0] = 0.9;
        let block = AudioBlock::new(&samples, 2, 48_000.0, Instant::now());
        let snapshot = match processor.process_block(&block) {
            ProcessorUpdate::Snapshot(snapshot) => snapshot,
            ProcessorUpdate::None => panic!("expected snapshot"),
        };
        assert!(snapshot.true_peak_db[0] > -1.0);
    }
}
