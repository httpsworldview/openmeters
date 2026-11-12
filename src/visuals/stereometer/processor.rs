// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::dsp::{AudioBlock, SampleRate, ThreeBand};
use crate::util::Finite;
pub(super) use crate::util::audio::BAND_COUNT;
use crate::util::audio::{
    BAND_SPLITS_HZ, MAX_DSP_BUFFER_LEN, Seconds, flush_denormal_f64, smoothing_weight,
};
use std::{collections::VecDeque, sync::Arc};

// Split-band display only; correlation is unscaled.
const BAND_DISPLAY_GAIN: f32 = 0.8;
// Per cloud, including full-band.
const MAX_SNAPSHOT_POINTS: usize = 16_384;

crate::macros::default_struct! {
    #[derive(Debug, Clone, Copy)]
    pub struct StereometerConfig {
        pub sample_rate: SampleRate = SampleRate::DEFAULT,
        pub segment_duration: Finite = Finite::must(0.02),
        pub target_sample_count: usize = 2_000,
        pub correlation_window: Seconds = Seconds::must(0.3),
        pub bands: Bands = Bands::Off,
    }
}

// What the split bands feed; each level includes the one before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bands {
    Off,
    Correlations,
    Points,
}

impl Bands {
    pub fn analyzed(self) -> bool {
        self != Self::Off
    }
}

pub struct StereometerSnapshot {
    pub elapsed: f32,
    pub points: [Arc<[(f32, f32)]>; BAND_COUNT + 1],
    pub correlations: [f32; BAND_COUNT + 1],
}

type BandSplitter = ThreeBand<2, 2, true>;

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
        (cross / denom).clamp(-1.0, 1.0) as f32
    }

    fn flush_denormals(&mut self) {
        self.moments.iter_mut().for_each(flush_denormal_f64);
    }
}

pub(super) const FULL_BAND: usize = 0;
pub struct StereometerProcessor {
    config: StereometerConfig,
    histories: [VecDeque<(f32, f32)>; BAND_COUNT + 1],
    band_splitter: BandSplitter,
    correlators: [Correlator; BAND_COUNT + 1],
    silent_frames: usize,
}

