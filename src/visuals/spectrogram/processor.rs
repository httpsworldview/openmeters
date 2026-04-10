// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

// Spectrogram DSP - Time-frequency analysis with reassignment
//
// # References
// 1. F. Auger and P. Flandrin, "Improving the readability of time-frequency and
//    time-scale representations by the reassignment method", IEEE Trans. SP,
//    vol. 43, no. 5, pp. 1068-1089, May 1995.
// 2. K. Kodera, R. Gendrin & C. de Villedary, "Analysis of time-varying signals
//    with small BT values", IEEE Trans. ASSP, vol. 26, no. 1, pp. 64-76, Feb 1978.
// 3. F. Auger et al., "Time-Frequency Reassignment and Synchrosqueezing: An
//    Overview", IEEE Signal Processing Magazine, vol. 30, pp. 32-41, Nov 2013.
// 4. T.J. Gardner and M.O. Magnasco, "Sparse time-frequency representations",
//    PNAS, vol. 103, no. 16, pp. 6094-6099, Apr 2006.
// 5. K.R. Fitz and S.A. Fulop, "A Unified Theory of Time-Frequency Reassignment",
//    arXiv:0903.3080 [cs.SD], Mar 2009.
// 6. S.A. Fulop and K. Fitz, "Algorithms for computing the time-corrected
//    instantaneous frequency (reassigned) spectrogram, with applications",
//    JASA, vol. 119, pp. 360-371, Jan 2006.
// 7. D.J. Nelson, "Cross-spectral methods for processing speech",
//    JASA, vol. 110, no. 5, pp. 2575-2592, Nov 2001.

use crate::dsp::{AudioBlock, AudioProcessor, Reconfigurable};
use crate::util::audio::{
    DB_FLOOR, DEFAULT_SAMPLE_RATE, LN_TO_DB, copy_from_deque, db_to_power, erb_rate_to_hz,
    hz_to_erb_rate,
};
use bytemuck::{Pod, Zeroable};
use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::VecDeque;
use std::hash::Hash;
use std::sync::RwLock;
use std::sync::{Arc, OnceLock};
use wide::{CmpGe, CmpGt, f32x8};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct SpectrogramPoint {
    pub time_offset: f32,
    pub freq_hz: f32,
    pub magnitude_db: f32,
}

impl SpectrogramPoint {
    pub const SENTINEL: Self = Self {
        time_offset: 0.0,
        freq_hz: 0.0,
        magnitude_db: f32::NEG_INFINITY,
    };
}

#[derive(Debug, Clone, Copy)]
pub struct SpectrogramConfig {
    pub sample_rate: f32,
    pub fft_size: usize,
    pub hop_size: usize,
    pub window: WindowKind,
    pub frequency_scale: FrequencyScale,
    pub history_length: usize,
    pub use_reassignment: bool,
    pub zero_padding_factor: usize,
}

impl Default for SpectrogramConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            fft_size: 2048,
            hop_size: 64,
            window: WindowKind::Blackman,
            frequency_scale: FrequencyScale::default(),
            history_length: 0,
            use_reassignment: true,
            zero_padding_factor: 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FrequencyScale {
    Linear,
    #[default]
    Logarithmic,
    #[serde(alias = "mel")]
    Erb,
}

impl FrequencyScale {
    #[inline]
    pub fn freq_at(self, min: f32, max: f32, t: f32) -> f32 {
        match self {
            Self::Linear => min + (max - min) * t,
            Self::Logarithmic => {
                let min = min.max(1e-6);
                // decomposed from powf so LLVM can hoist the ln when
                // min/max are loop-invariant
                min * (t * (max / min).max(1.0).ln()).exp()
            }
            Self::Erb => {
                let erb_min = hz_to_erb_rate(min);
                erb_rate_to_hz(erb_min + (hz_to_erb_rate(max) - erb_min) * t)
            }
        }
    }

    #[inline]
    pub fn pos_of(self, min: f32, max: f32, freq: f32) -> f32 {
        match self {
            Self::Linear => (freq - min) / (max - min).max(1e-6),
            Self::Logarithmic => {
                let min = min.max(1e-6);
                let ratio = max / min;
                if ratio <= 1.0 {
                    return 0.0;
                }
                (freq / min).ln() / ratio.ln()
            }
            Self::Erb => {
                let erb_min = hz_to_erb_rate(min);
                (hz_to_erb_rate(freq) - erb_min) / (hz_to_erb_rate(max) - erb_min).max(1e-6)
            }
        }
    }
}

