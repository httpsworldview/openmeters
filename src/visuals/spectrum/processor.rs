// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::dsp::AudioBlock;
use crate::util::audio::{
    Channel, DB_FLOOR, DEFAULT_SAMPLE_RATE, LN_TO_DB, WindowKind,
    compute_fft_bin_normalization, copy_dc_removed_windowed_from_deque, db_to_power,
    sanitize_negative_db, sanitize_sample_rate, window_coefficients,
};
use realfft::{RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex32;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;

pub const MIN_SPECTRUM_EXP_FACTOR: f32 = 0.0;
pub const MAX_SPECTRUM_EXP_FACTOR: f32 = 0.95;
pub const MIN_SPECTRUM_PEAK_DECAY: f32 = 0.0;
pub const MAX_SPECTRUM_PEAK_DECAY: f32 = 120.0;
pub const MIN_SPECTRUM_DB_FLOOR: f32 = DB_FLOOR;
pub const MAX_SPECTRUM_DB_FLOOR: f32 = -1.0;
pub const DEFAULT_SPECTRUM_DB_FLOOR: f32 = -100.0;

const DEFAULT_SPECTRUM_HOP_DIVISOR: usize = 16;
const DEFAULT_SPECTRUM_FFT_SIZE: usize = 16_384;
pub const DEFAULT_SPECTRUM_EXP_FACTOR: f32 = 0.5;
pub const DEFAULT_SPECTRUM_PEAK_DECAY: f32 = 12.0;
const TRACE_COUNT: usize = 2;
const WEIGHTING_COUNT: usize = 2;

pub type SpectrumTraceSnapshot = [Vec<f32>; WEIGHTING_COUNT];

#[derive(Debug, Clone, Default)]
pub struct SpectrumSnapshot {
    pub frequency_bins: Vec<f32>,
    pub traces: [SpectrumTraceSnapshot; TRACE_COUNT],
}

crate::macros::default_struct! {
    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    pub struct SpectrumConfig {
        pub sample_rate: f32 = DEFAULT_SAMPLE_RATE,
        pub fft_size: usize = DEFAULT_SPECTRUM_FFT_SIZE,
        pub hop_size: usize = DEFAULT_SPECTRUM_FFT_SIZE / DEFAULT_SPECTRUM_HOP_DIVISOR,
        pub window: WindowKind = WindowKind::Hann,
        pub averaging: AveragingMode = AveragingMode::None,
        pub source: Channel = Channel::Mid,
        pub secondary_source: Channel = Channel::None,
        pub floor_db: f32 = DEFAULT_SPECTRUM_DB_FLOOR,
    }
}

impl SpectrumConfig {
    pub fn normalize(&mut self) {
        self.sample_rate = sanitize_sample_rate(self.sample_rate);
        self.fft_size = self.fft_size.max(1);
        if self.hop_size == 0 {
            self.hop_size = (self.fft_size / DEFAULT_SPECTRUM_HOP_DIVISOR).max(1);
        }
        self.floor_db = sanitize_negative_db(self.floor_db, DEFAULT_SPECTRUM_DB_FLOOR);
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AveragingMode {
    None,
    Exponential { factor: f32 },
    PeakHold { decay_per_second: f32 },
}

pub struct SpectrumProcessor {
    config: SpectrumConfig,
    snapshot: SpectrumSnapshot,
    planner: RealFftPlanner<f32>,
    fft: Option<Arc<dyn RealToComplex<f32>>>,
    window: Arc<[f32]>,
    real_buffer: Vec<f32>,
    spectrum_buffer: Vec<Complex32>,
    scratch_buffer: Vec<Complex32>,
    bin_normalization: Vec<f32>,
    pcm_buffers: [VecDeque<f32>; TRACE_COUNT],
    pending_skip_frames: usize,
    levels: [SpectrumLevelBuffers; TRACE_COUNT],
    a_weighting_db: Vec<f32>,
}

impl SpectrumProcessor {
    pub fn new(mut config: SpectrumConfig) -> Self {
        config.normalize();
        Self {
            config,
            snapshot: SpectrumSnapshot::default(),
            planner: RealFftPlanner::new(),
            fft: None,
            window: Arc::from([]),
            real_buffer: Vec::new(),
            spectrum_buffer: Vec::new(),
            scratch_buffer: Vec::new(),
            bin_normalization: Vec::new(),
            pcm_buffers: [VecDeque::new(), VecDeque::new()],
            pending_skip_frames: 0,
            levels: Default::default(),
            a_weighting_db: Vec::new(),
        }
    }

    pub fn config(&self) -> SpectrumConfig {
        self.config
    }

    pub fn reset_audio(&mut self) {
        if self.fft.is_some() {
            self.reset_level_buffers();
        }
        self.pcm_buffers.iter_mut().for_each(VecDeque::clear);
        self.pending_skip_frames = 0;
    }

    pub(in crate::visuals) fn prepare(&mut self) {
        if self.fft.is_none() {
            self.rebuild_fft();
        }
    }

    fn rebuild_fft(&mut self) {
        let fft_size = self.config.fft_size;
        let fft = self.planner.plan_fft_forward(fft_size);
        self.window = window_coefficients(self.config.window, fft_size);
        self.real_buffer.resize(fft_size, 0.0);
        self.spectrum_buffer = fft.make_output_vec();
        self.scratch_buffer = fft.make_scratch_vec();
        self.fft = Some(fft);
        self.bin_normalization = compute_fft_bin_normalization(&self.window, fft_size);
        self.reset_buffers();
    }

    fn reset_buffers(&mut self) {
        let bins = self.config.fft_size / 2 + 1;
        let bin_hz = self.config.sample_rate / self.config.fft_size as f32;
        (self.snapshot.frequency_bins, self.a_weighting_db) = (0..bins)
            .map(|bin| {
                let frequency = bin as f32 * bin_hz;
                (frequency, a_weight(frequency))
            })
            .unzip();
        self.reset_level_buffers();
        self.pcm_buffers.iter_mut().for_each(VecDeque::clear);
        self.pending_skip_frames = 0;
    }

    fn reset_level_buffers(&mut self) {
        let bins = self.config.fft_size / 2 + 1;
        let floor = self.config.floor_db;
        for trace in &mut self.snapshot.traces {
            for db in trace { reset_to_floor(db, bins, floor); }
        }
        let state_floor = smoothing_state_floor(&self.a_weighting_db, floor);
        let active = self.active_traces();
        let smoothing = !matches!(self.config.averaging, AveragingMode::None);
        for (index, buffers) in self.levels.iter_mut().enumerate() {
            if active[index] {
                buffers.reset(bins, state_floor, smoothing);
            } else {
                *buffers = SpectrumLevelBuffers::default();
            }
        }
    }

    fn sources(&self) -> [Channel; TRACE_COUNT] {
        [self.config.source, self.config.secondary_source]
    }

    fn active_traces(&self) -> [bool; TRACE_COUNT] {
        let [primary, secondary] = self.sources();
        [primary != Channel::None, secondary != Channel::None && secondary != primary]
    }

    fn process_ready_windows(&mut self) -> bool {
        let fft_size = self.config.fft_size;
        let hop = self.config.hop_size;
        let bins = fft_size / 2 + 1;
        let floor = self.config.floor_db;
        let dt_seconds = hop as f32 / self.config.sample_rate;
        let active = self.active_traces();
        let mut produced = false;

        debug_assert_eq!(self.a_weighting_db.len(), bins);
        if !active.iter().any(|&active| active) { return false; }

        while (0..TRACE_COUNT).all(|trace| !active[trace] || self.pcm_buffers[trace].len() >= fft_size) {
            for (trace, &active) in active.iter().enumerate() {
                if active && !self.process_trace_window(trace, dt_seconds, floor) {
                    return produced;
                }
            }
            let mut drained = hop;
            for (trace, &active) in active.iter().enumerate() {
                if active {
                    let buf = &mut self.pcm_buffers[trace];
                    let count = hop.min(buf.len());
                    buf.drain(..count);
                    drained = drained.min(count);
                }
            }
            self.pending_skip_frames = self
                .pending_skip_frames
                .saturating_add(hop - drained);
            produced = true;
        }

        produced
    }

    fn process_trace_window(&mut self, trace: usize, dt_seconds: f32, floor: f32) -> bool {
        copy_dc_removed_windowed_from_deque(
            &mut self.real_buffer,
            &self.pcm_buffers[trace],
            &self.window,
        );
        if self
            .fft
            .as_ref()
            .expect("spectrum FFT")
            .process_with_scratch(
                &mut self.real_buffer,
                &mut self.spectrum_buffer,
                &mut self.scratch_buffer,
            )
            .is_err()
        {
            return false;
        }

        let level = &mut self.levels[trace];
        let snapshot = &mut self.snapshot.traces[trace];
        for ((power, complex), norm) in level
            .scratch_power
            .iter_mut()
            .zip(&self.spectrum_buffer)
            .zip(&self.bin_normalization)
        {
            *power = complex.norm_sqr() * *norm;
        }
        level.update_outputs(
            self.config.averaging,
            snapshot,
            &self.a_weighting_db,
            dt_seconds,
            floor,
        );
        true
    }

    pub fn process_block(&mut self, block: &AudioBlock<'_>) -> Option<&SpectrumSnapshot> {
        if block.is_empty() { return None; }

        if block.sample_rate != self.config.sample_rate {
            self.config.sample_rate = block.sample_rate;
            if self.fft.is_some() {
                self.reset_buffers();
            }
        }

        self.prepare();
        self.push_sources(block);

        self.process_ready_windows().then_some(&self.snapshot)
    }

    fn push_sources(&mut self, block: &AudioBlock<'_>) {
        let frames = block.frame_count();
        let skip = self.pending_skip_frames.min(frames);
        self.pending_skip_frames -= skip;
        if skip == frames {
            return;
        }

        let [primary_source, secondary_source] = self.sources();
        let active = self.active_traces();
        let [primary, secondary] = self.pcm_buffers.each_mut();
        match active {
            [true, true] => {
                let remaining = frames - skip;
                primary.reserve(remaining);
                secondary.reserve(remaining);
                for stereo in block.stereo_frames().skip(skip) {
                    primary.push_back(primary_source.project(stereo));
                    secondary.push_back(secondary_source.project(stereo));
                }
            }
            [true, false] => primary.extend(block.projected_frames(primary_source).skip(skip)),
            [false, true] => {
                secondary.extend(block.projected_frames(secondary_source).skip(skip));
            }
            [false, false] => {}
        }
    }

    pub fn update_config(&mut self, mut config: SpectrumConfig) {
        let old = self.config;
        config.normalize();
        self.config = config;
        if self.fft.is_none() {
            return;
        }
        let averaging_mode_changed =
            std::mem::discriminant(&old.averaging) != std::mem::discriminant(&config.averaging);
        if old.fft_size != config.fft_size || old.window != config.window {
            self.rebuild_fft();
        } else if old.sample_rate != config.sample_rate
            || old.hop_size != config.hop_size
            || old.source != config.source
            || old.secondary_source != config.secondary_source
        {
            self.reset_buffers();
        } else if averaging_mode_changed
            || (old.floor_db - config.floor_db).abs() > f32::EPSILON
        {
            self.reset_level_buffers();
        }
    }
}

#[derive(Default)]
struct SpectrumLevelBuffers {
    smoothed_power: Vec<f32>,
    scratch_power: Vec<f32>,
    state_floor: f32,
}

fn smoothing_state_floor(weighting_db: &[f32], floor: f32) -> f32 {
    // Positive weighting can lift raw power from below the floor into view.
    let headroom_db = weighting_db.iter().copied().fold(0.0_f32, f32::max);
    db_to_power(floor - headroom_db).max(f32::MIN_POSITIVE)
}

impl SpectrumLevelBuffers {
    fn reset(&mut self, bins: usize, state_floor: f32, smoothing: bool) {
        self.state_floor = state_floor;
        if smoothing {
            reset_to_floor(&mut self.smoothed_power, bins, 0.0);
        } else {
            self.smoothed_power = Vec::new();
        }
        reset_to_floor(&mut self.scratch_power, bins, 0.0);
    }

    fn update_outputs(
        &mut self,
        mode: AveragingMode,
        outputs: &mut [Vec<f32>; WEIGHTING_COUNT],
        weighting_db: &[f32],
        dt_seconds: f32,
        floor: f32,
    ) {
        let bins = self.scratch_power.len();
        debug_assert_eq!(weighting_db.len(), bins);
        for output in outputs.iter_mut() {
            if output.len() != bins {
                output.resize(bins, floor);
            }
        }
        let powers = match mode {
            AveragingMode::None => &self.scratch_power,
            AveragingMode::Exponential { factor } => {
                let alpha = factor.clamp(0.0, 0.9999);
                for (avg, &power) in self.smoothed_power.iter_mut().zip(&self.scratch_power) {
                    *avg = if *avg <= 0.0 {
                        power
                    } else {
                        *avg * alpha + power * (1.0 - alpha)
                    };
                    if *avg < self.state_floor {
                        *avg = 0.0;
                    }
                }
                &self.smoothed_power
            }
            AveragingMode::PeakHold { decay_per_second } => {
                let decay = db_to_power(-decay_per_second.max(0.0) * dt_seconds);
                for (hold, &power) in self.smoothed_power.iter_mut().zip(&self.scratch_power) {
                    *hold = (*hold * decay).max(power);
                    if *hold < self.state_floor {
                        *hold = 0.0;
                    }
                }
                &self.smoothed_power
            }
        };
        let [weighted_out, raw_out] = outputs;
        let bins = powers.iter().zip(weighting_db).zip(raw_out).zip(weighted_out);
        for (((&power, &weight), raw), weighted) in bins {
            if power < self.state_floor {
                (*raw, *weighted) = (floor, floor);
                continue;
            }
            let db = power.ln() * LN_TO_DB;
            *raw = db.max(floor);
            *weighted = (db + weight).max(floor);
        }
    }
}

fn reset_to_floor(buf: &mut Vec<f32>, bins: usize, floor: f32) {
    buf.clear();
    buf.resize(bins, floor);
}

fn a_weight(freq_hz: f32) -> f32 {
    const C1: f64 = 20.598_997 * 20.598_997;
    const C2: f64 = 107.652_65 * 107.652_65;
    const C3: f64 = 737.862_23 * 737.862_23;
    const C4: f64 = 12_194.217 * 12_194.217;

    if freq_hz <= 0.0 { return f32::NEG_INFINITY; }

    let f = freq_hz as f64;
    let f2 = f * f;
    let numerator = C4 * f2 * f2;
    let denom = (f2 + C1) * ((f2 + C2) * (f2 + C3)).sqrt() * (f2 + C4);

    let ra = numerator / denom;
    (20.0 * ra.log10() + 2.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::audio::sine_wave;

    #[test]
    fn normalization_bounds_runtime_values_without_enforcing_gui_ranges() {
        let mut invalid = SpectrumConfig {
            sample_rate: f32::NAN,
            fft_size: 0,
            hop_size: 0,
            floor_db: f32::INFINITY,
            ..Default::default()
        };
        invalid.normalize();
        assert_eq!(invalid.sample_rate, DEFAULT_SAMPLE_RATE);
        assert_eq!(invalid.fft_size, 1);
        assert_eq!(invalid.hop_size, 1);
        assert_eq!(invalid.floor_db, DEFAULT_SPECTRUM_DB_FLOOR);

        for (floor_db, expected) in [
            (1.0, DEFAULT_SPECTRUM_DB_FLOOR),
            (MIN_SPECTRUM_DB_FLOOR * 2.0, MIN_SPECTRUM_DB_FLOOR * 2.0),
        ] {
            let mut config = SpectrumConfig {
                floor_db,
                ..Default::default()
            };
            config.normalize();
            assert_eq!(config.floor_db, expected);
        }
    }

    #[test]
    fn floor_change_reseeds_state_buffers_without_clearing_pending_audio() {
        let mut p = SpectrumProcessor::new(SpectrumConfig::default());
        p.prepare();
        p.pcm_buffers[0].extend([0.25, -0.25]);
        let mut cfg = p.config();
        cfg.floor_db = -96.0;

        p.update_config(cfg);

        assert_eq!(p.pcm_buffers[0].len(), 2);
        for output in &p.snapshot.traces[0] {
            assert!(output.iter().all(|&v| v == cfg.floor_db));
        }
        let bins = cfg.fft_size / 2 + 1;
        assert_eq!(p.levels[0].scratch_power.len(), bins);
        assert!(p.levels[0].scratch_power.iter().all(|&v| v == 0.0));
        assert!(p.levels[0].smoothed_power.is_empty());
        assert!(p.levels[1].scratch_power.is_empty());
    }

    #[test]
    fn configured_sources_drive_their_fft_traces() {
        let sample_rate = 8.0;
        let samples: Vec<_> = sine_wave(1.0, sample_rate, 8, 1.0)
            .into_iter()
            .zip(sine_wave(2.0, sample_rate, 8, 1.0))
            .flat_map(|(left, side)| [left, left - 2.0 * side])
            .collect();

        for (source, expected_bins) in [
            (Channel::Left, [Some(1), Some(2)]),
            (Channel::None, [None, Some(2)]),
        ] {
            let config = SpectrumConfig {
                sample_rate,
                fft_size: 8,
                hop_size: 8,
                window: WindowKind::Rectangular,
                source,
                secondary_source: Channel::Side,
                ..Default::default()
            };
            let mut processor = SpectrumProcessor::new(config);
            let snapshot = processor
                .process_block(&AudioBlock::new(&samples, 2, sample_rate))
                .expect("expected spectrum");

            for (trace, expected_bin) in snapshot.traces.iter().zip(expected_bins) {
                let raw = &trace[1];
                match expected_bin {
                    Some(expected) => {
                        let (bin, level) = raw
                            .iter()
                            .enumerate()
                            .max_by(|a, b| a.1.total_cmp(b.1))
                            .unwrap();
                        assert_eq!(bin, expected);
                        assert!(level.abs() < 0.01, "peak was {level} dBFS");
                    }
                    None => assert!(raw.iter().all(|&level| level == config.floor_db)),
                }
            }
        }
    }

    #[test]
    fn fft_size_update_takes_effect_before_processing() {
        let mut processor = SpectrumProcessor::new(SpectrumConfig {
            fft_size: 128,
            hop_size: 128,
            ..Default::default()
        });
        processor.prepare();
        let mut config = processor.config();
        config.fft_size = 256;
        config.hop_size = 256;
        processor.update_config(config);

        let bins = config.fft_size / 2 + 1;
        let samples = vec![0.0; config.fft_size];
        let snapshot = processor
            .process_block(&AudioBlock::new(&samples, 1, config.sample_rate))
            .expect("expected resized spectrum");

        assert_eq!(snapshot.frequency_bins.len(), bins);
        assert!(snapshot.traces[0].iter().all(|trace| trace.len() == bins));
    }

    #[test]
    fn peak_hold_decays_for_each_audio_hop_in_large_batch() {
        let mut p = SpectrumProcessor::new(SpectrumConfig {
            sample_rate: 8.0,
            fft_size: 8,
            hop_size: 8,
            window: WindowKind::Rectangular,
            averaging: AveragingMode::PeakHold {
                decay_per_second: 24.0,
            },
            floor_db: -100.0,
            ..Default::default()
        });
        let mut samples = sine_wave(1.0, 8.0, 8, 1.0);
        samples.extend([0.0; 8]);

        let snap = p
            .process_block(&AudioBlock::new(&samples, 1, 8.0))
            .expect("expected snapshot");
        let held_db = snap.traces[0][1][1];

        assert!(
            (-24.1..-23.9).contains(&held_db),
            "held peak should decay once per hop, got {held_db} dB"
        );
    }

    #[test]
    fn changing_averaging_mode_clears_stale_state() {
        let mut processor = SpectrumProcessor::new(SpectrumConfig {
            averaging: AveragingMode::PeakHold {
                decay_per_second: 12.0,
            },
            ..SpectrumConfig::default()
        });
        processor.prepare();
        processor.levels[0].smoothed_power.fill(1.0);
        let mut config = processor.config();
        config.averaging = AveragingMode::Exponential { factor: 0.5 };

        processor.update_config(config);

        assert!(processor.levels[0].smoothed_power.iter().all(|&power| power == 0.0));
    }

    #[test]
    fn hops_larger_than_the_fft_are_block_partition_independent() {
        let config = SpectrumConfig {
            sample_rate: 32.0,
            fft_size: 8,
            hop_size: 16,
            window: WindowKind::Rectangular,
            source: Channel::Left,
            ..Default::default()
        };
        let samples: Vec<_> = (0..29).map(|i| (i as f32 * 0.73).sin()).collect();

        let mut whole_processor = SpectrumProcessor::new(config);
        let whole = whole_processor
            .process_block(&AudioBlock::new(&samples, 1, 32.0))
            .unwrap();

        let mut partitioned_processor = SpectrumProcessor::new(config);
        let mut partitioned = None;
        for chunk in samples.chunks(8) {
            partitioned = partitioned_processor
                .process_block(&AudioBlock::new(chunk, 1, 32.0))
                .cloned()
                .or(partitioned);
        }
        let partitioned = partitioned.unwrap();

        assert_eq!(whole.traces[0], partitioned.traces[0]);
    }

    #[test]
    fn averaged_power_is_zeroed_below_the_visible_floor() {
        let mut buffers = SpectrumLevelBuffers::default();
        buffers.reset(1, smoothing_state_floor(&[0.0], -100.0), true);
        buffers.smoothed_power[0] = db_to_power(-101.0);
        let mut outputs = [Vec::new(), Vec::new()];
        buffers.update_outputs(
            AveragingMode::Exponential { factor: 0.95 },
            &mut outputs,
            &[0.0],
            1.0,
            -100.0,
        );
        assert_eq!(buffers.smoothed_power[0], 0.0);
    }

    #[test]
    fn smoothing_retains_power_visible_after_weighting() {
        for mode in [
            AveragingMode::Exponential { factor: 0.95 },
            AveragingMode::PeakHold {
                decay_per_second: 12.0,
            },
        ] {
            let mut buffers = SpectrumLevelBuffers::default();
            buffers.reset(1, smoothing_state_floor(&[1.2], -100.0), true);
            buffers.scratch_power[0] = db_to_power(-100.5);
            let mut outputs = [Vec::new(), Vec::new()];

            buffers.update_outputs(mode, &mut outputs, &[1.2], 1.0, -100.0);

            assert_eq!(outputs[1][0], -100.0);
            assert!(
                (-99.4..-99.2).contains(&outputs[0][0]),
                "weighted output for {mode:?} was {} dB",
                outputs[0][0]
            );
        }
    }

    #[test]
    fn a_weight_matches_iec_reference_points() {
        let reference_points: &[(f32, f32)] = &[
            (1.0, -148.6),
            (5.0, -93.1),
            (31.5, -39.4),
            (63.0, -26.2),
            (100.0, -19.1),
            (200.0, -10.9),
            (500.0, -3.2),
            (1000.0, 0.0),
            (2000.0, 1.2),
            (4000.0, 1.0),
            (8000.0, -1.1),
            (16000.0, -6.6),
        ];

        for &(freq, expected_db) in reference_points {
            let actual = a_weight(freq);
            let delta = (actual - expected_db).abs();
            assert!(
                delta <= 0.15,
                "A-weight mismatch at {freq} Hz: expected {expected_db} dB, got {actual} dB (delta={delta})"
            );
        }
    }
}
