// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::dsp::{AudioBlock, Biquad, Cascade, ThreeBand};
use crate::util::audio::{BAND_SPLITS_HZ, DEFAULT_SAMPLE_RATE, flush_denormal_f64};
use std::{collections::VecDeque, sync::Arc};

const BAND_DISPLAY_GAIN: f32 = 0.8;
pub(super) const BAND_COUNT: usize = BAND_SPLITS_HZ.len() + 1;

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

pub struct StereometerSnapshot {
    pub points: [Arc<[(f32, f32)]>; BAND_COUNT + 1],
    pub correlations: [f32; BAND_COUNT + 1],
}

fn snapshot_points(points: &[(f32, f32)]) -> Arc<[(f32, f32)]> {
    if points.is_empty() { Arc::default() } else { Arc::from(points) }
}

type BandSplitter = ThreeBand<Cascade<[Biquad; 2], 2>, true>;

#[derive(Debug, Clone, Copy, Default)]
struct Correlator {
    moments: [f64; 3],
}

impl Correlator {
    fn update(&mut self, left: f32, right: f32, alpha: f64) {
        let (left, right) = (left as f64, right as f64);
        let [cross, left_power, right_power] = &mut self.moments;
        *cross += alpha * (left * right - *cross);
        *left_power += alpha * (left * left - *left_power);
        *right_power += alpha * (right * right - *right_power);
    }

    fn value(&self) -> f32 {
        let [cross, left_power, right_power] = self.moments;
        let denom = (left_power * right_power).sqrt();
        if denom <= 1e-12 {
            return 0.0;
        }
        let value = cross / denom;
        if value.is_finite() { value.clamp(-1.0, 1.0) as f32 } else { 0.0 }
    }

    fn flush_denormals(&mut self) {
        self.moments.iter_mut().for_each(flush_denormal_f64);
    }
}

pub(super) const FULL_BAND: usize = 0;
pub struct StereometerProcessor {
    config: StereometerConfig,
    snapshot: [Vec<(f32, f32)>; BAND_COUNT + 1],
    histories: [VecDeque<(f32, f32)>; BAND_COUNT + 1],
    history_channels: usize,
    band_splitter: BandSplitter,
    correlators: [Correlator; BAND_COUNT + 1],
    correlation_alpha: f64,
}

impl StereometerProcessor {
    pub fn new(mut config: StereometerConfig) -> Self {
        config.analyze_bands |= config.emit_band_points;
        Self {
            snapshot: Default::default(),
            histories: Default::default(),
            history_channels: 0,
            band_splitter: BandSplitter::new(config.sample_rate, BAND_SPLITS_HZ),
            correlators: Default::default(),
            correlation_alpha: ema_alpha(config.sample_rate, config.correlation_window),
            config,
        }
    }

    pub fn config(&self) -> StereometerConfig {
        self.config
    }

    pub fn reset_audio(&mut self) {
        self.histories.iter_mut().for_each(VecDeque::clear);
        self.band_splitter.clear();
        self.correlators = Default::default();
        self.snapshot = Default::default();
    }

