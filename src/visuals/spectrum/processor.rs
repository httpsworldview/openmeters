// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::dsp::{AudioBlock, AudioProcessor, Reconfigurable};
use crate::util::audio::{
    DB_FLOOR, DEFAULT_SAMPLE_RATE, FrequencyScale, WindowKind, apply_window,
    compute_fft_bin_normalization, copy_dc_removed_from_deque, mixdown_into_deque, power_to_db,
    window_coefficients,
};
use realfft::{RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex32;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

pub const MIN_SPECTRUM_EXP_FACTOR: f32 = 0.0;
pub const MAX_SPECTRUM_EXP_FACTOR: f32 = 0.95;
pub const MIN_SPECTRUM_PEAK_DECAY: f32 = 0.0;
pub const MAX_SPECTRUM_PEAK_DECAY: f32 = 120.0;
pub const MIN_SPECTRUM_DB_FLOOR: f32 = DB_FLOOR;
pub const MAX_SPECTRUM_DB_FLOOR: f32 = -1.0;
pub const DEFAULT_SPECTRUM_DB_FLOOR: f32 = -80.0;

const MIN_SPECTRUM_FFT_SIZE: usize = 128;
const DEFAULT_SPECTRUM_HOP_DIVISOR: usize = 8;
const DEFAULT_SPECTRUM_FFT_SIZE: usize = 4096;
const DEFAULT_SPECTRUM_EXP_FACTOR: f32 = 0.5;
const DEFAULT_SPECTRUM_PEAK_DECAY: f32 = 12.0;

fn frequency_bins(sample_rate: f32, fft_size: usize) -> Vec<f32> {
    let bins = fft_size / 2 + 1;
    let bin_hz = sample_rate / fft_size as f32;
    (0..bins).map(|i| i as f32 * bin_hz).collect()
}

#[derive(Debug, Clone, Default)]
pub struct SpectrumSnapshot {
    pub frequency_bins: Vec<f32>,
    pub magnitudes_db: Vec<f32>,
    pub magnitudes_unweighted_db: Vec<f32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SpectrumConfig {
    pub sample_rate: f32,
    pub fft_size: usize,
    pub hop_size: usize,
    pub window: WindowKind,
    pub averaging: AveragingMode,
    pub frequency_scale: FrequencyScale,
    pub reverse_frequency: bool,
    pub show_grid: bool,
    pub show_peak_label: bool,
    pub floor_db: f32,
}

impl Default for SpectrumConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            fft_size: DEFAULT_SPECTRUM_FFT_SIZE,
            hop_size: DEFAULT_SPECTRUM_FFT_SIZE / DEFAULT_SPECTRUM_HOP_DIVISOR,
            window: WindowKind::BlackmanHarris,
            averaging: AveragingMode::Exponential {
                factor: DEFAULT_SPECTRUM_EXP_FACTOR,
            },
            frequency_scale: FrequencyScale::Logarithmic,
            reverse_frequency: false,
            show_grid: true,
            show_peak_label: true,
            floor_db: DEFAULT_SPECTRUM_DB_FLOOR,
        }
    }
}

