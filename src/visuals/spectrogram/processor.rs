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

use crate::dsp::AudioBlock;
use crate::util::audio::{
    DB_FLOOR, DEFAULT_SAMPLE_RATE, FrequencyScale, LN_TO_DB, WindowKind,
    compute_fft_bin_normalization, copy_dc_removed_from_deque, db_to_power, power_to_db,
    sample_rates_differ, sanitize_sample_rate, window_coefficients,
};
use bytemuck::{Pod, Zeroable};
use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::collections::VecDeque;
use std::sync::Arc;

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

const DEFAULT_SPECTROGRAM_FFT_SIZE: usize = 2048;
const DEFAULT_SPECTROGRAM_HOP_SIZE: usize = 64;
pub(crate) const MAX_SPECTROGRAM_HISTORY_COLUMNS: usize = 8192;
pub(super) const SPECTROGRAM_HISTORY_BYTE_BUDGET: usize = 128 * 1024 * 1024;

// Fixed [dB] storage domain -- must match the shader constants in spectrogram.wgsl.
// u16 unorm over this range gives ~0.0024 dB/step, decoupled from the live
// floor/ceiling window so history recolors cleanly on slider drags.
pub(super) const CLASSIC_DB_STORE_LO: f32 = -144.0;
pub(super) const CLASSIC_DB_STORE_HI: f32 = 12.0;
pub(super) const CLASSIC_DB_STORE_RANGE: f32 = CLASSIC_DB_STORE_HI - CLASSIC_DB_STORE_LO;

impl Default for SpectrogramConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            fft_size: DEFAULT_SPECTROGRAM_FFT_SIZE,
            hop_size: DEFAULT_SPECTROGRAM_HOP_SIZE,
            window: WindowKind::Hann,
            frequency_scale: FrequencyScale::default(),
            history_length: 0,
            use_reassignment: true,
            zero_padding_factor: 1,
        }
    }
}

