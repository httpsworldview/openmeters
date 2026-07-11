// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::dsp::{
    AudioBlock, CrossoverFilter, FilterKind, LinkwitzRiley, ThreeBand,
};
use crate::util::audio::{
    BAND_SPLITS_HZ, DEFAULT_SAMPLE_RATE, extend_interleaved_history, flush_denormal_f64,
};
use std::collections::VecDeque;

const BAND_CHANNELS: usize = 2;
const BAND_DISPLAY_GAIN: f32 = 0.8;

crate::macros::default_struct! {
    #[derive(Debug, Clone, Copy)]
    pub struct StereometerConfig {
        pub sample_rate: f32 = DEFAULT_SAMPLE_RATE,
        pub segment_duration: f32 = 0.02,
        pub target_sample_count: usize = 2_000,
        pub correlation_window: f32 = 0.05,
        pub analyze_bands: bool = false,
        pub emit_band_points: bool = false,
    }
}

impl StereometerConfig {
    fn needs_band_analysis(&self) -> bool {
        self.analyze_bands || self.emit_band_points
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BandCorrelation {
    pub low: f32,
    pub mid: f32,
    pub high: f32,
}

#[derive(Debug, Clone, Default)]
pub struct StereometerSnapshot {
    pub xy_points: Vec<(f32, f32)>,
    pub correlation: f32,
    pub band_correlation: BandCorrelation,
    pub band_points: [Vec<(f32, f32)>; 3],
}

#[derive(Debug, Clone, Copy)]
struct StereoFilter {
    left: LinkwitzRiley,
    right: LinkwitzRiley,
}

impl CrossoverFilter for StereoFilter {
    type Sample = (f32, f32);

    fn new(kind: FilterKind, sample_rate: f32, frequency: f32) -> Self {
        let filter = LinkwitzRiley::new(kind, sample_rate, frequency);
        Self { left: filter, right: filter }
    }

    fn process(&mut self, (left, right): Self::Sample) -> Self::Sample {
        (self.left.process(left), self.right.process(right))
    }

    fn flush_denormals(&mut self) {
        self.left.flush_denormals();
        self.right.flush_denormals();
    }
}

type BandSplitter = ThreeBand<StereoFilter>;

#[derive(Debug, Clone, Copy)]
struct Correlator {
    cross: f64,
    left_power: f64,
    right_power: f64,
    alpha: f64,
}

impl Correlator {
    fn new(alpha: f64) -> Self {
        Self {
            cross: 0.0,
            left_power: 0.0,
            right_power: 0.0,
            alpha,
        }
    }

    fn update(&mut self, left: f32, right: f32) {
        let (left, right) = (left as f64, right as f64);
        self.cross += self.alpha * (left * right - self.cross);
        self.left_power += self.alpha * (left * left - self.left_power);
        self.right_power += self.alpha * (right * right - self.right_power);
    }

    fn value(&self) -> f32 {
        let denom = (self.left_power * self.right_power).sqrt();
        if denom <= 1e-12 {
            return 0.0;
        }
        let value = self.cross / denom;
        if value.is_finite() {
            value.clamp(-1.0, 1.0) as f32
        } else {
            0.0
        }
    }

    fn flush_denormals(&mut self) {
        [&mut self.cross, &mut self.left_power, &mut self.right_power]
            .into_iter()
            .for_each(flush_denormal_f64);
    }
}

#[derive(Debug, Clone, Copy)]
struct Correlators {
    full: Correlator,
    bands: [Correlator; 3],
}

impl Correlators {
    fn new(config: StereometerConfig) -> Self {
        let alpha = ema_alpha(config.sample_rate, config.correlation_window);
        Self {
            full: Correlator::new(alpha),
            bands: [Correlator::new(alpha); 3],
        }
    }

    fn set_alpha(&mut self, alpha: f64) {
        self.full.alpha = alpha;
        for band in &mut self.bands {
            band.alpha = alpha;
        }
    }

    fn reset_bands(&mut self, alpha: f64) {
        self.bands = [Correlator::new(alpha); 3];
    }

    fn band_correlation(&self) -> BandCorrelation {
        let [low, mid, high] = self.bands.each_ref().map(Correlator::value);
        BandCorrelation { low, mid, high }
    }
}

#[derive(Debug)]
pub struct StereometerProcessor {
    config: StereometerConfig,
    snapshot: StereometerSnapshot,
    history: VecDeque<f32>,
    band_history: [VecDeque<f32>; 3],
    history_channels: usize,
    band_splitter: BandSplitter,
    correlators: Correlators,
}

impl StereometerProcessor {
    pub fn new(config: StereometerConfig) -> Self {
        Self {
            snapshot: StereometerSnapshot::default(),
            history: VecDeque::new(),
            band_history: Default::default(),
            history_channels: 0,
            band_splitter: BandSplitter::cascaded(config.sample_rate, BAND_SPLITS_HZ),
            correlators: Correlators::new(config),
            config,
        }
    }

    pub fn config(&self) -> StereometerConfig {
        self.config
    }

    pub fn process_block(&mut self, block: &AudioBlock<'_>) -> Option<StereometerSnapshot> {
        let channel_count = block.channels;
        if block.is_empty() || channel_count < 2 { return None; }

        let sample_rate = block.sample_rate;
        if self.config.sample_rate != sample_rate {
            let mut config = self.config;
            config.sample_rate = sample_rate;
            self.update_config(config);
        }
        if self.history_channels != channel_count {
            self.history.clear();
            self.history_channels = channel_count;
        }

        let analyze_bands = self.config.needs_band_analysis();
        for frame in block.samples.chunks_exact(channel_count) {
            let (left, right) = (frame[0], frame[1]);
            self.correlators.full.update(left, right);

            if analyze_bands {
                let bands = self.band_splitter.process((left, right));
                for ((correlator, history), (left, right)) in self
                    .correlators
                    .bands
                    .iter_mut()
                    .zip(&mut self.band_history)
                    .zip(bands)
                {
                    correlator.update(left, right);
                    if self.config.emit_band_points {
                        history.extend([left, right]);
                    }
                }
            }
        }
        self.correlators.full.flush_denormals();
        if analyze_bands {
            self.correlators
                .bands
                .iter_mut()
                .for_each(Correlator::flush_denormals);
            self.band_splitter.flush_denormals();
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

        if self.history.len() < capacity { return None; }

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
                    buf.push((data[idx] * BAND_DISPLAY_GAIN, data[idx + 1] * BAND_DISPLAY_GAIN));
                }
            }
        }

        self.snapshot.correlation = self.correlators.full.value();
        self.snapshot.band_correlation = if analyze_bands {
            self.correlators.band_correlation()
        } else {
            BandCorrelation::default()
        };

        Some(self.snapshot.clone())
    }