impl SpectrumConfig {
    pub fn normalize(&mut self) {
        if !self.sample_rate.is_finite() || self.sample_rate <= 0.0 {
            self.sample_rate = DEFAULT_SAMPLE_RATE;
        }

        self.fft_size = self.fft_size.max(MIN_SPECTRUM_FFT_SIZE);

        self.hop_size = if self.hop_size == 0 {
            (self.fft_size / DEFAULT_SPECTRUM_HOP_DIVISOR).max(1)
        } else {
            self.hop_size.clamp(1, self.fft_size)
        };

        self.averaging = self.averaging.normalized();
        self.floor_db = clamp_finite(self.floor_db, MIN_SPECTRUM_DB_FLOOR, MAX_SPECTRUM_DB_FLOOR);
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AveragingMode {
    None,
    Exponential { factor: f32 },
    PeakHold { decay_per_second: f32 },
}

impl AveragingMode {
    pub fn normalized(self) -> Self {
        match self {
            AveragingMode::None => AveragingMode::None,
            AveragingMode::Exponential { factor } => AveragingMode::Exponential {
                factor: clamp_finite(factor, MIN_SPECTRUM_EXP_FACTOR, MAX_SPECTRUM_EXP_FACTOR),
            },
            AveragingMode::PeakHold { decay_per_second } => AveragingMode::PeakHold {
                decay_per_second: clamp_finite(
                    decay_per_second,
                    MIN_SPECTRUM_PEAK_DECAY,
                    MAX_SPECTRUM_PEAK_DECAY,
                ),
            },
        }
    }

    pub const fn default_exponential_factor() -> f32 {
        DEFAULT_SPECTRUM_EXP_FACTOR
    }

    pub const fn default_peak_decay() -> f32 {
        DEFAULT_SPECTRUM_PEAK_DECAY
    }
}

fn clamp_finite(value: f32, min: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        min
    }
}

pub struct SpectrumProcessor {
    config: SpectrumConfig,
    snapshot: SpectrumSnapshot,
    planner: RealFftPlanner<f32>,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Arc<[f32]>,
    real_buffer: Vec<f32>,
    spectrum_buffer: Vec<Complex32>,
    scratch_buffer: Vec<Complex32>,
    bin_normalization: Vec<f32>,
    pcm_buffer: VecDeque<f32>,
    averaged_db: Vec<f32>,
    peak_hold_db: Vec<f32>,
    scratch_magnitudes: Vec<f32>,
    averaged_unweighted_db: Vec<f32>,
    peak_hold_unweighted_db: Vec<f32>,
    scratch_unweighted: Vec<f32>,
    a_weighting_db: Vec<f32>,
    last_update_at: Option<Instant>,
}

impl SpectrumProcessor {
    pub fn new(mut config: SpectrumConfig) -> Self {
        config.normalize();
        let fft_size = config.fft_size;
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);
        let mut processor = Self {
            config,
            snapshot: SpectrumSnapshot::default(),
            planner,
            fft,
            window: Arc::from([]),
            real_buffer: Vec::new(),
            spectrum_buffer: Vec::new(),
            scratch_buffer: Vec::new(),
            bin_normalization: Vec::new(),
            pcm_buffer: VecDeque::new(),
            averaged_db: Vec::new(),
            peak_hold_db: Vec::new(),
            scratch_magnitudes: Vec::new(),
            averaged_unweighted_db: Vec::new(),
            peak_hold_unweighted_db: Vec::new(),
            scratch_unweighted: Vec::new(),
            a_weighting_db: Vec::new(),
            last_update_at: None,
        };
        processor.rebuild_fft();
        processor
    }

    pub fn config(&self) -> SpectrumConfig {
        self.config
    }

    fn rebuild_fft(&mut self) {
        self.config.normalize();
        let fft_size = self.config.fft_size;
        self.fft = self.planner.plan_fft_forward(fft_size);
        self.window = window_coefficients(self.config.window, fft_size);
        self.real_buffer.resize(fft_size, 0.0);
        self.spectrum_buffer = self.fft.make_output_vec();
        self.scratch_buffer = self.fft.make_scratch_vec();
        self.bin_normalization = compute_fft_bin_normalization(&self.window, fft_size);
        self.reset_buffers();
    }

    fn reset_buffers(&mut self) {
        self.snapshot.frequency_bins =
            frequency_bins(self.config.sample_rate, self.config.fft_size);
        self.a_weighting_db = self
            .snapshot
            .frequency_bins
            .iter()
            .map(|&f| a_weight(f))
            .collect();
        self.reset_level_buffers();
        self.pcm_buffer.clear();
    }

    fn reset_level_buffers(&mut self) {
        let bins = self.config.fft_size / 2 + 1;
        let floor = self.config.floor_db;
        for buf in [
            &mut self.snapshot.magnitudes_db,
            &mut self.snapshot.magnitudes_unweighted_db,
            &mut self.averaged_db,
            &mut self.peak_hold_db,
            &mut self.scratch_magnitudes,
            &mut self.averaged_unweighted_db,
            &mut self.peak_hold_unweighted_db,
            &mut self.scratch_unweighted,
        ] {
            buf.clear();
            buf.resize(bins, floor);
        }
    }

