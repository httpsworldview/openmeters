// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::dsp::AudioBlock;
use crate::util::audio::{
    Channel, DB_FLOOR, DEFAULT_SAMPLE_RATE, FrequencyScale, LN_TO_DB, WindowKind, apply_window,
    compute_fft_bin_normalization, copy_dc_removed_from_deque, db_to_power,
    project_interleaved_channel_into, sample_rates_differ, sanitize_negative_db,
    sanitize_sample_rate, window_coefficients,
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
pub const DEFAULT_SPECTRUM_DB_FLOOR: f32 = -100.0;

const DEFAULT_SPECTRUM_HOP_DIVISOR: usize = 16;
const DEFAULT_SPECTRUM_FFT_SIZE: usize = 16_384;
const DEFAULT_SPECTRUM_EXP_FACTOR: f32 = 0.5;
const DEFAULT_SPECTRUM_PEAK_DECAY: f32 = 12.0;
const TRACE_COUNT: usize = 2;
const WEIGHTING_COUNT: usize = 2;
const A_WEIGHTED: usize = 0;
const RAW: usize = 1;

fn frequency_bins(sample_rate: f32, fft_size: usize) -> Vec<f32> {
    let bins = fft_size / 2 + 1;
    let bin_hz = sample_rate / fft_size as f32;
    (0..bins).map(|i| i as f32 * bin_hz).collect()
}

pub type SpectrumTraceSnapshot = [Vec<f32>; WEIGHTING_COUNT];

#[derive(Debug, Clone, Default)]
pub struct SpectrumSnapshot {
    pub frequency_bins: Vec<f32>,
    pub traces: [SpectrumTraceSnapshot; TRACE_COUNT],
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SpectrumConfig {
    pub sample_rate: f32,
    pub fft_size: usize,
    pub hop_size: usize,
    pub window: WindowKind,
    pub averaging: AveragingMode,
    pub source: Channel,
    pub secondary_source: Channel,
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
            window: WindowKind::Hann,
            averaging: AveragingMode::None,
            source: Channel::Mid,
            secondary_source: Channel::None,
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

impl AveragingMode {
    pub const fn default_exponential_factor() -> f32 {
        DEFAULT_SPECTRUM_EXP_FACTOR
    }

    pub const fn default_peak_decay() -> f32 {
        DEFAULT_SPECTRUM_PEAK_DECAY
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
    pcm_buffers: [VecDeque<f32>; TRACE_COUNT],
    source_scratch: Vec<f32>,
    levels: [SpectrumLevelBuffers; TRACE_COUNT],
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
            pcm_buffers: [VecDeque::new(), VecDeque::new()],
            source_scratch: Vec::new(),
            levels: Default::default(),
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
        self.pcm_buffers.iter_mut().for_each(VecDeque::clear);
    }

    fn reset_level_buffers(&mut self) {
        let bins = self.config.fft_size / 2 + 1;
        let floor = self.config.floor_db;
        for trace in &mut self.snapshot.traces {
            for db in trace { reset_to_floor(db, bins, floor); }
        }
        for buffers in &mut self.levels { buffers.reset(bins); }
    }

    fn sources(&self) -> [Channel; TRACE_COUNT] {
        [self.config.source, self.config.secondary_source]
    }

    fn active_traces(&self) -> [bool; TRACE_COUNT] {
        let [primary, secondary] = self.sources();
        [primary != Channel::None, secondary != Channel::None && secondary != primary]
    }

    fn process_ready_windows(&mut self, timestamp: Instant) -> bool {
        let fft_size = self.config.fft_size;
        let hop = self.config.hop_size.max(1);
        let bins = fft_size / 2 + 1;
        let floor = self.config.floor_db;
        let active = self.active_traces();
        let mut produced = false;

        debug_assert_eq!(self.a_weighting_db.len(), bins);
        if !active.iter().any(|&active| active) { return false; }

        while (0..TRACE_COUNT).all(|trace| !active[trace] || self.pcm_buffers[trace].len() >= fft_size) {
            let dt_seconds = self.last_update_at.map_or(0.0, |last| {
                timestamp.saturating_duration_since(last).as_secs_f32()
            });
            for (trace, &active) in active.iter().enumerate() {
                if active && !self.process_trace_window(trace, dt_seconds, floor) {
                    return produced;
                }
            }
            self.last_update_at = Some(timestamp);
            for (trace, &active) in active.iter().enumerate() {
                if active {
                    let buf = &mut self.pcm_buffers[trace];
                    buf.drain(..hop.min(buf.len()));
                }
            }
            produced = true;
        }

        produced
    }

    fn process_trace_window(&mut self, trace: usize, dt_seconds: f32, floor: f32) -> bool {
        let bins = self.config.fft_size / 2 + 1;
        copy_dc_removed_from_deque(&mut self.real_buffer, &self.pcm_buffers[trace]);
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
            return false;
        }

        let level = &mut self.levels[trace];
        let snapshot = &mut self.snapshot.traces[trace];
        for (idx, (complex, norm)) in self
            .spectrum_buffer
            .iter()
            .zip(&self.bin_normalization)
            .take(bins)
            .enumerate()
        {
            level.scratch_power[idx] = complex.norm_sqr() * *norm;
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

    pub fn process_block(&mut self, block: &AudioBlock<'_>) -> Option<SpectrumSnapshot> {
        if block.is_empty() { return None; }

        if sample_rates_differ(block.sample_rate, self.config.sample_rate) {
            self.config.sample_rate = block.sample_rate;
            self.reset_buffers();
        }

        if self.real_buffer.len() != self.config.fft_size {
            self.rebuild_fft();
        }
        self.push_sources(block);

        if self.process_ready_windows(block.timestamp) {
            Some(self.snapshot.clone())
        } else {
            None
        }
    }

    fn push_sources(&mut self, block: &AudioBlock<'_>) {
        let active = self.active_traces();
        for (idx, source) in self.sources().into_iter().enumerate().filter(|(idx, _)| active[*idx]) {
            if project_interleaved_channel_into(
                &mut self.source_scratch,
                block.samples,
                block.channels,
                block.frame_count(),
                source,
            ) {
                self.pcm_buffers[idx].extend(&self.source_scratch);
            }
        }
    }

    pub fn update_config(&mut self, mut config: SpectrumConfig) {
        let old = self.config;
        config.normalize();
        self.config = config;
        if old.fft_size != config.fft_size || old.window != config.window {
            self.rebuild_fft();
        } else if sample_rates_differ(old.sample_rate, config.sample_rate)
            || old.source != config.source
            || old.secondary_source != config.secondary_source
        {
            self.reset_buffers();
        } else if (old.floor_db - config.floor_db).abs() > f32::EPSILON {
            self.reset_level_buffers();
        }
    }
}

#[derive(Default)]
struct SpectrumLevelBuffers {
    averaged_power: Vec<f32>,
    peak_hold_power: Vec<f32>,
    scratch_power: Vec<f32>,
}

impl SpectrumLevelBuffers {
    fn reset(&mut self, bins: usize) {
        reset_to_floor(&mut self.averaged_power, bins, 0.0);
        reset_to_floor(&mut self.peak_hold_power, bins, 0.0);
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
                for (avg, &power) in self.averaged_power.iter_mut().zip(&self.scratch_power) {
                    *avg = if *avg <= 0.0 { power } else { *avg * alpha + power * (1.0 - alpha) };
                }
                &self.averaged_power
            }
            AveragingMode::PeakHold { decay_per_second } => {
                let decay = db_to_power(-decay_per_second.max(0.0) * dt_seconds);
                for (hold, &power) in self.peak_hold_power.iter_mut().zip(&self.scratch_power) {
                    *hold = (*hold * decay).max(power);
                }
                &self.peak_hold_power
            }
        };
        let (weighted, raw) = outputs.split_at_mut(RAW);
        let (weighted_out, raw_out) = (&mut weighted[A_WEIGHTED], &mut raw[0]);
        for i in 0..bins {
            let db = powers[i].ln() * LN_TO_DB;
            raw_out[i] = db.max(floor);
            weighted_out[i] = (db + weighting_db[i]).max(floor);
        }
    }
}

fn reset_to_floor(buf: &mut Vec<f32>, bins: usize, floor: f32) {
    buf.clear();
    buf.resize(bins, floor);
}

fn a_weight(freq_hz: f32) -> f32 {
    const MIN_DB: f32 = -80.0;
    if freq_hz <= 0.0 { return MIN_DB; }

    const C1: f64 = 20.598_997 * 20.598_997;
    const C2: f64 = 107.652_65 * 107.652_65;
    const C3: f64 = 737.862_23 * 737.862_23;
    const C4: f64 = 12_194.217 * 12_194.217;

    let f = freq_hz as f64;
    let f2 = f * f;
    let numerator = C4 * f2 * f2;
    let denom = (f2 + C1) * ((f2 + C2) * (f2 + C3)).sqrt() * (f2 + C4);

    if denom <= 0.0 || numerator <= 0.0 { return MIN_DB; }

    let ra = numerator / denom;
    let db = 20.0 * ra.log10() + 2.0;
    db.max(MIN_DB as f64) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::AudioBlock;
    use std::time::Instant;

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
        p.pcm_buffers[0].extend([0.25, -0.25]);
        let mut cfg = p.config();
        cfg.floor_db = -96.0;

        p.update_config(cfg);

        assert_eq!(p.pcm_buffers[0].len(), 2);
        for output in &p.snapshot.traces[0] {
            assert!(output.iter().all(|&v| v == cfg.floor_db));
        }
        let bins = cfg.fft_size / 2 + 1;
        for buffers in &p.levels {
            assert_eq!(buffers.scratch_power.len(), bins);
            assert!(buffers.scratch_power.iter().all(|&v| v == 0.0));
            assert!(buffers.averaged_power.iter().all(|&v| v == 0.0));
            assert!(buffers.peak_hold_power.iter().all(|&v| v == 0.0));
        }
    }

    #[test]
    fn configured_sources_are_projected_before_fft() {
        let mut p = SpectrumProcessor::new(SpectrumConfig {
            fft_size: 8,
            source: Channel::Left,
            secondary_source: Channel::Side,
            ..Default::default()
        });
        let samples = [1.0, 0.0, 0.0, 1.0];
        p.process_block(&AudioBlock::new(&samples, 2, p.config.sample_rate, Instant::now()));

        assert_eq!(p.pcm_buffers[0].iter().copied().collect::<Vec<_>>(), [1.0, 0.0]);
        assert_eq!(p.pcm_buffers[1].iter().copied().collect::<Vec<_>>(), [0.5, -0.5]);
    }

    #[test]
    fn secondary_source_can_drive_processing_without_primary() {
        let mut p = SpectrumProcessor::new(SpectrumConfig {
            fft_size: 8,
            hop_size: 8,
            source: Channel::None,
            secondary_source: Channel::Left,
            ..Default::default()
        });
        let samples = vec![0.0; 8];

        let snap = p.process_block(&AudioBlock::new(&samples, 1, p.config.sample_rate, Instant::now()));

        assert!(snap.is_some());
    }

    #[test]
    fn fft_size_update_resizes_scratch_before_processing() {
        let mut p = SpectrumProcessor::new(SpectrumConfig {
            fft_size: 128,
            hop_size: 128,
            ..Default::default()
        });
        let mut cfg = p.config();
        cfg.fft_size = 256;
        cfg.hop_size = 256;
        p.update_config(cfg);

        let cfg = p.config();
        let bins = cfg.fft_size / 2 + 1;
        for buffers in &p.levels {
            assert_eq!(buffers.scratch_power.len(), bins);
        }

        let samples = vec![0.0; cfg.fft_size];
        let lengths = p
            .process_block(&AudioBlock::new(&samples, 1, cfg.sample_rate, Instant::now()))
            .map(|s| (s.traces[0][A_WEIGHTED].len(), s.traces[0][RAW].len()));
        assert_eq!(lengths, Some((bins, bins)));
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
