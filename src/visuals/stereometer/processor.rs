// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::dsp::{AudioBlock, AudioProcessor, Reconfigurable};
use crate::util::audio::{BAND_SPLITS_HZ, DEFAULT_SAMPLE_RATE, extend_interleaved_history};
use std::collections::VecDeque;

// Band histories store interleaved L/R only, independent of input channel count.
const BAND_CHANNELS: usize = 2;
// LR4 passbands can transiently exceed the input envelope; scale a
// tad to prevent.
const BAND_DISPLAY_GAIN: f32 = 0.8;

#[derive(Debug, Clone, Copy)]
pub struct StereometerConfig {
    pub sample_rate: f32,
    pub segment_duration: f32,
    pub target_sample_count: usize,
    pub correlation_window: f32,
    pub emit_band_points: bool,
}

impl Default for StereometerConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            segment_duration: 0.02,
            target_sample_count: 2_000,
            correlation_window: 0.05,
            emit_band_points: false,
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
    pub band_points: [Vec<(f32, f32)>; 3],
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
    band_history: [VecDeque<f32>; 3],
    history_channels: usize,
    // [left low/mid, right low/mid, left mid/high, right mid/high]
    crossovers: [LR4; 4],
    // [full, low, mid, high]
    correlators: [Correlator; 4],
}

impl StereometerProcessor {
    pub fn new(config: StereometerConfig) -> Self {
        Self {
            snapshot: StereometerSnapshot::default(),
            history: VecDeque::new(),
            band_history: Default::default(),
            history_channels: 0,
            crossovers: Self::build_crossovers(config.sample_rate),
            correlators: Self::fresh_correlators(config),
            config,
        }
    }

    fn build_crossovers(sample_rate: f32) -> [LR4; 4] {
        let [low_mid, mid_high] = BAND_SPLITS_HZ;
        [
            LR4::lowpass(sample_rate, low_mid),
            LR4::lowpass(sample_rate, low_mid),
            LR4::lowpass(sample_rate, mid_high),
            LR4::lowpass(sample_rate, mid_high),
        ]
    }

    fn fresh_correlators(config: StereometerConfig) -> [Correlator; 4] {
        let alpha = ema_alpha(config.sample_rate, config.correlation_window);
        [Correlator {
            alpha,
            ..Default::default()
        }; 4]
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

            let low_l = self.crossovers[0].process(left);
            let low_r = self.crossovers[1].process(right);
            let mid_l = self.crossovers[2].process(left - low_l);
            let mid_r = self.crossovers[3].process(right - low_r);
            let (high_l, high_r) = (left - low_l - mid_l, right - low_r - mid_r);

            self.correlators[1].update(low_l, low_r);
            self.correlators[2].update(mid_l, mid_r);
            self.correlators[3].update(high_l, high_r);

            if self.config.emit_band_points {
                self.band_history[0].extend([low_l, low_r]);
                self.band_history[1].extend([mid_l, mid_r]);
                self.band_history[2].extend([high_l, high_r]);
            }
        }

        let frames = (self.config.sample_rate * self.config.segment_duration)
            .round()
            .max(1.0) as usize;
        let capacity = frames * channel_count;

        extend_interleaved_history(&mut self.history, block.samples, capacity, channel_count);

        let band_capacity = frames * BAND_CHANNELS;
        if self.config.emit_band_points {
            for bh in &mut self.band_history {
                let drop = bh.len().saturating_sub(band_capacity);
                bh.drain(..drop);
            }
        }

        if self.history.len() < capacity {
            return None;
        }

        let target = self.config.target_sample_count.clamp(1, frames);
        {
            let data = self.history.make_contiguous();
            self.snapshot.xy_points.clear();
            self.snapshot.xy_points.reserve(target);
            for i in 0..target {
                let idx = (i * frames / target) * channel_count;
                self.snapshot.xy_points.push((data[idx], data[idx + 1]));
            }
        }

        if self.config.emit_band_points {
            for (bh, buf) in self
                .band_history
                .iter_mut()
                .zip(&mut self.snapshot.band_points)
            {
                buf.clear();
                if bh.len() < band_capacity {
                    continue;
                }
                let data = bh.make_contiguous();
                buf.reserve(target);
                for i in 0..target {
                    let idx = (i * frames / target) * BAND_CHANNELS;
                    buf.push((
                        data[idx] * BAND_DISPLAY_GAIN,
                        data[idx + 1] * BAND_DISPLAY_GAIN,
                    ));
                }
            }
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
        self.snapshot = StereometerSnapshot::default();
        self.history.clear();
        self.band_history.iter_mut().for_each(VecDeque::clear);
        self.history_channels = 0;
        self.crossovers = Self::build_crossovers(self.config.sample_rate);
        self.correlators = Self::fresh_correlators(self.config);
    }
}

impl Reconfigurable<StereometerConfig> for StereometerProcessor {
    fn update_config(&mut self, config: StereometerConfig) {
        let sample_rate_changed =
            (self.config.sample_rate - config.sample_rate).abs() > f32::EPSILON;
        let window_changed =
            (self.config.correlation_window - config.correlation_window).abs() > f32::EPSILON;
        let emit_turned_off = self.config.emit_band_points && !config.emit_band_points;
        self.config = config;

        if sample_rate_changed {
            self.reset();
        } else if window_changed {
            let alpha = ema_alpha(config.sample_rate, config.correlation_window);
            self.correlators.iter_mut().for_each(|c| c.alpha = alpha);
        }

        if emit_turned_off {
            self.band_history.iter_mut().for_each(VecDeque::clear);
            self.snapshot.band_points.iter_mut().for_each(Vec::clear);
        }
    }
}