    fn ensure_fft(&mut self) {
        if self.real_buffer.len() != self.config.fft_size {
            self.rebuild_fft();
        }
    }

    fn mixdown_into(&mut self, block: &AudioBlock<'_>) {
        mixdown_into_deque(&mut self.pcm_buffer, block.samples, block.channels);
    }

    fn process_ready_windows(&mut self, timestamp: Instant) -> bool {
        let fft_size = self.config.fft_size;
        let hop = self.config.hop_size.max(1);
        let bins = fft_size / 2 + 1;
        let floor = self.config.floor_db;
        let mut produced = false;

        for buf in [&mut self.scratch_magnitudes, &mut self.scratch_unweighted] {
            if buf.len() != bins {
                buf.resize(bins, floor);
            }
        }
        if self.a_weighting_db.len() != bins {
            self.a_weighting_db = self
                .snapshot
                .frequency_bins
                .iter()
                .map(|&f| a_weight(f))
                .collect();
        }

        while self.pcm_buffer.len() >= fft_size {
            copy_dc_removed_from_deque(&mut self.real_buffer[..fft_size], &self.pcm_buffer);
            apply_window(&mut self.real_buffer, &self.window);

            if self
                .fft
                .process_with_scratch(
                    &mut self.real_buffer,
                    &mut self.spectrum_buffer,
                    &mut self.scratch_buffer,
                )
                .is_err()
            {
                return produced;
            }

            for (idx, ((complex, norm), &weight)) in self
                .spectrum_buffer
                .iter()
                .zip(&self.bin_normalization)
                .zip(&self.a_weighting_db)
                .take(bins)
                .enumerate()
            {
                let raw_magnitude = power_to_db(complex.norm_sqr() * *norm, floor);
                self.scratch_unweighted[idx] = raw_magnitude;
                let weight = if raw_magnitude > floor { weight } else { 0.0 };
                self.scratch_magnitudes[idx] = (raw_magnitude + weight).max(floor);
            }

            let dt_seconds = self.last_update_at.map_or(0.0, |last| {
                timestamp.saturating_duration_since(last).as_secs_f32()
            });
            averaging_update(
                &self.config.averaging,
                &mut self.averaged_db,
                &mut self.peak_hold_db,
                &mut self.snapshot.magnitudes_db,
                &self.scratch_magnitudes,
                dt_seconds,
                floor,
            );

            averaging_update(
                &self.config.averaging,
                &mut self.averaged_unweighted_db,
                &mut self.peak_hold_unweighted_db,
                &mut self.snapshot.magnitudes_unweighted_db,
                &self.scratch_unweighted,
                dt_seconds,
                floor,
            );

            self.last_update_at = Some(timestamp);

            self.pcm_buffer.drain(..hop);

            produced = true;
        }

        produced
    }
}

impl AudioProcessor for SpectrumProcessor {
    type Output = SpectrumSnapshot;

    fn process_block(&mut self, block: &AudioBlock<'_>) -> Option<Self::Output> {
        if block.frame_count() == 0 {
            return None;
        }

        if (block.sample_rate - self.config.sample_rate).abs() > f32::EPSILON {
            self.config.sample_rate = block.sample_rate;
            self.reset_buffers();
        }

        self.ensure_fft();
        self.mixdown_into(block);

        if self.process_ready_windows(block.timestamp) {
            Some(self.snapshot.clone())
        } else {
            None
        }
    }

    fn reset(&mut self) {
        self.reset_buffers();
        self.last_update_at = None;
    }
}