impl StereometerProcessor {
    pub fn new(config: StereometerConfig) -> Self {
        Self {
            histories: Default::default(),
            band_splitter: BandSplitter::new(config.sample_rate, BAND_SPLITS_HZ),
            correlators: Default::default(),
            silent_frames: 0,
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
        self.silent_frames = 0;
    }

    pub fn process_block(&mut self, block: &AudioBlock<'_>) -> Option<StereometerSnapshot> {
        let sample_rate = block.sample_rate;
        if self.config.sample_rate != sample_rate {
            self.update_config(StereometerConfig {
                sample_rate,
                ..self.config
            });
        }

        let mode = self.config.bands;
        let frame = sample_rate.hz().recip();
        // A zero window would pin correlation to +/-1.
        let window = self.config.correlation_window.get().max(frame);
        let alpha = f64::from(smoothing_weight(frame, window));
        for [left, right] in block.stereo_frames() {
            self.histories[FULL_BAND].push_back((left, right));
            self.correlators[FULL_BAND].update(left, right, alpha);
            self.silent_frames = if [left, right] == [0.0; 2] {
                self.silent_frames + 1
            } else {
                0
            };

            if mode.analyzed() {
                let bands = self.band_splitter.process([left, right]);
                for ((correlator, history), [left, right]) in self.correlators[1..]
                    .iter_mut()
                    .zip(&mut self.histories[1..])
                    .zip(bands)
                {
                    correlator.update(left, right, alpha);
                    if mode == Bands::Points {
                        history.push_back((left * BAND_DISPLAY_GAIN, right * BAND_DISPLAY_GAIN));
                    }
                }
            }
        }
        self.correlators
            .iter_mut()
            .for_each(Correlator::flush_denormals);
        self.band_splitter.flush_denormals();
        // A window of only digital silence holds no signal: its correlation is undefined.
        let silent = self.silent_frames as f32 >= window * sample_rate.hz();

        let frames = sample_rate
            .frames(self.config.segment_duration.get())
            .min(MAX_DSP_BUFFER_LEN);
        let history_count = 1 + BAND_COUNT * usize::from(mode == Bands::Points);
        for history in &mut self.histories[..history_count] {
            history.drain(..history.len().saturating_sub(frames));
        }

        if self.histories[FULL_BAND].len() < frames {
            return None;
        }

        let target = self
            .config
            .target_sample_count
            .clamp(1, frames)
            .min(MAX_SNAPSHOT_POINTS);
        Some(StereometerSnapshot {
            elapsed: block.duration(),
            points: std::array::from_fn(|band| {
                let history = &mut self.histories[band];
                if band >= history_count || history.len() < frames {
                    return Arc::default();
                }
                let data = history.make_contiguous();
                if target == frames {
                    return Arc::from(&*data);
                }
                let scale = (frames - 1) as f64 / (target - 1).max(1) as f64;
                (0..target)
                    .map(|i| {
                        let index = if target == 1 {
                            frames - 1
                        } else {
                            (i as f64 * scale).round() as usize
                        };
                        data[index]
                    })
                    .collect()
            }),
            // Band correlators hold zero moments, and read 0, while bands are not analyzed.
            correlations: self
                .correlators
                .map(|correlator| if silent { 0.0 } else { correlator.value() }),
        })
    }
    pub fn update_config(&mut self, config: StereometerConfig) {
        if self.config.sample_rate != config.sample_rate {
            *self = Self::new(config);
            return;
        }
        if self.config.bands.analyzed() != config.bands.analyzed() {
            self.band_splitter.clear();
            self.correlators[1..].fill(Correlator::default());
        }
        if config.bands != Bands::Points {
            self.histories[1..].fill_with(VecDeque::new);
        }
        self.config = config;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: SampleRate = SampleRate::must(4);

    fn correlation(pairs: &[(f32, f32)]) -> f32 {
        let mut meter = Correlator::default();
        pairs
            .iter()
            .for_each(|&(left, right)| meter.update(left, right, 0.5));
        meter.value()
    }

    fn assert_close(a: f32, b: f32) {
        assert!((a - b).abs() <= 1e-6, "{a} != {b}");
    }

    #[test]
    fn snapshot_downsampling_and_mode_transition_preserve_stereo_pairs() {
        let mut processor = StereometerProcessor::new(StereometerConfig {
            segment_duration: Finite::must(1.0),
            target_sample_count: 2,
            correlation_window: Seconds::must(1.0),
            ..Default::default()
        });
        let samples = [1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0, 1.0];
        for (target_sample_count, expected) in [
            (1, vec![(-1.0, 1.0)]),
            (2, vec![(1.0, -1.0), (-1.0, 1.0)]),
            (3, vec![(1.0, -1.0), (1.0, -1.0), (-1.0, 1.0)]),
            (
                usize::MAX,
                vec![(1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, 1.0)],
            ),
        ] {
            let mut processor = StereometerProcessor::new(StereometerConfig {
                target_sample_count,
                ..processor.config()
            });
            for block in [samples.as_slice(), &samples[..2]] {
                let snapshot = processor
                    .process_block(&AudioBlock::fallback(block, 2, RATE))
                    .unwrap();
                if block.len() == samples.len() {
                    assert_eq!(&*snapshot.points[FULL_BAND], expected);
                } else {
                    assert_eq!(snapshot.points[FULL_BAND].last(), Some(&(1.0, -1.0)));
                }
            }
        }
        for bands in [Bands::Off, Bands::Points] {
            processor.update_config(StereometerConfig {
                bands,
                ..processor.config()
            });
            let snapshot = processor
                .process_block(&AudioBlock::fallback(&samples, 2, RATE))
                .unwrap();
            assert_eq!(&*snapshot.points[FULL_BAND], &[(1.0, -1.0), (-1.0, 1.0)]);
            assert_close(snapshot.correlations[FULL_BAND], -1.0);
            assert!(
                snapshot.points[1..]
                    .iter()
                    .all(|points| points.len() == if bands == Bands::Points { 2 } else { 0 })
            );
        }
        processor.update_config(StereometerConfig {
            bands: Bands::Correlations,
            ..processor.config()
        });
        // A single new frame must retain the full-band history from before the mode change.
        let snapshot = processor
            .process_block(&AudioBlock::fallback(&[1.0, -1.0], 2, RATE))
            .unwrap();
        assert_eq!(&*snapshot.points[FULL_BAND], &[(-1.0, 1.0), (1.0, -1.0)]);
        assert_close(snapshot.correlations[FULL_BAND], -1.0);
        assert!(snapshot.points[1..].iter().all(|points| points.is_empty()));
        processor.reset_audio();
        let snapshot = processor
            .process_block(&AudioBlock::fallback(&[1.0; 4], 1, RATE))
            .unwrap();
        assert_close(snapshot.correlations[FULL_BAND], 1.0);
    }

    #[test]
    fn band_points_match_lr4_response_and_correlations_follow_each_band() {
        let config = StereometerConfig {
            sample_rate: SampleRate::DEFAULT,
            segment_duration: Finite::must(0.1),
            target_sample_count: 4_800,
            bands: Bands::Points,
            ..Default::default()
        };
        let tone = |frequency: f64, frame| {
            (std::f64::consts::TAU * frequency * frame as f64 / 48_000.0).sin() as f32
        };
        for frequency in [80.0, 200.0, 500.0, 2_000.0, 5_000.0] {
            let samples: Vec<_> = (0..24_000)
                .flat_map(|frame| {
                    let sample = tone(frequency, frame);
                    [sample, -sample * 0.5]
                })
                .collect();
            let snapshot = StereometerProcessor::new(config)
                .process_block(&AudioBlock::fallback(&samples, 2, SampleRate::DEFAULT))
                .unwrap();
            // Bilinear LR4 amplitude: two cascaded Butterworth sections.
            let warp = |frequency: f64| (std::f64::consts::PI * frequency / 48_000.0).tan();
            let lowpass = |cutoff| 1.0 / (1.0 + (warp(frequency) / warp(cutoff)).powi(4));
            let (low, high) = (lowpass(200.0), lowpass(2_000.0));
            for (band, gain) in [low, (1.0 - low) * high, (1.0 - low) * (1.0 - high)]
                .into_iter()
                .enumerate()
            {
                let points = &snapshot.points[band + 1];
                assert_eq!(points.len(), 4_800);
                let rms = (points
                    .iter()
                    .map(|&(left, _)| f64::from(left).powi(2))
                    .sum::<f64>()
                    / 4_800.0)
                    .sqrt();
                let expected = gain * 0.8 / 2.0_f64.sqrt();
                assert!(
                    (rms - expected).abs() < 1.0e-6,
                    "{frequency} Hz, band {band}: {rms} vs {expected}"
                );
                assert!(
                    points
                        .iter()
                        .all(|&(left, right)| (right + left * 0.5).abs() < 1.0e-6)
                );
            }
        }
        let samples: Vec<_> = (0..48_000)
            .flat_map(|frame| {
                let [low, mid, high] =
                    [80.0, 700.0, 8_000.0].map(|frequency| tone(frequency, frame));
                [low + mid + high, low - mid + high]
            })
            .collect();
        let mut processor = StereometerProcessor::new(StereometerConfig {
            bands: Bands::Correlations,
            ..config
        });
        for bands in [Bands::Correlations, Bands::Points, Bands::Correlations] {
            processor.update_config(StereometerConfig {
                bands,
                ..processor.config()
            });
            let snapshot = processor
                .process_block(&AudioBlock::fallback(&samples, 2, SampleRate::DEFAULT))
                .unwrap();
            for (&actual, expected) in snapshot.correlations[1..].iter().zip([1.0, -1.0, 1.0]) {
                assert!(
                    (actual - expected).abs() < 0.02,
                    "{bands:?}, band correlation: {actual} vs {expected}"
                );
            }
        }
    }

    #[test]
    fn correlator_matches_reference_points() {
        assert_close(correlation(&[(1.0, 1.0), (-1.0, -1.0)]), 1.0);
        assert_close(correlation(&[(1.0, -1.0), (-1.0, 1.0)]), -1.0);
        assert_close(correlation(&[(1.0, 0.25), (-1.0, -0.25)]), 1.0);
        assert_close(correlation(&[(1., 0.), (0., 1.), (-1., 0.), (0., -1.)]), 0.);
        assert_close(correlation(&[(0.0, 0.0)]), 0.0);
    }

    #[test]
    fn overflowing_downmix_leaves_correlation_finite() {
        let mut processor = StereometerProcessor::new(StereometerConfig {
            segment_duration: Finite::must(1.0),
            ..Default::default()
        });
        // Front left and center both feed the left side, so their sum overflows.
        let samples = [[f32::MAX, 0.0, f32::MAX], [0.5; 3], [0.5; 3], [0.5; 3]];
        let snapshot = processor
            .process_block(&AudioBlock::fallback(samples.as_flattened(), 3, RATE))
            .unwrap();
        assert!(snapshot.correlations[FULL_BAND].is_finite());
    }
}