    fn reset(&mut self) {
        *self = Self::new(self.config);
    }
    pub fn update_config(&mut self, config: StereometerConfig) {
        let sample_rate_changed = self.config.sample_rate != config.sample_rate;
        let window_changed =
            (self.config.correlation_window - config.correlation_window).abs() > f32::EPSILON;
        let band_analysis_changed = self.config.needs_band_analysis() != config.needs_band_analysis();
        self.config = config;

        if sample_rate_changed {
            self.reset();
        } else {
            let alpha = ema_alpha(config.sample_rate, config.correlation_window);
            if window_changed {
                self.correlators.set_alpha(alpha);
            }
            if band_analysis_changed {
                self.band_splitter = BandSplitter::cascaded(config.sample_rate, BAND_SPLITS_HZ);
                self.correlators.reset_bands(alpha);
                self.snapshot.band_correlation = BandCorrelation::default();
            }
        }

        if !config.emit_band_points {
            self.band_history = Default::default();
            self.snapshot.band_points = Default::default();
        }
    }
}

fn ema_alpha(sample_rate: f32, window: f32) -> f64 {
    1.0 - (-1.0 / (sample_rate as f64 * window as f64).max(1.0)).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn correlation(pairs: &[(f32, f32)]) -> f32 {
        let mut meter = Correlator::new(0.5);
        for &(left, right) in pairs {
            meter.update(left, right);
        }
        meter.value()
    }

    fn assert_close(a: f32, b: f32) {
        assert!((a - b).abs() <= 1e-6, "{a} != {b}");
    }

    #[test]
    fn correlator_matches_reference_points() {
        assert_close(correlation(&[(1.0, 1.0), (-1.0, -1.0)]), 1.0);
        assert_close(correlation(&[(1.0, -1.0), (-1.0, 1.0)]), -1.0);
        assert_close(correlation(&[(1.0, 0.25), (-1.0, -0.25)]), 1.0);
        assert_close(
            correlation(&[(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)]),
            0.0,
        );
        assert_close(correlation(&[(0.0, 0.0)]), 0.0);
    }
}
