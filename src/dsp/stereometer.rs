// Stereometer (vectorscope & correlation meter) DSP.

use super::{AudioBlock, AudioProcessor, Reconfigurable};
use crate::util::audio::{DEFAULT_SAMPLE_RATE, extend_interleaved_history};
use std::collections::VecDeque;

const LOW_MID_HZ: f32 = 250.0;
const MID_HIGH_HZ: f32 = 4000.0;

#[derive(Debug, Clone, Copy)]
pub struct StereometerConfig {
    pub sample_rate: f32,
    pub segment_duration: f32,
    pub target_sample_count: usize,
    pub correlation_window: f32,
}

impl Default for StereometerConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            segment_duration: 0.02,
            target_sample_count: 2_000,
            correlation_window: 0.05,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BandCorrelation {
    pub low: f32,
    pub mid: f32,
    pub high: f32,
}

impl std::ops::Index<usize> for BandCorrelation {
    type Output = f32;
    fn index(&self, i: usize) -> &f32 {
        match i {
            0 => &self.low,
            1 => &self.mid,
            _ => &self.high,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct StereometerSnapshot {
    pub xy_points: Vec<(f32, f32)>,
    pub correlation: f32,
    pub band_correlation: BandCorrelation,
}

impl StereometerSnapshot {
    fn prepare_xy_points(&mut self, capacity: usize) {
        self.xy_points.clear();
        self.xy_points.reserve(capacity);
    }

    fn push_xy_point(&mut self, left: f32, right: f32) {
        self.xy_points.push((left, right));
    }
}

// Linkwitz-Riley 4th-order crossover (two cascaded 2nd-order Butterworth).
#[derive(Debug, Clone, Copy, Default)]
struct LR4 {
    feedforward: [[f32; 3]; 2],
    feedback: [[f32; 2]; 2],
    delays: [[f32; 4]; 2],
}

impl LR4 {
    fn lowpass(sample_rate: f32, freq: f32) -> Self {
        let omega = std::f32::consts::TAU * freq / sample_rate;
        let (sin_w, cos_w) = omega.sin_cos();
        let alpha = sin_w * std::f32::consts::FRAC_1_SQRT_2;
        let a0_inv = 1.0 / (1.0 + alpha);
        let gain = 1.0 - cos_w;
        let (b0, b1) = (gain * 0.5 * a0_inv, gain * a0_inv);
        Self {
            feedforward: [[b0, b1, b0]; 2],
            feedback: [[-2.0 * cos_w * a0_inv, (1.0 - alpha) * a0_inv]; 2],
            delays: [[0.0; 4]; 2],
        }
    }

    #[inline]
    fn process(&mut self, sample: f32) -> f32 {
        let mut signal = sample;
        for i in 0..2 {
            let [b0, b1, b2] = self.feedforward[i];
            let [a1, a2] = self.feedback[i];
            let [x1, x2, y1, y2] = self.delays[i];
            let y = b0 * signal + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            self.delays[i] = [signal, x1, y, y1];
            signal = y;
        }
        signal
    }
}

// EMA-based stereo correlation with continuous update.
#[derive(Debug, Clone, Copy, Default)]
struct Correlator {
    cross: f64,
    left_power: f64,
    right_power: f64,
    alpha: f64,
}

impl Correlator {
    #[inline]
    fn update(&mut self, left: f32, right: f32) {
        let (left, right) = (left as f64, right as f64);
        self.cross += self.alpha * (left * right - self.cross);
        self.left_power += self.alpha * (left * left - self.left_power);
        self.right_power += self.alpha * (right * right - self.right_power);
    }

    #[inline]
    fn value(&self) -> f32 {
        let denom = (self.left_power * self.right_power).sqrt();
        if denom < 1e-12 {
            0.0
        } else {
            (self.cross / denom).clamp(-1.0, 1.0) as f32
        }
    }
}

#[derive(Debug, Clone)]
pub struct StereometerProcessor {
    config: StereometerConfig,
    snapshot: StereometerSnapshot,
    history: VecDeque<f32>,
    history_channels: usize,
    // [left low/mid, right low/mid, left mid/high, right mid/high]
    crossovers: [LR4; 4],
    // [full, low, mid, high]
    correlators: [Correlator; 4],
}

impl StereometerProcessor {
    pub fn new(config: StereometerConfig) -> Self {
        let alpha = ema_alpha(config.sample_rate, config.correlation_window);
        Self {
            config,
            snapshot: StereometerSnapshot::default(),
            history: VecDeque::new(),
            history_channels: 0,
            crossovers: Self::build_crossovers(config.sample_rate),
            correlators: [Correlator {
                alpha,
                ..Default::default()
            }; 4],
        }
    }

    fn build_crossovers(sample_rate: f32) -> [LR4; 4] {
        [
            LR4::lowpass(sample_rate, LOW_MID_HZ),
            LR4::lowpass(sample_rate, LOW_MID_HZ),
            LR4::lowpass(sample_rate, MID_HIGH_HZ),
            LR4::lowpass(sample_rate, MID_HIGH_HZ),
        ]
    }

    pub fn config(&self) -> StereometerConfig {
        self.config
    }
}

fn ema_alpha(sample_rate: f32, window: f32) -> f64 {
    1.0 - (-1.0 / (sample_rate as f64 * window as f64).max(1.0)).exp()
}

impl AudioProcessor for StereometerProcessor {
    type Output = StereometerSnapshot;

    fn process_block(&mut self, block: &AudioBlock<'_>) -> Option<Self::Output> {
        let channel_count = block.channels.max(1);
        if block.frame_count() == 0 || channel_count < 2 {
            return None;
        }

        let sample_rate = block.sample_rate.max(1.0);
        if (self.config.sample_rate - sample_rate).abs() > f32::EPSILON {
            let mut config = self.config;
            config.sample_rate = sample_rate;
            self.update_config(config);
        }
        if self.history_channels != channel_count {
            self.history.clear();
            self.history_channels = channel_count;
        }

        for frame in block.samples.chunks_exact(channel_count) {
            let (left, right) = (frame[0], frame[1]);
            self.correlators[0].update(left, right);

            // 3-band split: low < 250Hz, mid 250-4000Hz, high > 4000Hz
            let low_l = self.crossovers[0].process(left);
            let low_r = self.crossovers[1].process(right);
            let mid_l = self.crossovers[2].process(left - low_l);
            let mid_r = self.crossovers[3].process(right - low_r);
            let (high_l, high_r) = (left - low_l - mid_l, right - low_r - mid_r);

            self.correlators[1].update(low_l, low_r);
            self.correlators[2].update(mid_l, mid_r);
            self.correlators[3].update(high_l, high_r);
        }

        // Manage history buffer for XY display
        let frames = (self.config.sample_rate * self.config.segment_duration)
            .round()
            .max(1.0) as usize;
        let capacity = frames * channel_count;

        extend_interleaved_history(&mut self.history, block.samples, capacity, channel_count);

        if self.history.len() < capacity {
            return None;
        }

        // Downsample to target point count
        let data = self.history.make_contiguous();
        let target = self.config.target_sample_count.clamp(1, frames);
        self.snapshot.prepare_xy_points(target);
        for i in 0..target {
            let idx = (i * frames / target) * channel_count;
            self.snapshot.push_xy_point(data[idx], data[idx + 1]);
        }

        self.snapshot.correlation = self.correlators[0].value();
        self.snapshot.band_correlation = BandCorrelation {
            low: self.correlators[1].value(),
            mid: self.correlators[2].value(),
            high: self.correlators[3].value(),
        };

        Some(self.snapshot.clone())
    }

    fn reset(&mut self) {
        let alpha = ema_alpha(self.config.sample_rate, self.config.correlation_window);
        self.snapshot = StereometerSnapshot::default();
        self.history.clear();
        self.history_channels = 0;
        self.crossovers = Self::build_crossovers(self.config.sample_rate);
        self.correlators = [Correlator {
            alpha,
            ..Default::default()
        }; 4];
    }
}

impl Reconfigurable<StereometerConfig> for StereometerProcessor {
    fn update_config(&mut self, config: StereometerConfig) {
        let sample_rate_changed =
            (self.config.sample_rate - config.sample_rate).abs() > f32::EPSILON;
        let window_changed =
            (self.config.correlation_window - config.correlation_window).abs() > f32::EPSILON;
        self.config = config;

        if sample_rate_changed {
            self.reset();
        } else if window_changed {
            let alpha = ema_alpha(config.sample_rate, config.correlation_window);
            self.correlators.iter_mut().for_each(|c| c.alpha = alpha);
        }
    }
}