    pub fn process_block(&mut self, block: &AudioBlock<'_>) -> Option<StereometerSnapshot> {
        let channel_count = block.channels;
        if block.is_empty() { return None; }
        let sample_rate = block.sample_rate;
        if self.config.sample_rate != sample_rate {
            let mut config = self.config;
            config.sample_rate = sample_rate;
            self.update_config(config);
        }
        if self.history_channels != channel_count {
            self.histories[FULL_BAND].clear();
            self.history_channels = channel_count;
        }

        let analyze_bands = self.config.analyze_bands;
        let alpha = self.correlation_alpha;
        for [left, right] in block.stereo_frames() {
            self.histories[FULL_BAND].push_back((left, right));
            self.correlators[FULL_BAND].update(left, right, alpha);

            if analyze_bands {
                let bands = self.band_splitter.process([left, right]);
                for ((correlator, history), [left, right]) in self
                    .correlators[1..]
                    .iter_mut()
                    .zip(&mut self.histories[1..])
                    .zip(bands)
                {
                    correlator.update(left, right, alpha);
                    if self.config.emit_band_points {
                        history.push_back((left, right));
                    }
                }
            }
        }
        self.correlators[FULL_BAND].flush_denormals();
        if analyze_bands {
            self.correlators[1..]
                .iter_mut()
                .for_each(Correlator::flush_denormals);
            self.band_splitter.flush_denormals();
        }

        let frames = (self.config.sample_rate * self.config.segment_duration)
            .round()
            .max(1.0) as usize;
        let history_count = if self.config.emit_band_points { BAND_COUNT + 1 } else { 1 };
        for history in &mut self.histories[..history_count] {
            history.drain(..history.len().saturating_sub(frames));
        }

        if self.histories[FULL_BAND].len() < frames { return None; }

        let target = self.config.target_sample_count.clamp(1, frames);
        for (band, (history, buf)) in self.histories[..history_count]
            .iter_mut()
            .zip(&mut self.snapshot[..history_count])
            .enumerate()
        {
            buf.clear();
            if history.len() < frames { continue; }
            let data = history.make_contiguous();
            buf.reserve(target);
            if target == frames {
                if band == FULL_BAND {
                    buf.extend_from_slice(data);
                } else {
                    buf.extend(data.iter().map(|&(l, r)| (l * BAND_DISPLAY_GAIN, r * BAND_DISPLAY_GAIN)));
                }
                continue;
            }
            if band == FULL_BAND {
                buf.extend((0..target).map(|i| data[i * frames / target]));
            } else {
                buf.extend((0..target).map(|i| {
                    let point = data[i * frames / target];
                    (point.0 * BAND_DISPLAY_GAIN, point.1 * BAND_DISPLAY_GAIN)
                }));
            }
        }

        Some(StereometerSnapshot {
            points: std::array::from_fn(|band| snapshot_points(&self.snapshot[band])),
            correlations: std::array::from_fn(|band| {
                if band == FULL_BAND || analyze_bands {
                    self.correlators[band].value()
                } else {
                    0.0
                }
            }),
        })
    }
    pub fn update_config(&mut self, mut config: StereometerConfig) {
        config.analyze_bands |= config.emit_band_points;
        let sample_rate_changed = self.config.sample_rate != config.sample_rate;
        let window_changed =
            (self.config.correlation_window - config.correlation_window).abs() > f32::EPSILON;
        let band_analysis_changed = self.config.analyze_bands != config.analyze_bands;
        self.config = config;

        if sample_rate_changed {
            *self = Self::new(self.config);
        } else {
            if window_changed {
                self.correlation_alpha = ema_alpha(config.sample_rate, config.correlation_window);
            }
            if band_analysis_changed {
                self.band_splitter = BandSplitter::new(config.sample_rate, BAND_SPLITS_HZ);
                self.correlators[1..].fill(Correlator::default());
            }
        }

        if !config.emit_band_points {
            self.histories[1..].fill_with(VecDeque::new);
            self.snapshot[1..].fill_with(Vec::new);
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
        let mut meter = Correlator::default();
        for &(left, right) in pairs {
            meter.update(left, right, 0.5);
        }
        meter.value()
    }

    fn assert_close(a: f32, b: f32) {
        assert!((a - b).abs() <= 1e-6, "{a} != {b}");
    }

    #[test]
    fn snapshot_downsampling_and_mode_transition_preserve_stereo_pairs() {
        let mut processor = StereometerProcessor::new(StereometerConfig {
            sample_rate: 4.0,
            segment_duration: 1.0,
            target_sample_count: 2,
            ..Default::default()
        });
        let samples = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let snapshot = processor
            .process_block(&AudioBlock::new(&samples, 2, 4.0))
            .unwrap();

        assert_eq!(&*snapshot.points[FULL_BAND], &[(1.0, 2.0), (5.0, 6.0)]);
        processor.update_config(StereometerConfig { emit_band_points: true, ..processor.config() });
        let snapshot = processor
            .process_block(&AudioBlock::new(&samples, 2, 4.0))
            .unwrap();
        assert_eq!(&*snapshot.points[FULL_BAND], &[(1.0, 2.0), (5.0, 6.0)]);
        processor.update_config(StereometerConfig { emit_band_points: false, ..processor.config() });
        let snapshot = processor.process_block(&AudioBlock::new(&[9.0, 10.0], 2, 4.0)).unwrap();
        assert_eq!(&*snapshot.points[FULL_BAND], &[(3.0, 4.0), (7.0, 8.0)]);
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
