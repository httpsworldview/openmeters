// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::dsp::AudioBlock;
use crate::util::audio::{DEFAULT_SAMPLE_RATE, WindowKind, apply_window, mixdown_into_deque, window_coefficients};
use realfft::{RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex32;
use std::collections::VecDeque;
use std::sync::Arc;

pub const NUM_PITCH_CLASSES: usize = 12;

pub const NOTE_NAMES: [&str; NUM_PITCH_CLASSES] =
    ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

// A4 = 440 Hz -> C0 = 440 * 2^(-69/12)
const C0_HZ: f64 = 16.351_597_831_287_4;

pub const DEFAULT_FFT_SIZE: usize = 4096;
pub const DEFAULT_HOP_SIZE: usize = 1024;

// full piano range, A0–C8
pub const DEFAULT_MIN_FREQ_HZ: f32 = 27.5;
pub const DEFAULT_MAX_FREQ_HZ: f32 = 4186.0;

pub const MIN_SMOOTHING: f32 = 0.01;
pub const MAX_SMOOTHING: f32 = 0.5;
pub const MIN_FLOOR_DB: f32 = -100.0;
pub const MAX_FLOOR_DB: f32 = -20.0;

#[derive(Debug, Clone, Copy)]
pub struct ChromaConfig {
    pub sample_rate: f32,
    pub fft_size: usize,
    pub hop_size: usize,
    pub min_freq_hz: f32,
    pub max_freq_hz: f32,
    /// 0 = slow, 1 = instant
    pub smoothing: f32,
    pub floor_db: f32,
    /// < 1.0, applied every hop
    pub peak_decay: f32,
}

impl Default for ChromaConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            fft_size: DEFAULT_FFT_SIZE,
            hop_size: DEFAULT_HOP_SIZE,
            min_freq_hz: DEFAULT_MIN_FREQ_HZ,
            max_freq_hz: DEFAULT_MAX_FREQ_HZ,
            smoothing: 0.07,
            floor_db: -80.0,
            peak_decay: 0.998,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ChromaSnapshot {
    pub bins: [f32; NUM_PITCH_CLASSES],
    pub peak_bins: [f32; NUM_PITCH_CLASSES],
}

// hz -> 0..11 (C=0, B=11), None if <= 0 or non-finite
fn freq_to_pitch_class(hz: f32) -> Option<usize> {
    if hz <= 0.0 || !hz.is_finite() {
        return None;
    }
    let p = 12.0 * (hz as f64 / C0_HZ).log2();
    let pc = p.round().rem_euclid(12.0) as usize;
    Some(pc)
}

pub struct ChromaProcessor {
    config: ChromaConfig,
    fft: Arc<dyn RealToComplex<f32>>,
    fft_input: Vec<f32>,
    fft_output: Vec<Complex32>,
    scratch: Vec<Complex32>,
    window: Arc<[f32]>,
    mono_buf: VecDeque<f32>,
    // bin -> pitch class, None if outside freq range
    bin_class: Box<[Option<usize>]>,
    smoothed_bins: [f32; NUM_PITCH_CLASSES],
    peak_bins: [f32; NUM_PITCH_CLASSES],
    snapshot: ChromaSnapshot,
}

impl ChromaProcessor {
    pub fn new(config: ChromaConfig) -> Self {
        let fft_size = config.fft_size.next_power_of_two().max(64);
        let fft = RealFftPlanner::new().plan_fft_forward(fft_size);
        let window = window_coefficients(WindowKind::Hann, fft_size);
        let bin_class = Self::precompute_bin_classes(
            fft_size,
            config.sample_rate,
            config.min_freq_hz,
            config.max_freq_hz,
        );
        let fft_input = fft.make_input_vec();
        let fft_output = fft.make_output_vec();
        let scratch = fft.make_scratch_vec();
        Self {
            config,
            fft,
            fft_input,
            fft_output,
            scratch,
            window,
            mono_buf: VecDeque::new(),
            bin_class,
            smoothed_bins: [0.0; NUM_PITCH_CLASSES],
            peak_bins: [0.0; NUM_PITCH_CLASSES],
            snapshot: ChromaSnapshot::default(),
        }
    }

    pub fn config(&self) -> ChromaConfig {
        self.config
    }

    pub fn update_config(&mut self, config: ChromaConfig) {
        let rebuild = config.fft_size != self.config.fft_size
            || (config.sample_rate - self.config.sample_rate).abs() > f32::EPSILON
            || (config.min_freq_hz - self.config.min_freq_hz).abs() > 0.01
            || (config.max_freq_hz - self.config.max_freq_hz).abs() > 0.01;
        self.config = config;
        if rebuild {
            self.rebuild();
        }
    }

    fn rebuild(&mut self) {
        let fft_size = self.config.fft_size.next_power_of_two().max(64);
        let fft = RealFftPlanner::new().plan_fft_forward(fft_size);
        self.window = window_coefficients(WindowKind::Hann, fft_size);
        self.bin_class = Self::precompute_bin_classes(
            fft_size,
            self.config.sample_rate,
            self.config.min_freq_hz,
            self.config.max_freq_hz,
        );
        self.fft_input = fft.make_input_vec();
        self.fft_output = fft.make_output_vec();
        self.scratch = fft.make_scratch_vec();
        self.fft = fft;
        self.mono_buf.clear();
        self.smoothed_bins = [0.0; NUM_PITCH_CLASSES];
        self.peak_bins = [0.0; NUM_PITCH_CLASSES];
    }

    fn precompute_bin_classes(
        fft_size: usize,
        sample_rate: f32,
        min_freq: f32,
        max_freq: f32,
    ) -> Box<[Option<usize>]> {
        let spectrum_len = fft_size / 2 + 1;
        let bin_hz = sample_rate / fft_size as f32;
        (0..spectrum_len)
            .map(|b| {
                let hz = b as f32 * bin_hz;
                if hz < min_freq || hz > max_freq {
                    return None;
                }
                freq_to_pitch_class(hz)
            })
            .collect()
    }

    pub fn process_block(&mut self, block: &AudioBlock<'_>) -> Option<ChromaSnapshot> {
        if block.frame_count() == 0 {
            return None;
        }

        let sample_rate = block.sample_rate.max(1.0);
        if (sample_rate - self.config.sample_rate).abs() > f32::EPSILON {
            self.config.sample_rate = sample_rate;
            self.rebuild();
        }

        mixdown_into_deque(&mut self.mono_buf, block.samples, block.channels.max(1));

        let fft_size = self.config.fft_size.next_power_of_two().max(64);
        let hop_size = self.config.hop_size.clamp(1, fft_size);
        let mut any = false;

        while self.mono_buf.len() >= fft_size {
            self.fill_and_window(fft_size);
            if self
                .fft
                .process_with_scratch(
                    &mut self.fft_input,
                    &mut self.fft_output,
                    &mut self.scratch,
                )
                .is_ok()
            {
                self.accumulate_spectrum(fft_size);
            }
            self.mono_buf.drain(..hop_size);
            any = true;
        }

        if any {
            self.snapshot.bins = self.smoothed_bins;
            self.snapshot.peak_bins = self.peak_bins;
        }
        Some(self.snapshot)
    }

    fn fill_and_window(&mut self, fft_size: usize) {
        let window = self.window.clone();
        for (i, s) in self.mono_buf.iter().take(fft_size).enumerate() {
            self.fft_input[i] = *s;
        }
        apply_window(&mut self.fft_input[..fft_size], &window);
    }

    fn accumulate_spectrum(&mut self, fft_size: usize) {
        let floor_power = 10.0_f32.powf(self.config.floor_db / 10.0);
        let scale = 1.0 / (fft_size as f32 * fft_size as f32);

        let mut raw = [0.0_f32; NUM_PITCH_CLASSES];
        let mut count = [0_u32; NUM_PITCH_CLASSES];

        for (b, &pc_opt) in self.bin_class.iter().enumerate() {
            let Some(pc) = pc_opt else { continue };
            let power = self.fft_output[b].norm_sqr() * scale;
            raw[pc] += power;
            count[pc] += 1;
        }

        // avg power per class
        for i in 0..NUM_PITCH_CLASSES {
            if count[i] > 0 {
                raw[i] /= count[i] as f32;
            }
        }

        // normalize to loudest, floor at floor_power
        let peak_raw = raw.iter().cloned().fold(floor_power, f32::max);
        let normalized: [f32; NUM_PITCH_CLASSES] =
            std::array::from_fn(|i| (raw[i] / peak_raw).clamp(0.0, 1.0).sqrt());

        let alpha = self.config.smoothing.clamp(MIN_SMOOTHING, MAX_SMOOTHING);
        let decay = self.config.peak_decay.clamp(0.9, 1.0);

        for ((smoothed, peak), norm) in self
            .smoothed_bins
            .iter_mut()
            .zip(self.peak_bins.iter_mut())
            .zip(normalized.iter())
        {
            *smoothed += alpha * (norm - *smoothed);
            *peak = (*peak * decay).max(*smoothed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    fn block(samples: &[f32], channels: usize) -> AudioBlock<'_> {
        AudioBlock::now(samples, channels, RATE)
    }

    #[test]
    fn pitch_class_mapping() {
        assert_eq!(freq_to_pitch_class(261.63), Some(0)); // C4 -> C
        assert_eq!(freq_to_pitch_class(440.0), Some(9));  // A4 -> A
        assert_eq!(freq_to_pitch_class(659.26), Some(4)); // E5 -> E
    }

    #[test]
    fn process_sine_wave_returns_snapshot() {
        let mut proc = ChromaProcessor::new(ChromaConfig::default());
        let samples: Vec<f32> = (0..DEFAULT_FFT_SIZE * 2)
            .map(|n| (2.0 * std::f32::consts::PI * 440.0 * n as f32 / RATE).sin())
            .collect();
        let snap = proc.process_block(&block(&samples, 1));
        assert!(snap.is_some());
    }

    #[test]
    fn a440_activates_a_pitch_class() {
        let mut proc = ChromaProcessor::new(ChromaConfig {
            smoothing: 1.0,
            ..Default::default()
        });
        let n_samples = DEFAULT_FFT_SIZE * 4;
        let samples: Vec<f32> = (0..n_samples)
            .map(|n| (2.0 * std::f32::consts::PI * 440.0 * n as f32 / RATE).sin())
            .collect();
        let snap = proc.process_block(&block(&samples, 1)).unwrap();
        // A => class 9, should win
        let a_class = snap.bins[9];
        let max_other = snap
            .bins
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != 9)
            .map(|(_, &v)| v)
            .fold(0.0_f32, f32::max);
        assert!(
            a_class > max_other,
            "A class ({a_class:.3}) should dominate other classes (max {max_other:.3})"
        );
    }
}