impl SpectrogramConfig {
    fn normalize(&mut self) {
        self.sample_rate = sanitize_sample_rate(self.sample_rate);
        if self.fft_size == 0 {
            self.fft_size = DEFAULT_SPECTROGRAM_FFT_SIZE;
        }
        if self.hop_size == 0 {
            self.hop_size = DEFAULT_SPECTROGRAM_HOP_SIZE.min(self.fft_size).max(1);
        }
        self.zero_padding_factor = self.zero_padding_factor.max(1);
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

fn resize_trim<T: Clone>(buf: &mut Vec<T>, len: usize, value: T) {
    buf.resize(len, value);
    if buf.capacity() > len.saturating_mul(4).max(1) {
        buf.shrink_to(len);
    }
}

pub(super) fn pack_classic_db(db: f32) -> u16 {
    const SCALE: f32 = 65535.0 / CLASSIC_DB_STORE_RANGE;
    ((db - CLASSIC_DB_STORE_LO) * SCALE)
        .round()
        .clamp(0.0, 65535.0) as u16
}

impl ReassignmentBuffers {
    fn rebuild(&mut self, planner: &mut FftPlanner<f32>, window: &[f32], bin_count: usize) {
        self.derivative_window = compute_derivative_spectral(planner, window);
        self.time_weighted_window = compute_time_weighted(window);
        self.derivative_spectrum = vec![Complex32::ZERO; bin_count];
        self.time_weighted_spectrum = vec![Complex32::ZERO; bin_count];
        self.floor_linear = db_to_power(DB_FLOOR);
    }
}

// Reassigned ships fractional (t, f, mag) per bin for splat rendering.
// Classic ships packed fixed-domain dB per bin; freq is implicit (k * bin_hz)
// and the renderer fills between adjacent bins.
#[derive(Debug, Clone)]
pub enum SpectrogramColumn {
    Reassigned(Vec<SpectrogramPoint>),
    Classic(Vec<u16>),
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
    fft: Arc<dyn Fft<f32>>,
    hilbert_fft: Arc<dyn Fft<f32>>,
    hilbert_ifft: Arc<dyn Fft<f32>>,
    window_size: usize,
    fft_size: usize,
    window: Arc<[f32]>,
    real: Vec<f32>,
    complex_buf: Vec<Complex32>,
    hilbert_buf: Vec<Complex32>,
    spectrum: Vec<Complex32>,
    scratch: Vec<Complex32>,
    classic_bins: Vec<u16>,
    reassign: ReassignmentBuffers,
    bin_norm: Vec<f32>,
    audio_buffer: VecDeque<f32>,
    audio_front_sample: u64,
    audio_last_nonzero: Option<u64>,
    bin_hz: f32,
    reset: bool,
}

impl SpectrogramProcessor {
    pub fn new(mut cfg: SpectrogramConfig) -> Self {
        cfg.normalize();
        let mut planner = FftPlanner::new();
        let placeholder_fft = planner.plan_fft_forward(1024);
        let mut processor = Self {
            config: cfg,
            fft: placeholder_fft.clone(),
            hilbert_fft: placeholder_fft.clone(),
            hilbert_ifft: placeholder_fft,
            window_size: 0,
            fft_size: 0,
            window: Arc::from([]),
            real: Vec::new(),
            complex_buf: Vec::new(),
            hilbert_buf: Vec::new(),
            spectrum: Vec::new(),
            scratch: Vec::new(),
            classic_bins: Vec::new(),
            reassign: ReassignmentBuffers::default(),
            bin_norm: Vec::new(),
            audio_buffer: VecDeque::new(),
            audio_front_sample: 0,
            audio_last_nonzero: None,
            bin_hz: 0.0,
            reset: true,
        };
        processor.rebuild_fft();
        processor
    }

    pub fn config(&self) -> SpectrogramConfig {
        self.config
    }

    fn hilbert_len_for(window_size: usize) -> usize {
        (window_size * 2).next_power_of_two().max(2)
    }

    fn rebuild_fft(&mut self) {
        self.window_size = self.config.fft_size;
        self.fft_size = self.window_size * self.config.zero_padding_factor.max(1);
        let hilbert_len = Self::hilbert_len_for(self.window_size);
        let use_reassignment = self.config.use_reassignment;
        let active_len = if use_reassignment { hilbert_len } else { self.window_size };
        let mut planner = FftPlanner::new();
        self.fft = planner.plan_fft_forward(self.fft_size);
        (self.hilbert_fft, self.hilbert_ifft) = if use_reassignment {
            (planner.plan_fft_forward(hilbert_len), planner.plan_fft_inverse(hilbert_len))
        } else {
            (self.fft.clone(), self.fft.clone())
        };
        self.window = window_coefficients(self.config.window, self.window_size);
        let bin_count = self.fft_size / 2 + 1;
        let reassigned_len = if use_reassignment { hilbert_len } else { 0 };
        let reassigned_bins = if use_reassignment { bin_count } else { 0 };
        resize_trim(&mut self.real, active_len, 0.0);
        resize_trim(&mut self.complex_buf, self.fft_size, Complex32::ZERO);
        resize_trim(&mut self.hilbert_buf, reassigned_len, Complex32::ZERO);
        resize_trim(&mut self.spectrum, reassigned_bins, Complex32::ZERO);
        let scratch_len = if use_reassignment {
            self.fft
                .get_inplace_scratch_len()
                .max(self.hilbert_fft.get_inplace_scratch_len())
                .max(self.hilbert_ifft.get_inplace_scratch_len())
        } else {
            self.fft.get_inplace_scratch_len()
        };
        resize_trim(&mut self.scratch, scratch_len, Complex32::ZERO);
        resize_trim(&mut self.classic_bins, bin_count, 0);
        if use_reassignment {
            self.reassign.rebuild(&mut planner, &self.window, bin_count);
        } else {
            self.reassign = ReassignmentBuffers::default();
        }
        self.bin_norm = compute_fft_bin_normalization(&self.window, self.fft_size);
        let buffered_len = active_len.saturating_mul(2);
        self.audio_buffer.truncate(buffered_len);
        self.recompute_audio_activity();
        self.shrink_audio_buffer(buffered_len);
        self.bin_hz = self.config.sample_rate / self.fft_size.max(1) as f32;
    }

    fn max_retained_columns(&self, bin_count: usize) -> usize {
        let reassigned = self.config.use_reassignment;
        let stride = if reassigned {
            bin_count.saturating_mul(std::mem::size_of::<SpectrogramPoint>())
        } else {
            bin_count.div_ceil(2).saturating_mul(4)
        };
        let max_cols = SPECTROGRAM_HISTORY_BYTE_BUDGET * (1 + usize::from(reassigned)) / stride.max(1);
        self.config.history_length.clamp(1, MAX_SPECTROGRAM_HISTORY_COLUMNS).min(max_cols)
    }

    fn process_ready_windows(&mut self) -> Vec<SpectrogramColumn> {
        if self.window_size == 0 { return Vec::new(); }
        let (hop_size, sample_rate) = (self.config.hop_size, self.config.sample_rate);
        let reassignment_enabled = self.config.use_reassignment && sample_rate > f32::EPSILON;
        let bin_count = self.fft_size / 2 + 1;

        let (read_len, center_offset) = if reassignment_enabled {
            let hilbert_len = Self::hilbert_len_for(self.window_size);
            (hilbert_len, (hilbert_len - self.window_size) / 2)
        } else {
            (self.window_size, 0)
        };

        let pending = self.audio_buffer.len();
        let ready = if pending >= read_len {
            (pending - read_len) / hop_size.max(1) + 1
        } else {
            0
        };
        let retained = self.max_retained_columns(bin_count);
        let skip = ready.saturating_sub(retained);
        let mut output = Vec::with_capacity(ready.min(retained));
        let skipped_samples = skip.saturating_mul(hop_size).min(self.audio_buffer.len());
        self.drain_audio(skipped_samples);

        for _ in skip..ready {
            if self.audio_is_silent() {
                let col = if reassignment_enabled {
                    SpectrogramColumn::Reassigned(vec![SpectrogramPoint::SENTINEL; bin_count])
                } else {
                    self.classic_bins[..bin_count].fill(pack_classic_db(DB_FLOOR));
                    SpectrogramColumn::Classic(self.classic_bins[..bin_count].to_vec())
                };
                output.push(col);
                self.drain_audio(hop_size);
                continue;
            }

            copy_dc_removed_from_deque(&mut self.real[..read_len], &self.audio_buffer);
            let center = &self.real[center_offset..center_offset + self.window_size];

            let col = if reassignment_enabled {
                // Use an analytic signal so low-frequency bins are not polluted
                // by the negative-frequency mirror of the windowed real signal.
                hilbert_transform(
                    &self.real[..read_len],
                    &mut self.hilbert_buf,
                    &*self.hilbert_fft,
                    &*self.hilbert_ifft,
                    &mut self.scratch,
                );
                let analytic = &self.hilbert_buf[center_offset..center_offset + self.window_size];
                let fft = &*self.fft;
                let r = &mut self.reassign;
                let stages: [(&[f32], &mut [Complex32]); 3] = [
                    (&self.window, &mut self.spectrum),
                    (&r.derivative_window, &mut r.derivative_spectrum),
                    (&r.time_weighted_window, &mut r.time_weighted_spectrum),
                ];
                for (window, out) in stages {
                    fft_windowed(
                        analytic,
                        window,
                        &mut self.complex_buf,
                        out,
                        fft,
                        &mut self.scratch,
                    );
                }
                SpectrogramColumn::Reassigned(self.reassigned_points(
                    sample_rate,
                    hop_size,
                    center_offset,
                    bin_count,
                ))
            } else {
                for (c, (&sample, &weight)) in self
                    .complex_buf
                    .iter_mut()
                    .zip(center.iter().zip(self.window.iter()))
                {
                    *c = Complex32::new(sample * weight, 0.0);
                }
                self.complex_buf[self.window_size..].fill(Complex32::ZERO);
                self.fft
                    .process_with_scratch(&mut self.complex_buf, &mut self.scratch);
                Self::compute_classic_bins(
                    &self.complex_buf[..bin_count],
                    &self.bin_norm,
                    &mut self.classic_bins,
                );
                SpectrogramColumn::Classic(self.classic_bins[..bin_count].to_vec())
            };

            output.push(col);
            self.drain_audio(hop_size);
        }
        self.shrink_audio_buffer(read_len.saturating_mul(4));
        output
    }

    fn shrink_audio_buffer(&mut self, target: usize) {
        let target = target.max(self.audio_buffer.len());
        if self.audio_buffer.capacity() > target.saturating_mul(4).max(1) {
            self.audio_buffer.shrink_to(target);
        }
    }

    fn recompute_audio_activity(&mut self) {
        let base = self.audio_front_sample;
        self.audio_last_nonzero = self
            .audio_buffer
            .iter()
            .rposition(|&sample| sample != 0.0)
            .map(|i| base + i as u64);
    }

    fn audio_is_silent(&self) -> bool {
        self.audio_last_nonzero
            .is_none_or(|last| last < self.audio_front_sample)
    }

    fn drain_audio(&mut self, count: usize) {
        let count = count.min(self.audio_buffer.len());
        if count == 0 {
            return;
        }
        drop(self.audio_buffer.drain(..count));
        self.audio_front_sample = self.audio_front_sample.saturating_add(count as u64);
    }

    fn push_audio(&mut self, samples: &[f32], channels: usize) {
        if channels == 0 || samples.is_empty() {
            return;
        }
        if channels == 1 {
            let base = self.audio_front_sample + self.audio_buffer.len() as u64;
            if let Some(i) = samples.iter().rposition(|&sample| sample != 0.0) {
                self.audio_last_nonzero = Some(base + i as u64);
            }
            self.audio_buffer.extend(samples);
            return;
        }

        self.audio_buffer.reserve(samples.len() / channels);
        let inv = 1.0 / channels as f32;
        for frame in samples.chunks_exact(channels) {
            let sample = frame.iter().sum::<f32>() * inv;
            if sample != 0.0 {
                self.audio_last_nonzero =
                    Some(self.audio_front_sample + self.audio_buffer.len() as u64);
            }
            self.audio_buffer.push_back(sample);
        }
    }

    fn compute_classic_bins(spectrum: &[Complex32], bin_norm: &[f32], bins: &mut [u16]) {
        for (i, c) in spectrum.iter().enumerate() {
            let power = (c.re * c.re + c.im * c.im) * bin_norm[i];
            bins[i] = pack_classic_db(power_to_db(power, DB_FLOOR));
        }
    }

    fn reassigned_points(
        &self,
        sample_rate: f32,
        hop_size: usize,
        latency_samples: usize,
        bin_count: usize,
    ) -> Vec<SpectrogramPoint> {
        let bin_hz = self.bin_hz;
        let max_hz = sample_rate * 0.5;
        let floor_linear = self.reassign.floor_linear;
        let inv_2pi = sample_rate / core::f32::consts::TAU;
        let inv_hop = 1.0 / hop_size.max(1) as f32;
        let mut points = vec![SpectrogramPoint::SENTINEL; bin_count];

        for (i, point) in points.iter_mut().enumerate() {
            let base = self.spectrum[i];
            let d = self.reassign.derivative_spectrum[i];
            let t = self.reassign.time_weighted_spectrum[i];
            let energy_scale = self.bin_norm[i];
            let pow = base.re * base.re + base.im * base.im;
            let scaled_power = pow * energy_scale;
            let inv_pow = 1.0 / pow.max(f32::MIN_POSITIVE);
            let d_omega = -(d.im * base.re - d.re * base.im) * inv_pow;
            let freq_hz = i as f32 * bin_hz + d_omega * inv_2pi;
            let time_offset = (t.re * base.re + t.im * base.im) * inv_pow * inv_hop
                - latency_samples as f32 * inv_hop;

            if scaled_power >= floor_linear
                && energy_scale > 0.0
                && freq_hz > 0.0
                && max_hz - freq_hz > 0.0
            {
                *point = SpectrogramPoint {
                    time_offset,
                    freq_hz,
                    magnitude_db: (scaled_power.ln() * LN_TO_DB).max(DB_FLOOR),
                };
            }
        }

        points
    }
    pub fn process_block(&mut self, block: &AudioBlock<'_>) -> Option<SpectrogramUpdate> {
        if block.is_empty() { return None; }
        let sample_rate = block.sample_rate;
        if sample_rates_differ(self.config.sample_rate, sample_rate) {
            self.config.sample_rate = sample_rate;
            self.rebuild_fft();
            self.audio_buffer.clear();
            self.audio_front_sample = 0;
            self.audio_last_nonzero = None;
            self.reset = true;
        }
        self.push_audio(block.samples, block.channels);
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

    pub fn update_config(&mut self, mut cfg: SpectrogramConfig) {
        cfg.normalize();
        let prev = self.config;
        self.config = cfg;

        let rate_changed = sample_rates_differ(prev.sample_rate, cfg.sample_rate);
        let rebuild = prev.fft_size != cfg.fft_size
            || prev.zero_padding_factor != cfg.zero_padding_factor
            || prev.window != cfg.window
            || prev.use_reassignment != cfg.use_reassignment
            || rate_changed;

        if rebuild {
            self.rebuild_fft();
            if rate_changed {
                self.audio_buffer.clear();
                self.audio_front_sample = 0;
                self.audio_last_nonzero = None;
            }
        }
        let reset = rebuild || prev.hop_size != cfg.hop_size;
        if reset {
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

fn compute_derivative_spectral(planner: &mut FftPlanner<f32>, window: &[f32]) -> Vec<f32> {
    let n = window.len();
    if n <= 1 {
        return vec![0.0; n];
    }
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);

    let mut buf: Vec<Complex32> = window.iter().map(|&r| Complex32::new(r, 0.0)).collect();
    let scratch_len = fwd
        .get_inplace_scratch_len()
        .max(inv.get_inplace_scratch_len());
    let mut scratch = vec![Complex32::ZERO; scratch_len];
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
mod tests {
    use super::*;
    use crate::dsp::AudioBlock;

    fn sine(freq: f32, rate: f32, count: usize) -> Vec<f32> {
        (0..count)
            .map(|i| (core::f32::consts::TAU * freq * i as f32 / rate).sin())
            .collect()
    }

    fn process_samples(cfg: SpectrogramConfig, samples: &[f32]) -> SpectrogramUpdate {
        let mut processor = SpectrogramProcessor::new(cfg);
        processor
            .process_block(&AudioBlock::now(samples, 1, cfg.sample_rate))
            .expect("expected snapshot")
    }

    fn process_sine(cfg: SpectrogramConfig, freq: f32, samples: usize) -> SpectrogramUpdate {
        process_samples(cfg, &sine(freq, cfg.sample_rate, samples))
    }

    fn cfg(fft_size: usize, hop_size: usize, use_reassignment: bool) -> SpectrogramConfig {
        SpectrogramConfig {
            fft_size,
            hop_size,
            history_length: 4,
            use_reassignment,
            zero_padding_factor: 1,
            ..Default::default()
        }
    }

    fn peak_bin(mags: &[u16]) -> usize {
        mags.iter().enumerate().max_by_key(|&(_, &db)| db).unwrap().0
    }

    fn peak_point(points: &[SpectrogramPoint]) -> &SpectrogramPoint {
        points
            .iter()
            .filter(|p| p.magnitude_db > DB_FLOOR)
            .max_by(|a, b| a.magnitude_db.total_cmp(&b.magnitude_db))
            .expect("expected non-sentinel point")
    }

    fn classic_mags(col: &SpectrogramColumn) -> &[u16] {
        match col {
            SpectrogramColumn::Classic(v) => v,
            SpectrogramColumn::Reassigned(_) => panic!("expected classic column"),
        }
    }

    fn reassigned_points(col: &SpectrogramColumn) -> &[SpectrogramPoint] {
        match col {
            SpectrogramColumn::Reassigned(v) => v,
            SpectrogramColumn::Classic(_) => panic!("expected reassigned column"),
        }
    }

    #[test]
    fn classic_db_packing_rounds_to_nearest_code() {
        let step = CLASSIC_DB_STORE_RANGE / 65535.0;
        assert_eq!(pack_classic_db(CLASSIC_DB_STORE_LO + step * 1234.49), 1234);
        assert_eq!(pack_classic_db(CLASSIC_DB_STORE_LO + step * 1234.50), 1235);
    }

    #[test]
    fn invalid_config_values_are_normalized() {
        let processor = SpectrogramProcessor::new(SpectrogramConfig {
            sample_rate: f32::NAN,
            fft_size: 0,
            hop_size: 0,
            zero_padding_factor: 0,
            ..Default::default()
        });

        assert_eq!(processor.config.sample_rate, DEFAULT_SAMPLE_RATE);
        assert_eq!(processor.config.fft_size, DEFAULT_SPECTROGRAM_FFT_SIZE);
        assert_eq!(processor.config.hop_size, DEFAULT_SPECTROGRAM_HOP_SIZE);
        assert_eq!(processor.config.zero_padding_factor, 1);
    }

    #[test]
    fn detects_sine_frequency_peak() {
        let cfg = SpectrogramConfig {
            history_length: 8,
            window: WindowKind::Hann,
            ..cfg(1024, 512, false)
        };
        let freq = 200.0 * cfg.sample_rate / cfg.fft_size as f32;
        let update = process_sine(cfg, freq, 2048);
        let mags = classic_mags(update.new_columns.last().unwrap());
        let idx = peak_bin(mags);

        assert_eq!(update.points_per_column, cfg.fft_size / 2 + 1);
        assert_eq!(mags.len(), update.points_per_column);
        assert_eq!(idx, 200);
        assert!(mags[idx] >= pack_classic_db(-0.01));
    }

    #[test]
    fn retained_history_matches_full_suffix() {
        let mut full_cfg = cfg(64, 16, false);
        full_cfg.history_length = 32;
        let mut capped_cfg = full_cfg;
        capped_cfg.history_length = 3;
        let samples: Vec<_> = (0..192).map(|i| ((i * i + 3 * i) as f32 * 0.017).sin()).collect();

        let full = process_samples(full_cfg, &samples);
        let capped = process_samples(capped_cfg, &samples);
        let expected = &full.new_columns[full.new_columns.len() - capped.new_columns.len()..];

        assert_eq!(capped.new_columns.len(), capped_cfg.history_length);
        assert_ne!(classic_mags(&full.new_columns[0]), classic_mags(&expected[0]));
        for (expected, actual) in expected.iter().zip(&capped.new_columns) {
            assert_eq!(classic_mags(expected), classic_mags(actual));
        }
    }

    #[test]
    fn classic_retention_budget_uses_packed_column_width() {
        let processor = SpectrogramProcessor::new(SpectrogramConfig {
            fft_size: 16_384,
            zero_padding_factor: 32,
            history_length: MAX_SPECTROGRAM_HISTORY_COLUMNS,
            use_reassignment: false,
            ..Default::default()
        });
        let bins = processor.fft_size / 2 + 1;
        let packed_stride = bins.div_ceil(2) * std::mem::size_of::<u32>();

        assert_eq!(
            processor.max_retained_columns(bins),
            SPECTROGRAM_HISTORY_BYTE_BUDGET / packed_stride
        );
    }

    #[test]
    fn sample_rate_config_rebuilds_bin_spacing() {
        let cfg = SpectrogramConfig {
            fft_size: 1024,
            ..Default::default()
        };
        let mut processor = SpectrogramProcessor::new(cfg);
        let mut next = cfg;
        next.sample_rate *= 2.0;

        processor.update_config(next);

        assert_eq!(processor.bin_hz, next.sample_rate / processor.fft_size as f32);
    }

    #[test]
    fn silent_input_advances_transparent_columns() {
        let samples = vec![0.0; 192];
        let floor = pack_classic_db(DB_FLOOR);

        let classic = process_samples(cfg(64, 16, false), &samples);
        assert_eq!(classic.new_columns.len(), 4);
        assert!(classic
            .new_columns
            .iter()
            .all(|col| classic_mags(col).iter().all(|&mag| mag == floor)));

        let reassigned = process_samples(cfg(64, 16, true), &samples);
        assert_eq!(reassigned.new_columns.len(), 4);
        assert!(reassigned
            .new_columns
            .iter()
            .all(|col| {
                reassigned_points(col)
                    .iter()
                    .all(|&p| p == SpectrogramPoint::SENTINEL)
            }));
    }

    #[test]
    fn reassignment_places_peak_frequency_and_time() {
        let cfg = cfg(2048, 512, true);
        let latency = (SpectrogramProcessor::hilbert_len_for(cfg.fft_size) - cfg.fft_size) / 2;
        let expected_time = -(latency as f32) / cfg.hop_size as f32;

        for bin in [3.4, 10.25, 50.25, 200.75, 800.4] {
            let freq = bin * cfg.sample_rate / cfg.fft_size as f32;
            let update = process_sine(cfg, freq, 4096);
            let points = reassigned_points(update.new_columns.last().unwrap());
            let peak = peak_point(points);

            assert!(
                (peak.freq_hz - freq).abs() < 2.0,
                "reassigned freq {:.4} vs expected {freq:.4}",
                peak.freq_hz
            );
            assert!(
                (peak.time_offset - expected_time).abs() < 0.05,
                "time offset {:.4} vs expected {expected_time:.4}",
                peak.time_offset
            );
            assert!(points.contains(&SpectrogramPoint::SENTINEL));
        }
    }
}