impl std::fmt::Display for FrequencyScale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    Rectangular,
    Hann,
    Hamming,
    Blackman,
    BlackmanHarris,
}

impl WindowKind {
    pub const ALL: [Self; 5] = [
        Self::Rectangular,
        Self::Hann,
        Self::Hamming,
        Self::Blackman,
        Self::BlackmanHarris,
    ];

    pub(crate) fn coefficients(self, len: usize) -> Vec<f32> {
        if len <= 1 {
            return vec![1.0; len];
        }
        match self {
            Self::Rectangular => vec![1.0; len],
            Self::Hann => cosine_window(len, &[0.5, -0.5]),
            Self::Hamming => cosine_window(len, &[25.0 / 46.0, -21.0 / 46.0]),
            Self::Blackman => cosine_window(len, &[0.42, -0.5, 0.08]),
            Self::BlackmanHarris => cosine_window(len, &[0.35875, -0.48829, 0.14128, -0.01168]),
        }
    }
}

impl std::fmt::Display for WindowKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Rectangular => "Rectangular",
            Self::Hann => "Hann",
            Self::Hamming => "Hamming",
            Self::Blackman => "Blackman",
            Self::BlackmanHarris => "Blackman-Harris",
        })
    }
}

fn cosine_window(len: usize, coeffs: &[f32]) -> Vec<f32> {
    let step = core::f32::consts::TAU / (len.saturating_sub(1).max(1) as f32);
    (0..len)
        .map(|n| {
            let phi = n as f32 * step;
            coeffs
                .iter()
                .enumerate()
                .fold(0.0, |sum, (k, &c)| sum + c * (phi * k as f32).cos())
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct WindowKey {
    kind: WindowKind,
    len: usize,
}

struct WindowCache(RwLock<HashMap<WindowKey, Arc<[f32]>>>);

impl WindowCache {
    fn get(kind: WindowKind, len: usize) -> Arc<[f32]> {
        static INSTANCE: OnceLock<WindowCache> = OnceLock::new();
        let cache = INSTANCE.get_or_init(|| WindowCache(RwLock::new(HashMap::default())));
        if len == 0 {
            return Arc::from([]);
        }
        // SAFETY: lock cannot be poisoned as panic is set to abort in
        // cargo.toml.
        let key = WindowKey { kind, len };
        if let Some(cached) = cache.0.read().unwrap().get(&key) {
            return cached.clone();
        }
        cache
            .0
            .write()
            .unwrap()
            .entry(key)
            .or_insert_with(|| Arc::from(kind.coefficients(len)))
            .clone()
    }
}

#[derive(Default)]
struct ReassignmentBuffers {
    derivative_window: Vec<f32>,
    time_weighted_window: Vec<f32>,
    derivative_spectrum: Vec<Complex32>,
    time_weighted_spectrum: Vec<Complex32>,
    floor_linear: f32,
}

impl ReassignmentBuffers {
    fn rebuild(&mut self, window: &[f32], bin_count: usize) {
        self.derivative_window = compute_derivative_spectral(window);
        self.time_weighted_window = compute_time_weighted(window);
        self.derivative_spectrum = vec![Complex32::ZERO; bin_count];
        self.time_weighted_spectrum = vec![Complex32::ZERO; bin_count];
        self.floor_linear = db_to_power(DB_FLOOR);
    }
}

#[derive(Debug, Clone)]
pub struct SpectrogramColumn {
    pub points: Vec<SpectrogramPoint>,
}

#[derive(Debug, Clone)]
pub struct SpectrogramUpdate {
    pub fft_size: usize,
    pub hop_size: usize,
    pub sample_rate: f32,
    pub frequency_scale: FrequencyScale,
    pub history_length: usize,
    pub reset: bool,
    pub points_per_column: usize,
    pub new_columns: Vec<SpectrogramColumn>,
}

pub struct SpectrogramProcessor {
    config: SpectrogramConfig,
    planner: FftPlanner<f32>,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    window_size: usize,
    fft_size: usize,
    window: Arc<[f32]>,
    real: Vec<f32>,
    analytic: Vec<Complex32>,
    complex_buf: Vec<Complex32>,
    spectrum: Vec<Complex32>,
    scratch: Vec<Complex32>,
    magnitudes: Vec<f32>,
    reassign: ReassignmentBuffers,
    bin_norm: Vec<f32>,
    audio_buffer: VecDeque<f32>,
    points_buf: Vec<SpectrogramPoint>,
    bin_hz: f32,
    reset: bool,
}

impl SpectrogramProcessor {
    pub fn new(cfg: SpectrogramConfig) -> Self {
        let mut planner = FftPlanner::new();
        let placeholder_fft = planner.plan_fft_forward(1024);
        let placeholder_ifft = planner.plan_fft_inverse(1024);
        let mut processor = Self {
            config: cfg,
            planner,
            fft: placeholder_fft,
            ifft: placeholder_ifft,
            window_size: 0,
            fft_size: 0,
            window: Arc::from([]),
            real: vec![],
            analytic: vec![],
            complex_buf: vec![],
            spectrum: vec![],
            scratch: vec![],
            magnitudes: vec![],
            reassign: ReassignmentBuffers::default(),
            bin_norm: vec![],
            audio_buffer: VecDeque::new(),
            points_buf: vec![],
            bin_hz: 0.0,
            reset: true,
        };
        processor.rebuild_fft();
        processor
    }

    pub fn config(&self) -> SpectrogramConfig {
        self.config
    }

    fn needs_fft_rebuild(&self) -> bool {
        self.window_size != self.config.fft_size
            || self.fft_size != self.config.fft_size * self.config.zero_padding_factor.max(1)
            || WindowCache::get(self.config.window, self.config.fft_size).as_ptr()
                != self.window.as_ptr()
    }

    fn rebuild_fft(&mut self) {
        self.window_size = self.config.fft_size;
        self.fft_size = self.window_size * self.config.zero_padding_factor.max(1);
        self.fft = self.planner.plan_fft_forward(self.fft_size);
        self.ifft = self.planner.plan_fft_inverse(self.fft_size);
        self.window = WindowCache::get(self.config.window, self.window_size);
        self.real.resize(self.fft_size, 0.0);
        self.analytic.resize(self.fft_size, Complex32::ZERO);
        self.complex_buf.resize(self.fft_size, Complex32::ZERO);
        let bin_count = self.fft_size / 2 + 1;
        self.spectrum.resize(bin_count, Complex32::ZERO);
        self.scratch.resize(
            self.fft
                .get_inplace_scratch_len()
                .max(self.ifft.get_inplace_scratch_len()),
            Complex32::ZERO,
        );
        self.magnitudes.resize(bin_count, 0.0);
        self.reassign.rebuild(&self.window, bin_count);
        self.bin_norm =
            crate::util::audio::compute_fft_bin_normalization(&self.window, self.fft_size);
        self.audio_buffer.truncate(self.window_size * 2);
        self.bin_hz = self.config.sample_rate / self.fft_size.max(1) as f32;
        self.points_buf
            .resize(bin_count, SpectrogramPoint::SENTINEL);
    }

    fn process_ready_windows(&mut self) -> Vec<SpectrogramColumn> {
        if self.window_size == 0 {
            return vec![];
        }
        let (hop_size, sample_rate) = (self.config.hop_size, self.config.sample_rate);
        let reassignment_enabled = self.config.use_reassignment && sample_rate > f32::EPSILON;
        let bin_count = self.fft_size / 2 + 1;
        let mut output = Vec::new();

        while self.audio_buffer.len() >= self.window_size {
            copy_from_deque(&mut self.real[..self.window_size], &self.audio_buffer);
            crate::util::audio::remove_dc(&mut self.real[..self.window_size]);

            if reassignment_enabled {
                hilbert_transform(
                    &self.real[..self.window_size],
                    &mut self.analytic,
                    &*self.fft,
                    &*self.ifft,
                    &mut self.scratch,
                );
                fft_windowed(
                    &self.analytic,
                    &self.window,
                    &mut self.complex_buf,
                    &mut self.spectrum,
                    &*self.fft,
                    &mut self.scratch,
                );
                fft_windowed(
                    &self.analytic,
                    &self.reassign.derivative_window,
                    &mut self.complex_buf,
                    &mut self.reassign.derivative_spectrum,
                    &*self.fft,
                    &mut self.scratch,
                );
                fft_windowed(
                    &self.analytic,
                    &self.reassign.time_weighted_window,
                    &mut self.complex_buf,
                    &mut self.reassign.time_weighted_spectrum,
                    &*self.fft,
                    &mut self.scratch,
                );
                self.emit_reassigned_points(sample_rate, hop_size, bin_count);
            } else {
                for (c, (&r, &w)) in self
                    .complex_buf
                    .iter_mut()
                    .zip(self.real.iter().zip(self.window.iter()))
                {
                    *c = Complex32::new(r * w, 0.0);
                }
                self.complex_buf[self.window_size..].fill(Complex32::ZERO);
                self.fft
                    .process_with_scratch(&mut self.complex_buf, &mut self.scratch);
                self.spectrum
                    .copy_from_slice(&self.complex_buf[..bin_count]);
                self.compute_standard_magnitudes(bin_count);
                let bin_hz = self.bin_hz;
                for (k, pt) in self.points_buf.iter_mut().enumerate() {
                    *pt = SpectrogramPoint {
                        time_offset: 0.0,
                        freq_hz: k as f32 * bin_hz,
                        magnitude_db: self.magnitudes[k],
                    };
                }
            };

            output.push(SpectrogramColumn {
                points: self.points_buf.clone(),
            });
            self.audio_buffer
                .drain(..hop_size.min(self.audio_buffer.len()));
        }
        output
    }

    fn compute_standard_magnitudes(&mut self, bin_count: usize) {
        let v_floor = f32x8::splat(DB_FLOOR);
        let v_ln_to_db = f32x8::splat(LN_TO_DB);
        let v_eps = f32x8::splat(1.0e-20);

        for chunk in 0..bin_count.div_ceil(8) {
            let off = chunk * 8;
            let (re, im) = load_complex_simd(&self.spectrum, off);
            let norm = load_f32_simd(&self.bin_norm, off);
            let power = (re * re + im * im) * norm;
            let valid = power.simd_gt(v_eps);
            let db = (power.max(v_eps).ln() * v_ln_to_db).max(v_floor);
            let result = valid.blend(db, v_floor).to_array();
            let count = (bin_count.saturating_sub(off)).min(8);
            self.magnitudes[off..off + count].copy_from_slice(&result[..count]);
        }
    }

    fn emit_reassigned_points(&mut self, sample_rate: f32, hop_size: usize, bin_count: usize) {
        let bin_hz = self.bin_hz;
        let min_hz = bin_hz;
        let max_hz = sample_rate * 0.5;
        let floor_linear = self.reassign.floor_linear;
        let inv_2pi = sample_rate / core::f32::consts::TAU;
        let inv_hop = 1.0 / hop_size.max(1) as f32;

        let v_eps = f32x8::splat(f32::MIN_POSITIVE);
        let v_floor = f32x8::splat(floor_linear);
        let v_bin_hz = f32x8::splat(bin_hz);
        let v_inv_2pi = f32x8::splat(inv_2pi);
        let v_min_hz = f32x8::splat(min_hz);
        let v_max_hz = f32x8::splat(max_hz);
        let v_inv_hop = f32x8::splat(inv_hop);
        let v_db_floor = f32x8::splat(DB_FLOOR);
        let v_ln_to_db = f32x8::splat(LN_TO_DB);

        for chunk in 0..bin_count.div_ceil(8) {
            let off = chunk * 8;
            let k_idx = f32x8::new(std::array::from_fn(|j| (off + j) as f32));

            let (base_re, base_im) = load_complex_simd(&self.spectrum, off);
            let (d_re, d_im) = load_complex_simd(&self.reassign.derivative_spectrum, off);
            let (t_re, t_im) = load_complex_simd(&self.reassign.time_weighted_spectrum, off);
            let energy_scale = load_f32_simd(&self.bin_norm, off);

            let pow = base_re * base_re + base_im * base_im;
            let mask = pow.simd_ge(v_floor) & energy_scale.simd_gt(f32x8::splat(0.0));

            let inv_pow = f32x8::splat(1.0) / pow.max(v_eps);
            let d_omega = -(d_im * base_re - d_re * base_im) * inv_pow;
            let f_corr = d_omega * v_inv_2pi;

            let freq = k_idx.mul_add(v_bin_hz, f_corr);

            let final_mask =
                mask & freq.simd_ge(v_min_hz) & (v_max_hz - freq).simd_gt(f32x8::splat(0.0));

            let d_tau = (t_re * base_re + t_im * base_im) * inv_pow;

            let time_offset = d_tau * v_inv_hop;
            let mag_db = (pow.max(v_eps) * energy_scale.max(v_eps)).ln() * v_ln_to_db;
            let mag_db = mag_db.max(v_db_floor);

            let freqs = freq.to_array();
            let times = time_offset.to_array();
            let mags = mag_db.to_array();
            let masks = final_mask
                .blend(f32x8::splat(1.0), f32x8::splat(0.0))
                .to_array();

            let count = (bin_count.saturating_sub(off)).min(8);
            for i in 0..count {
                self.points_buf[off + i] = if masks[i] != 0.0 {
                    SpectrogramPoint {
                        time_offset: times[i],
                        freq_hz: freqs[i],
                        magnitude_db: mags[i],
                    }
                } else {
                    SpectrogramPoint::SENTINEL
                };
            }
        }
    }
}

impl AudioProcessor for SpectrogramProcessor {
    type Output = SpectrogramUpdate;

    fn process_block(&mut self, block: &AudioBlock<'_>) -> Option<Self::Output> {
        if block.frame_count() == 0 || block.channels == 0 {
            return None;
        }
        if self.config.sample_rate <= 0.0
            || (self.config.sample_rate - block.sample_rate).abs() > f32::EPSILON
        {
            self.config.sample_rate = block.sample_rate;
            self.rebuild_fft();
            self.reset = true;
        } else if self.needs_fft_rebuild() {
            self.rebuild_fft();
            self.reset = true;
        }
        crate::util::audio::mixdown_into_deque(
            &mut self.audio_buffer,
            block.samples,
            block.channels,
        );
        let cols = self.process_ready_windows();
        let bin_count = self.fft_size / 2 + 1;
        if cols.is_empty() {
            None
        } else {
            Some(SpectrogramUpdate {
                fft_size: self.fft_size,
                hop_size: self.config.hop_size,
                sample_rate: self.config.sample_rate,
                frequency_scale: self.config.frequency_scale,
                history_length: self.config.history_length,
                reset: std::mem::take(&mut self.reset),
                points_per_column: bin_count,
                new_columns: cols,
            })
        }
    }

    fn reset(&mut self) {
        self.audio_buffer.clear();
        self.reset = true;
    }
}

impl Reconfigurable<SpectrogramConfig> for SpectrogramProcessor {
    fn update_config(&mut self, cfg: SpectrogramConfig) {
        let prev = self.config;
        self.config = cfg;

        if self.needs_fft_rebuild() {
            self.rebuild_fft();
            self.reset = true;
        } else if prev.use_reassignment != cfg.use_reassignment {
            self.reset = true;
        }
    }
}

fn hilbert_transform(
    real: &[f32],
    analytic: &mut [Complex32],
    fft: &dyn Fft<f32>,
    ifft: &dyn Fft<f32>,
    scratch: &mut [Complex32],
) {
    let n = analytic.len();
    for (c, &r) in analytic.iter_mut().zip(real.iter()) {
        *c = Complex32::new(r, 0.0);
    }
    analytic[real.len()..].fill(Complex32::ZERO);

    fft.process_with_scratch(analytic, scratch);
    analytic[n / 2 + 1..].fill(Complex32::ZERO);
    ifft.process_with_scratch(analytic, scratch);

    let inv_n = 1.0 / n as f32;
    for c in analytic.iter_mut() {
        *c *= inv_n;
    }
}

fn fft_windowed(
    analytic: &[Complex32],
    window: &[f32],
    complex_buf: &mut [Complex32],
    output: &mut [Complex32],
    fft: &dyn Fft<f32>,
    scratch: &mut [Complex32],
) {
    for (c, (&a, &w)) in complex_buf
        .iter_mut()
        .zip(analytic.iter().zip(window.iter()))
    {
        *c = a * w;
    }
    complex_buf[window.len()..].fill(Complex32::ZERO);
    fft.process_with_scratch(complex_buf, scratch);
    output.copy_from_slice(&complex_buf[..output.len()]);
}

fn compute_derivative_spectral(window: &[f32]) -> Vec<f32> {
    let n = window.len();
    if n <= 1 {
        return vec![0.0; n];
    }
    let mut planner = FftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);

    let mut buf: Vec<Complex32> = window.iter().map(|&r| Complex32::new(r, 0.0)).collect();
    let mut scratch = vec![Complex32::ZERO; fwd.get_inplace_scratch_len()];
    fwd.process_with_scratch(&mut buf, &mut scratch);

    let scale = core::f32::consts::TAU / n as f32;
    let half = n / 2;
    buf[0] = Complex32::ZERO;
    if n.is_multiple_of(2) {
        buf[half] = Complex32::ZERO;
    }
    for (k, bin) in buf.iter_mut().enumerate().skip(1) {
        let omega = scale * (k as f32 - if k > half { n as f32 } else { 0.0 });
        *bin = Complex32::new(-omega * bin.im, omega * bin.re);
    }

    scratch.resize(inv.get_inplace_scratch_len(), Complex32::ZERO);
    inv.process_with_scratch(&mut buf, &mut scratch);

    let inv_n = 1.0 / n as f32;
    buf.iter().map(|c| c.re * inv_n).collect()
}

fn compute_time_weighted(window: &[f32]) -> Vec<f32> {
    let center = (window.len().saturating_sub(1)) as f32 * 0.5;
    window
        .iter()
        .enumerate()
        .map(|(i, &weight)| (i as f32 - center) * weight)
        .collect()
}

#[cfg(test)]
fn compute_sigma_t(window: &[f32]) -> f32 {
    let center = (window.len().saturating_sub(1)) as f32 * 0.5;
    let (weighted, total) =
        window
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(weighted, total), (i, &sample)| {
                let (offset, sq) = (i as f32 - center, (sample * sample) as f64);
                (weighted + (offset * offset) as f64 * sq, total + sq)
            });
    if total < 1e-10 {
        1.0
    } else {
        (weighted / total).sqrt().max(1.0) as f32
    }
}

#[inline]
fn load_f32_simd(data: &[f32], off: usize) -> f32x8 {
    let mut lanes = [0.0; 8];
    let count = (data.len().saturating_sub(off)).min(8);
    lanes[..count].copy_from_slice(&data[off..off + count]);
    f32x8::new(lanes)
}

#[inline]
fn load_complex_simd(data: &[Complex32], off: usize) -> (f32x8, f32x8) {
    let (mut re, mut im) = ([0.0; 8], [0.0; 8]);
    for (i, c) in data[off..].iter().take(8).enumerate() {
        re[i] = c.re;
        im[i] = c.im;
    }
    (f32x8::new(re), f32x8::new(im))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::AudioBlock;
    use std::time::Instant;

    fn make_block(samples: Vec<f32>, channels: usize, rate: f32) -> AudioBlock<'static> {
        AudioBlock::new(
            Box::leak(samples.into_boxed_slice()),
            channels,
            rate,
            Instant::now(),
        )
    }
    fn sine(freq: f32, rate: f32, count: usize) -> Vec<f32> {
        (0..count)
            .map(|i| (core::f32::consts::TAU * freq * i as f32 / rate).sin())
            .collect()
    }
    fn unwrap(result: Option<SpectrogramUpdate>) -> SpectrogramUpdate {
        result.expect("expected snapshot")
    }

    fn find_peak(points: &[SpectrogramPoint]) -> (usize, f32) {
        points
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.magnitude_db.total_cmp(&b.1.magnitude_db))
            .map(|(i, p)| (i, p.magnitude_db))
            .unwrap()
    }

    #[test]
    fn detects_sine_frequency_peak() {
        let cfg = SpectrogramConfig {
            fft_size: 1024,
            hop_size: 512,
            history_length: 8,
            window: WindowKind::Hann,
            zero_padding_factor: 1,
            use_reassignment: false,
            ..Default::default()
        };
        let mut processor = SpectrogramProcessor::new(cfg);
        let freq = 200.0 * cfg.sample_rate / 1024.0;
        let block = make_block(sine(freq, cfg.sample_rate, 2048), 1, cfg.sample_rate);
        let update = unwrap(processor.process_block(&block));
        let col = update.new_columns.last().unwrap();
        let (idx, db) = find_peak(&col.points);
        assert_eq!(idx, 200);
        assert!(db > -0.01 && db < 0.01, "peak dB = {db:.6}, expected ~0.0");
    }

    #[test]
    fn erb_conversions_are_invertible() {
        for &h in &[20.0f32, 100.0, 440.0, 1000.0, 4000.0, 10000.0] {
            assert!((h - erb_rate_to_hz(hz_to_erb_rate(h))).abs() < 0.002);
        }
    }

    #[test]
    fn reassignment_2d_with_group_delay() {
        let cfg = SpectrogramConfig {
            fft_size: 2048,
            hop_size: 512,
            history_length: 4,
            use_reassignment: true,
            zero_padding_factor: 1,
            ..Default::default()
        };
        let mut processor = SpectrogramProcessor::new(cfg);
        let freq = 50.3 * cfg.sample_rate / 2048.0;
        let block = make_block(sine(freq, cfg.sample_rate, 4096), 1, cfg.sample_rate);
        let update = unwrap(processor.process_block(&block));
        let col = update.new_columns.last().unwrap();
        let (_, peak_db) = find_peak(&col.points);
        let peak_pt = col
            .points
            .iter()
            .filter(|p| p.magnitude_db > DB_FLOOR)
            .max_by(|a, b| a.magnitude_db.total_cmp(&b.magnitude_db))
            .expect("expected non-sentinel point");
        assert!(
            (peak_pt.freq_hz - freq).abs() < 2.0,
            "reassigned freq {:.4} vs expected {freq:.4}",
            peak_pt.freq_hz
        );
        assert!(
            peak_db > DB_FLOOR,
            "peak dB = {peak_db:.6}, expected above floor"
        );
    }

    #[test]
    fn window_sigma_t_matches_theoretical_ratios() {
        let size = 4096_f32;
        let pairs: &[(WindowKind, f32)] = &[
            (WindowKind::Rectangular, 0.2887),
            (WindowKind::Hann, 0.1414),
            (WindowKind::Hamming, 0.1540),
            (WindowKind::Blackman, 0.1188),
            (WindowKind::BlackmanHarris, 0.1013),
        ];
        for &(kind, expected) in pairs {
            let window = kind.coefficients(size as usize);
            let ratio = compute_sigma_t(&window) / size;
            assert!(
                (ratio - expected).abs() < 0.001,
                "{kind:?}: sigma_t ratio = {ratio:.6}, expected ~{expected}"
            );
        }
    }

    #[test]
    fn points_per_column_matches_bin_count() {
        let cfg = SpectrogramConfig {
            fft_size: 512,
            hop_size: 256,
            history_length: 4,
            use_reassignment: false,
            zero_padding_factor: 1,
            ..Default::default()
        };
        let mut processor = SpectrogramProcessor::new(cfg);
        let block = make_block(sine(440.0, cfg.sample_rate, 1024), 1, cfg.sample_rate);
        let update = unwrap(processor.process_block(&block));
        let expected_bins = cfg.fft_size / 2 + 1;
        assert_eq!(update.points_per_column, expected_bins);
        for col in &update.new_columns {
            assert_eq!(col.points.len(), expected_bins);
        }
    }

    #[test]
    fn reassigned_sentinels_for_filtered_bins() {
        let cfg = SpectrogramConfig {
            fft_size: 1024,
            hop_size: 512,
            history_length: 4,
            use_reassignment: true,
            zero_padding_factor: 1,
            ..Default::default()
        };
        let mut processor = SpectrogramProcessor::new(cfg);
        let block = make_block(sine(1000.0, cfg.sample_rate, 2048), 1, cfg.sample_rate);
        let update = unwrap(processor.process_block(&block));
        let col = update.new_columns.last().unwrap();
        let sentinel_count = col
            .points
            .iter()
            .filter(|p| *p == &SpectrogramPoint::SENTINEL)
            .count();
        assert!(
            sentinel_count > 0,
            "expected some sentinel points for bins outside frequency range or below floor"
        );
        let non_sentinel_count = col.points.len() - sentinel_count;
        assert!(
            non_sentinel_count > 0,
            "expected some non-sentinel points for a 1kHz sine"
        );
    }
}