impl Reconfigurable<SpectrumConfig> for SpectrumProcessor {
    fn update_config(&mut self, mut config: SpectrumConfig) {
        let old = self.config;
        config.normalize();
        self.config = config;
        if old.fft_size != config.fft_size || old.window != config.window {
            self.rebuild_fft();
        } else if (old.sample_rate - config.sample_rate).abs() > f32::EPSILON {
            self.reset_buffers();
        } else if (old.floor_db - config.floor_db).abs() > f32::EPSILON {
            self.reset_level_buffers();
        }
    }
}

fn averaging_update(
    mode: &AveragingMode,
    averaged_db: &mut Vec<f32>,
    peak_hold_db: &mut Vec<f32>,
    output: &mut Vec<f32>,
    input: &[f32],
    dt_seconds: f32,
    floor: f32,
) {
    let bins = input.len();

    for buf in [&mut *averaged_db, &mut *peak_hold_db, &mut *output] {
        if buf.len() != bins {
            buf.resize(bins, floor);
        }
    }

    match mode {
        AveragingMode::None => {
            for (out, &value) in output.iter_mut().zip(input) {
                *out = value.max(floor);
            }
        }
        AveragingMode::Exponential { factor } => {
            let alpha = factor.clamp(0.0, 0.9999);
            for ((avg, out), &value) in averaged_db.iter_mut().zip(output).zip(input) {
                *avg = if *avg <= floor + f32::EPSILON {
                    value
                } else {
                    *avg * alpha + value * (1.0 - alpha)
                };
                *out = (*avg).max(floor);
            }
        }
        AveragingMode::PeakHold { decay_per_second } => {
            let decay = decay_per_second.max(0.0) * dt_seconds;
            for ((hold, out), &value) in peak_hold_db.iter_mut().zip(output).zip(input) {
                *hold = (*hold - decay).max(floor).max(value);
                *out = *hold;
            }
        }
    }
}

fn a_weight(freq_hz: f32) -> f32 {
    const MIN_DB: f32 = -80.0;
    if freq_hz <= 0.0 {
        return MIN_DB;
    }

    // IEC 61672-1:2013 reference frequencies.
    const C1: f64 = 20.598_997 * 20.598_997;
    const C2: f64 = 107.652_65 * 107.652_65;
    const C3: f64 = 737.862_23 * 737.862_23;
    const C4: f64 = 12_194.217 * 12_194.217;

    let f = freq_hz as f64;
    let f2 = f * f;
    let numerator = C4 * f2 * f2;
    let denom = (f2 + C1) * ((f2 + C2) * (f2 + C3)).sqrt() * (f2 + C4);

    if denom <= 0.0 || numerator <= 0.0 {
        return MIN_DB;
    }

    let ra = numerator / denom;
    let db = 20.0 * ra.log10() + 2.0;
    db.max(MIN_DB as f64) as f32
}

#[cfg(test)]
mod tests {
    use super::{SpectrumConfig, SpectrumProcessor, a_weight};
    use crate::dsp::{AudioProcessor, Reconfigurable};

    #[test]
    fn reset_keeps_frequency_axis_initialized() {
        let mut p = SpectrumProcessor::new(SpectrumConfig::default());
        p.reset();

        let bins = p.snapshot.frequency_bins.len();
        assert!(bins > 0);
        assert_eq!(p.snapshot.magnitudes_db.len(), bins);
        assert_eq!(p.snapshot.magnitudes_unweighted_db.len(), bins);
    }

    #[test]
    fn floor_change_reseeds_state_buffers_without_clearing_pending_audio() {
        let mut p = SpectrumProcessor::new(SpectrumConfig::default());
        p.pcm_buffer.extend([0.25, -0.25]);
        let mut cfg = p.config();
        cfg.floor_db = -96.0;

        p.update_config(cfg);

        assert_eq!(p.pcm_buffer.len(), 2);
        assert!(p.snapshot.magnitudes_db.iter().all(|&v| v == cfg.floor_db));
        assert!(p.averaged_db.iter().all(|&v| v == cfg.floor_db));
        assert!(p.peak_hold_db.iter().all(|&v| v == cfg.floor_db));
    }

    #[test]
    fn a_weight_matches_iec_reference_points() {
        let reference_points: &[(f32, f32)] = &[
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
