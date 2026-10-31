// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::dsp::AudioBlock;
use crate::util::audio::{
    Channel, DB_FLOOR, DEFAULT_SAMPLE_RATE, DcHistory, MAX_DSP_BUFFER_LEN, WindowKind,
    compute_fft_bin_normalization, copy_dc_removed_windowed, kaiser, normalized_blackman_harris,
    power_to_db, sanitize_sample_rate, weighted_mean,
};
use bytemuck::{Pod, Zeroable};
use realfft::{RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::collections::VecDeque;
use std::sync::Arc;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct SpectrogramPoint {
    pub time_offset: f32,
    pub freq_hz: f32,
    pub power: f32,
}

crate::macros::default_struct! {
    #[derive(Debug, Clone, Copy)]
    pub struct SpectrogramConfig {
        pub sample_rate: f32 = DEFAULT_SAMPLE_RATE,
        pub fft_size: usize = DEFAULT_SPECTROGRAM_FFT_SIZE,
        pub hop_size: usize = DEFAULT_SPECTROGRAM_HOP_SIZE,
        pub window: WindowKind = WindowKind::Hann,
        pub source: Channel = Channel::Mid,
        pub history_length: usize = 0,
        pub use_reassignment: bool = true,
        pub zero_padding_factor: usize = 1,
    }
}

const DEFAULT_SPECTROGRAM_FFT_SIZE: usize = 4096;
const DEFAULT_SPECTROGRAM_HOP_SIZE: usize = 256;
// Full-column payload budget: classic 1x, reassigned 2x; minimum one column.
pub(super) const SPECTROGRAM_HISTORY_BYTE_BUDGET: usize = 128 * 1024 * 1024;

// Fixed shader-mirrored dB domain; u16 gives ~0.0024 dB/step and permits live history recoloring.
pub(super) const CLASSIC_DB_STORE_LO: f32 = -144.0;
pub(super) const CLASSIC_DB_STORE_HI: f32 = 12.0;
pub(super) const CLASSIC_DB_STORE_RANGE: f32 = CLASSIC_DB_STORE_HI - CLASSIC_DB_STORE_LO;
// Linear-power equivalent of util::audio::DB_FLOOR.
const ANALYSIS_FLOOR_POWER: f32 = 1e-14;

impl SpectrogramConfig {
    fn normalize(&mut self) {
        self.sample_rate = sanitize_sample_rate(self.sample_rate);
        self.fft_size = self.fft_size.min(MAX_DSP_BUFFER_LEN);
        // A one-sample window shows nothing, and its three operator taps would wrap twice.
        if self.fft_size < 2 {
            self.fft_size = DEFAULT_SPECTROGRAM_FFT_SIZE;
        }
        if self.hop_size == 0 {
            self.hop_size = DEFAULT_SPECTROGRAM_HOP_SIZE.min(self.fft_size);
        }
        self.zero_padding_factor = self
            .zero_padding_factor
            .clamp(1, MAX_DSP_BUFFER_LEN / self.fft_size);
    }
}

enum Transforms {
    Reassigned([Arc<dyn Fft<f32>>; 3]),
    Classic(Arc<dyn RealToComplex<f32>>),
}

#[derive(Default)]
struct ReassignmentBuffers {
    hilbert_gains: Vec<f32>,
    // Blackman-Harris weights summing to one; tones leak into this mean far less than a plain one.
    dc_weights: Vec<f32>,
    // Bins past DC and Nyquist that the main lobes of tones near them reach; no bin is read twice.
    edge_bins: usize,
    // N + 2 taps from one sample before the window: the window smoothed by [1/4, 1/2, 1/4]
    // and its central difference, both centered on it. A tone at any offset w from a bin
    // gives a derivative transform of exactly 2j tan(w / 2) times the base transform.
    base_window: Vec<f32>,
    derivative_window: Vec<f32>,
    time_weighted_window: Vec<f32>,
    // Reassigned times the window can hold, in samples from its weighted center.
    time_support: [f32; 2],
    spectra: Vec<Complex32>,
}

pub(super) fn pack_classic_db(db: f32) -> u16 {
    const SCALE: f32 = 65535.0 / CLASSIC_DB_STORE_RANGE;
    ((db - CLASSIC_DB_STORE_LO) * SCALE + 0.5) as u16
}

// Correct coherent-gain power for ENBW and zero-padding after splat accumulation.
fn reassigned_power_scale(window: &[f32], fft_size: usize) -> f32 {
    let (sum, sum_squares) = window.iter().fold((0.0, 0.0), |(sum, squares), &x| {
        let x = f64::from(x);
        (sum + x, squares + x * x)
    });
    (sum * sum / (fft_size as f64 * sum_squares)) as f32
}

// Reassigned: (hop offset, Hz, linear power); omit sub-floor bins.
// Classic: packed dB; shader interpolates frequency, not time.
#[derive(Debug, PartialEq)]
pub enum SpectrogramColumn {
    Reassigned(Vec<SpectrogramPoint>),
    Classic(Vec<u16>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ColumnKind {
    Reassigned,
    Classic,
}

impl SpectrogramColumn {
    pub(super) fn kind(&self) -> ColumnKind {
        match self {
            Self::Reassigned(_) => ColumnKind::Reassigned,
            Self::Classic(_) => ColumnKind::Classic,
        }
    }

    pub(super) fn is_quiescent(&self) -> bool {
        match self {
            Self::Reassigned(points) => points.is_empty(),
            Self::Classic(levels) => levels
                .iter()
                .all(|level| *level == pack_classic_db(DB_FLOOR)),
        }
    }
}

pub(super) fn col_byte_stride(kind: ColumnKind, points: u32) -> u64 {
    match kind {
        ColumnKind::Reassigned => {
            u64::from(points) * std::mem::size_of::<SpectrogramPoint>() as u64
        }
        ColumnKind::Classic => u64::from(points).div_ceil(2) * 4,
    }
}

// Columns past the view that keep evicted splats off it (render.rs image_start).
const REASSIGNED_HISTORY_MARGIN: usize = 2;

pub(super) fn history_columns(kind: ColumnKind, points: u32, requested: usize) -> usize {
    let affordable = SPECTROGRAM_HISTORY_BYTE_BUDGET
        * (1 + usize::from(kind == ColumnKind::Reassigned))
        / col_byte_stride(kind, points).max(1) as usize;
    let margin = REASSIGNED_HISTORY_MARGIN * usize::from(kind == ColumnKind::Reassigned);
    (requested.max(1) + margin).min(affordable.max(1))
}

pub struct SpectrogramUpdate {
    pub fft_size: usize,
    pub points_per_column: usize,
    pub hop_size: usize,
    pub sample_rate: f32,
    pub history_length: usize,
    pub reset: bool,
    pub reassigned_power_scale: f32,
    // Samples from the newest audio back to the moment drawn at the live edge.
    pub edge_delay: usize,
    pub new_columns: Vec<SpectrogramColumn>,
}

crate::macros::default_struct! {
    pub struct SpectrogramProcessor {
        config: SpectrogramConfig = SpectrogramConfig::default(),
        transforms: Option<Transforms> = None,
        fft_size: usize = 0,
        window: Vec<f32> = Vec::new(),
        real: Vec<f32> = Vec::new(),
        complex: Vec<Complex32> = Vec::new(),
        scratch: Vec<Complex32> = Vec::new(),
        reassign: ReassignmentBuffers = ReassignmentBuffers::default(),
        bin_norm: Vec<f32> = Vec::new(),
        reassigned_power_scale: f32 = 1.0,
        edge_delay: usize = 0,
        audio_buffer: VecDeque<f32> = VecDeque::new(),
        classic_dc: DcHistory = DcHistory::default(),
        pending_skip_samples: usize = 0,
        audio_last_nonzero: Option<usize> = None,
        reset: bool = true,
    }
}

impl SpectrogramProcessor {
    pub fn new(mut cfg: SpectrogramConfig) -> Self {
        cfg.normalize();
        Self {
            config: cfg,
            ..Self::default()
        }
    }

    pub fn config(&self) -> SpectrogramConfig {
        self.config
    }

    pub fn reset_audio(&mut self) {
        self.audio_buffer.clear();
        self.classic_dc.clear();
        self.pending_skip_samples = 0;
        self.audio_last_nonzero = None;
        self.reset = true;
    }

    pub(in crate::visuals) fn prepare(&mut self) {
        if self.transforms.is_none() {
            self.rebuild_fft();
        }
    }

    pub(in crate::visuals) fn has_buffered_signal(&self) -> bool {
        self.audio_last_nonzero.is_some()
    }

    // (block length, kernel reach). The Hilbert kernel reaches half a window past each side of
    // it, and that reach is the lookahead. Small windows reach further, up to two windows, toward
    // the time the default size and rate reach, so low tones keep their accuracy without the
    // lookahead outgrowing the window. Older samples pad the block to twice the kernel span, so
    // the DC estimate keeps every tone the kernel passes.
    fn hilbert_layout(window_size: usize, sample_rate: f32) -> (usize, usize) {
        let default_reach =
            sample_rate / DEFAULT_SAMPLE_RATE * (DEFAULT_SPECTROGRAM_FFT_SIZE / 2) as f32;
        let reach = window_size
            .div_ceil(2)
            .max((default_reach as usize).min(2 * window_size));
        ((4 * reach).next_power_of_two(), reach)
    }

    fn rebuild_fft(&mut self) {
        let window_size = self.config.fft_size;
        self.fft_size = window_size * self.config.zero_padding_factor;
        self.window = self.config.window.coefficients(window_size);
        self.bin_norm = compute_fft_bin_normalization(&self.window, self.fft_size);
        let (transforms, active_len) = if self.config.use_reassignment {
            let (hilbert_len, reach) = Self::hilbert_layout(window_size, self.config.sample_rate);
            self.classic_dc = DcHistory::default();
            let mut planner = FftPlanner::new();
            let ffts = [
                planner.plan_fft_forward(self.fft_size),
                planner.plan_fft_forward(hilbert_len),
                planner.plan_fft_inverse(hilbert_len),
            ];
            let r = &mut self.reassign;
            r.edge_bins = (self.config.window.cosine_terms().len()
                * self.config.zero_padding_factor)
                .min(self.fft_size.saturating_sub(2) / 4);
            r.hilbert_gains = hilbert_gains(&*ffts[1], reach);
            r.dc_weights = normalized_blackman_harris(hilbert_len);
            let h = |tap: usize| self.window.get(tap.wrapping_sub(2)).copied().unwrap_or(0.0);
            (r.base_window, r.derivative_window) = (0..window_size + 2)
                .map(|n| {
                    let [a, b, c] = [n, n + 1, n + 2].map(h);
                    (0.25 * (a + c) + 0.5 * b, 0.5 * (a - c))
                })
                .unzip();
            (r.time_weighted_window, r.time_support) = compute_time_weighted(&r.base_window);
            r.spectra = vec![Complex32::ZERO; self.fft_size * 3];
            self.reassigned_power_scale = reassigned_power_scale(&r.base_window, self.fft_size);
            self.edge_delay = reach + window_size / 2 - Self::reassigned_reference(window_size);
            let scratch_len = ffts.iter().map(|fft| fft.get_inplace_scratch_len()).max();
            self.real = Vec::new();
            self.complex = vec![Complex32::ZERO; hilbert_len];
            self.scratch = vec![Complex32::ZERO; scratch_len.unwrap_or(0)];
            (Transforms::Reassigned(ffts), hilbert_len)
        } else {
            self.reassign = ReassignmentBuffers::default();
            self.classic_dc = DcHistory::new(window_size);
            self.reassigned_power_scale = 1.0;
            self.edge_delay = window_size / 2;
            let fft = RealFftPlanner::new().plan_fft_forward(self.fft_size);
            self.real = vec![0.0; self.fft_size];
            self.complex = vec![Complex32::ZERO; self.fft_size / 2 + 1];
            self.scratch = vec![Complex32::ZERO; fft.get_scratch_len()];
            (Transforms::Classic(fft), self.fft_size)
        };
        self.transforms = Some(transforms);
        let buffered_len = active_len.saturating_mul(2);
        self.drain_audio(self.audio_buffer.len().saturating_sub(buffered_len));
        self.pending_skip_samples = 0;
        self.audio_buffer.shrink_to(buffered_len);
    }

    fn process_ready_windows(&mut self) -> Vec<SpectrogramColumn> {
        let window_size = self.config.fft_size;
        let hop_size = self.config.hop_size;
        let points = self.points_per_column();
        let (kind, (read_len, lookahead)) =
            match self.transforms.as_ref().expect("spectrogram FFT prepared") {
                Transforms::Reassigned(..) => (
                    ColumnKind::Reassigned,
                    Self::hilbert_layout(window_size, self.config.sample_rate),
                ),
                Transforms::Classic(_) => (ColumnKind::Classic, (window_size, 0)),
            };

        let pending = self.audio_buffer.len();
        let ready = pending
            .checked_sub(read_len)
            .map_or(0, |extra| extra / hop_size + 1);
        let retained = history_columns(kind, points as u32, self.config.history_length);
        let skip = ready.saturating_sub(retained);
        let mut output = Vec::with_capacity(ready.min(retained));
        self.advance_audio(skip.saturating_mul(hop_size));
        // Columns share one analytic block while their windows and the kernel's reach fit in it.
        let per_block = (read_len - window_size - 2 * lookahead) / hop_size + 1;
        let mut block_offset = None;

        for column in skip..ready {
            let silent = self.audio_last_nonzero.is_none();
            let col = match self.transforms.as_ref().expect("spectrogram FFT prepared") {
                Transforms::Reassigned(..) if silent => SpectrogramColumn::Reassigned(Vec::new()),
                Transforms::Classic(_) if silent => {
                    SpectrogramColumn::Classic(vec![pack_classic_db(DB_FLOOR); points])
                }
                Transforms::Reassigned([analysis, hilbert_forward, hilbert_inverse]) => {
                    let audio = &*self.audio_buffer.make_contiguous();
                    let offset = *block_offset.get_or_insert_with(|| {
                        let offset = (per_block.min(ready - column) - 1) * hop_size;
                        let block = &audio[offset..offset + read_len];
                        for (dst, &sample) in self.complex.iter_mut().zip(block) {
                            *dst = Complex32::new(sample, 0.0);
                        }
                        // Avoid low-bin pollution from the real signal's negative-frequency mirror.
                        hilbert_forward.process_with_scratch(&mut self.complex, &mut self.scratch);
                        for (bin, gain) in self.complex.iter_mut().zip(&self.reassign.hilbert_gains)
                        {
                            *bin *= gain;
                        }
                        hilbert_inverse.process_with_scratch(&mut self.complex, &mut self.scratch);
                        offset
                    });
                    block_offset = offset.checked_sub(hop_size);
                    let r = &mut self.reassign;
                    let dc = weighted_mean(&r.dc_weights, &audio[..read_len]);
                    let start = read_len - lookahead - window_size - offset;
                    let analytic = &self.complex[start - 1..=start + window_size];
                    let (base, auxiliary) = r.spectra.split_at_mut(self.fft_size);
                    let (derivative, time_weighted) = auxiliary.split_at_mut(self.fft_size);
                    apply_complex_window(analytic, dc, &r.base_window, base);
                    apply_complex_window(analytic, dc, &r.derivative_window, derivative);
                    apply_complex_window(analytic, dc, &r.time_weighted_window, time_weighted);
                    analysis.process_with_scratch(&mut r.spectra, &mut self.scratch);
                    SpectrogramColumn::Reassigned(self.reassigned_points())
                }
                Transforms::Classic(fft) => {
                    let samples = &self.audio_buffer.make_contiguous()[..window_size];
                    let dc = self.classic_dc.dc(samples);
                    copy_dc_removed_windowed(
                        &mut self.real[..window_size],
                        samples,
                        &self.window,
                        dc,
                    );
                    self.real[window_size..].fill(0.0);
                    fft.process_with_scratch(&mut self.real, &mut self.complex, &mut self.scratch)
                        .expect("internally sized spectrogram FFT buffers");
                    SpectrogramColumn::Classic(
                        self.complex
                            .iter()
                            .zip(&self.bin_norm)
                            .map(|(complex, &normalization)| {
                                pack_classic_db(power_to_db(
                                    (complex.re * complex.re + complex.im * complex.im)
                                        * normalization,
                                    DB_FLOOR,
                                ))
                            })
                            .collect(),
                    )
                }
            };

            output.push(col);
            self.advance_audio(hop_size);
        }
        output
    }

    fn drain_audio(&mut self, count: usize) {
        let count = count.min(self.audio_buffer.len());
        self.classic_dc.extend(self.audio_buffer.drain(..count));
        self.audio_last_nonzero = self
            .audio_last_nonzero
            .and_then(|index| index.checked_sub(count));
    }

    fn advance_audio(&mut self, count: usize) {
        let missing = count.saturating_sub(self.audio_buffer.len());
        self.drain_audio(count);
        self.pending_skip_samples = self.pending_skip_samples.saturating_add(missing);
    }

    fn push_audio(&mut self, block: &AudioBlock<'_>) {
        let skip = self.pending_skip_samples.min(block.frame_count());
        self.pending_skip_samples -= skip;
        let mut samples = block.projected_frames(self.config.source);
        self.classic_dc.extend(samples.by_ref().take(skip));
        for sample in samples {
            if sample != 0.0 {
                self.audio_last_nonzero = Some(self.audio_buffer.len());
            }
            self.audio_buffer.push_back(sample);
        }
    }

    // Splats end at the live edge for every window, whose smoothed taps reach one sample past
    // half of it; a lookahead past half a window then delays the image instead of blanking it.
    const fn reassigned_reference(window_size: usize) -> usize {
        window_size / 2 + 1
    }

    fn points_per_column(&self) -> usize {
        self.fft_size / 2 + 1 + 2 * self.reassign.edge_bins
    }

    fn reassigned_points(&self) -> Vec<SpectrogramPoint> {
        let sample_rate = self.config.sample_rate;
        let bin_hz = sample_rate / self.fft_size as f32;
        let max_hz = sample_rate * 0.5;
        let inv_2pi = sample_rate / core::f32::consts::TAU;
        let inv_hop = 1.0 / self.config.hop_size as f32;
        let latency_hops = Self::reassigned_reference(self.config.fft_size) as f32 * inv_hop;
        let mut points = Vec::new();
        let (spectrum, auxiliary) = self.reassign.spectra.split_at(self.fft_size);
        let (derivative_spectrum, time_weighted_spectrum) = auxiliary.split_at(self.fft_size);
        let [earliest, latest] = self.reassign.time_support;

        let edge = self.reassign.edge_bins as isize;
        let nyquist = (self.fft_size / 2) as isize;
        // An edge bin only holds the main lobes of tones near the edge it guards.
        let edge_reach = 2.0 * edge as f32 * bin_hz;
        for k in -edge..=nyquist + edge {
            let i = if k < 0 { k + self.fft_size as isize } else { k } as usize;
            let base = spectrum[i];
            // keep power calculations in f32 for performance. I'm
            // willing to accept possible overflow with finite input.
            let pow = base.re * base.re + base.im * base.im;
            // Tones appear once, at twice their amplitude, so the DC scale fits every bin.
            let scaled_power = pow * self.bin_norm[0];
            if scaled_power < ANALYSIS_FLOOR_POWER {
                continue;
            }

            let d = derivative_spectrum[i];
            let t = time_weighted_spectrum[i];
            let inv_pow = 1.0 / pow;
            // No sample lies outside the window; such times come from near-zeros of the STFT.
            let time = (t.re * base.re + t.im * base.im) * inv_pow;
            if !(earliest..=latest).contains(&time) {
                continue;
            }
            let x = 0.5 * (d.im * base.re - d.re * base.im) * inv_pow;
            // atan to f32 precision; most points lie near their bin, where the series suffices.
            let d_omega = 2.0
                * if x.abs() < 0.02 {
                    x - x * x * x * (1.0 / 3.0)
                } else {
                    x.atan()
                };
            let freq_hz = k as f32 * bin_hz + d_omega * inv_2pi;
            if !(freq_hz > 0.0 && max_hz - freq_hz > 0.0)
                || (k < 0 && freq_hz > edge_reach)
                || (k > nyquist && max_hz - freq_hz > edge_reach)
            {
                continue;
            }

            if points.is_empty() {
                points.reserve(self.points_per_column());
            }
            points.push(SpectrogramPoint {
                time_offset: time * inv_hop - latency_hops,
                freq_hz,
                power: scaled_power,
            });
        }

        points
    }

    pub fn process_block(&mut self, block: &AudioBlock<'_>) -> Option<SpectrogramUpdate> {
        let sample_rate = block.sample_rate;
        if self.config.sample_rate != sample_rate {
            self.config.sample_rate = sample_rate;
            self.rebuild_fft();
            self.reset_audio();
        }
        self.prepare();
        self.push_audio(block);
        let cols = self.process_ready_windows();
        (!cols.is_empty()).then(|| SpectrogramUpdate {
            fft_size: self.fft_size,
            points_per_column: self.points_per_column(),
            hop_size: self.config.hop_size,
            sample_rate: self.config.sample_rate,
            history_length: self.config.history_length,
            reset: std::mem::take(&mut self.reset),
            reassigned_power_scale: self.reassigned_power_scale,
            edge_delay: self.edge_delay,
            new_columns: cols,
        })
    }

    pub fn update_config(&mut self, mut cfg: SpectrogramConfig) {
        cfg.normalize();
        let prev = self.config;
        let prepared = self.transforms.is_some();
        self.config = cfg;

        let rate_changed = prev.sample_rate != cfg.sample_rate;
        let rebuild = prev.fft_size != cfg.fft_size
            || prev.zero_padding_factor != cfg.zero_padding_factor
            || prev.window != cfg.window
            || prev.use_reassignment != cfg.use_reassignment
            || rate_changed;

        if rebuild && prepared {
            self.rebuild_fft();
        }
        if rate_changed || prev.source != cfg.source {
            self.reset_audio();
        }
        let hop_changed = prev.hop_size != cfg.hop_size;
        if hop_changed {
            self.pending_skip_samples = 0;
        }
        self.reset |= rebuild || hop_changed;
    }
}

// Spectral gains of a Kaiser-tapered Hilbert kernel short enough that the window sees a
// linear, not circular, transform: ~2 for positive bins, ~0 for negative, with the inverse
// transform's scale folded in.
// Beta 8 keeps the images of tones over two bins from DC or Nyquist about 85 dB down, and
// impulse times within a sample.
fn hilbert_gains(fft: &dyn Fft<f32>, half: usize) -> Vec<f32> {
    let len = fft.len();
    let mut kernel = vec![Complex32::ZERO; len];
    for tap in (1..half).step_by(2) {
        let taper = kaiser(8.0, tap as f64 / half as f64);
        let value = (taper / (core::f64::consts::PI * tap as f64)) as f32;
        kernel[tap].re = value;
        kernel[len - tap].re = -value;
    }
    fft.process(&mut kernel);
    kernel
        .iter()
        .map(|bin| (1.0 - 2.0 * bin.im) / len as f32)
        .collect()
}

// Taps past the output wrap around, as the FFT is circular.
fn apply_complex_window(analytic: &[Complex32], dc: f32, window: &[f32], output: &mut [Complex32]) {
    let (head, tail) = window.split_at(window.len().min(output.len()));
    for (out, (&sample, &weight)) in output.iter_mut().zip(analytic.iter().zip(head)) {
        *out = (sample - dc) * weight;
    }
    output[head.len()..].fill(Complex32::ZERO);
    for ((out, &sample), &weight) in output.iter_mut().zip(&analytic[head.len()..]).zip(tail) {
        *out += (sample - dc) * weight;
    }
}

// Center on the weights' mean index, which is the window's; also returns the times they hold.
fn compute_time_weighted(weights: &[f32]) -> (Vec<f32>, [f32; 2]) {
    let (weighted, sum) = weights
        .iter()
        .enumerate()
        .fold((0.0, 0.0), |(m, s), (i, &w)| {
            (m + i as f64 * f64::from(w), s + f64::from(w))
        });
    let center = (weighted / sum) as f32;
    let weighted = weights
        .iter()
        .enumerate()
        .map(|(i, &weight)| (i as f32 - center) * weight)
        .collect();
    let first = weights.iter().position(|&w| w != 0.0).unwrap_or(0) as f32;
    let last = weights.iter().rposition(|&w| w != 0.0).unwrap_or(0) as f32;
    (weighted, [first - center - 0.5, last - center + 0.5])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::audio::sine_wave;

    fn process_samples(cfg: SpectrogramConfig, samples: &[f32]) -> SpectrogramUpdate {
        let mut processor = SpectrogramProcessor::new(cfg);
        processor
            .process_block(&AudioBlock::new(samples, 1, cfg.sample_rate))
            .expect("expected snapshot")
    }

    fn process_sine(cfg: SpectrogramConfig, freq: f32, samples: usize) -> SpectrogramUpdate {
        process_samples(cfg, &sine_wave(freq, cfg.sample_rate, samples, 1.0))
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

    // Samples a reassigned column reads at the default rate, which is more than its window.
    fn block_len(fft_size: usize) -> usize {
        SpectrogramProcessor::hilbert_layout(fft_size, DEFAULT_SAMPLE_RATE).0
    }

    fn peak_point(points: &[SpectrogramPoint]) -> &SpectrogramPoint {
        points
            .iter()
            .filter(|p| p.power > ANALYSIS_FLOOR_POWER)
            .max_by(|a, b| a.power.total_cmp(&b.power))
            .expect("expected non-sentinel point")
    }

    fn classic_mags(col: &SpectrogramColumn) -> &[u16] {
        match col {
            SpectrogramColumn::Classic(v) => v,
            SpectrogramColumn::Reassigned(_) => panic!("expected classic column"),
        }
    }

    fn reassigned_points(col: &SpectrogramColumn) -> &Vec<SpectrogramPoint> {
        match col {
            SpectrogramColumn::Reassigned(v) => v,
            SpectrogramColumn::Classic(_) => panic!("expected reassigned column"),
        }
    }

    #[test]
    fn classic_db_packing_rounds_to_nearest_code() {
        let step = CLASSIC_DB_STORE_RANGE / 65535.0;
        for (db, expected) in [
            (CLASSIC_DB_STORE_LO + step * 1234.49, 1234),
            (CLASSIC_DB_STORE_LO + step * 1234.50, 1235),
            (CLASSIC_DB_STORE_LO, 0),
            (CLASSIC_DB_STORE_LO - 1.0, 0),
            (CLASSIC_DB_STORE_HI, u16::MAX),
            (CLASSIC_DB_STORE_HI + 1.0, u16::MAX),
            (f32::NEG_INFINITY, 0),
            (f32::INFINITY, u16::MAX),
            (f32::NAN, 0),
        ] {
            assert_eq!(pack_classic_db(db), expected, "{db:?}");
        }
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
    fn switching_analysis_modes_rebuilds_the_active_buffers() {
        let mut processor = SpectrogramProcessor::new(cfg(64, 16, true));
        let mut config = processor.config();
        config.use_reassignment = false;
        processor.update_config(config);
        let classic = processor
            .process_block(&AudioBlock::new(&[0.25; 64], 1, config.sample_rate))
            .expect("expected classic snapshot");
        assert!(matches!(
            classic.new_columns[0],
            SpectrogramColumn::Classic(_)
        ));

        config.use_reassignment = true;
        processor.update_config(config);
        processor.reset_audio();
        let samples = vec![0.25; block_len(64)];
        let reassigned = processor
            .process_block(&AudioBlock::new(&samples, 1, config.sample_rate))
            .expect("expected reassigned snapshot");
        assert!(matches!(
            reassigned.new_columns[0],
            SpectrogramColumn::Reassigned(_)
        ));
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
        let idx = mags
            .iter()
            .enumerate()
            .max_by_key(|&(_, &db)| db)
            .unwrap()
            .0;

        assert_eq!(mags.len(), cfg.fft_size / 2 + 1);
        assert_eq!(idx, 200);
        assert!(mags[idx] >= pack_classic_db(-0.01));
    }

    #[test]
    fn configured_sources_match_projected_input() {
        let mono = sine_wave(3000.0, DEFAULT_SAMPLE_RATE, block_len(64) + 128, 1.0);
        let stereo: Vec<_> = mono
            .iter()
            .flat_map(|&sample| [sample, -0.5 * sample])
            .collect();

        for use_reassignment in [false, true] {
            let config = cfg(64, 16, use_reassignment);
            for (source, mono_gain, stereo_gain) in [
                (Channel::Left, 1.0, 1.0),
                (Channel::Right, 1.0, -0.5),
                (Channel::Mid, 1.0, 0.25),
                (Channel::Side, 0.0, 0.75),
                (Channel::None, 0.0, 0.0),
            ] {
                for (channels, samples, gain) in [
                    (1, mono.as_slice(), mono_gain),
                    (2, stereo.as_slice(), stereo_gain),
                ] {
                    let projected: Vec<_> = mono.iter().map(|sample| sample * gain).collect();
                    let expected = process_samples(config, &projected);
                    let mut processor =
                        SpectrogramProcessor::new(SpectrogramConfig { source, ..config });
                    let actual = processor
                        .process_block(&AudioBlock::new(samples, channels, config.sample_rate))
                        .expect("expected snapshot");

                    assert_eq!(
                        actual.new_columns, expected.new_columns,
                        "{source:?}, {channels} channels"
                    );
                }
            }
        }
    }

    #[test]
    fn source_changes_discard_pending_audio_and_reset_history() {
        let samples: Vec<_> = sine_wave(3000.0, DEFAULT_SAMPLE_RATE, block_len(64) + 128, 1.0)
            .into_iter()
            .flat_map(|sample| [sample, 0.0])
            .collect();
        let block = AudioBlock::new(&samples, 2, DEFAULT_SAMPLE_RATE);

        for use_reassignment in [false, true] {
            for hop_size in [16, 256] {
                let mut config = cfg(64, hop_size, use_reassignment);
                let mut processor = SpectrogramProcessor::new(config);
                assert!(processor.process_block(&block).unwrap().reset);
                assert!(processor.has_buffered_signal() || processor.pending_skip_samples > 0);

                config.history_length += 1;
                processor.update_config(config);
                assert!(!processor.reset);

                config.source = Channel::Right;
                processor.update_config(config);
                assert!(processor.audio_buffer.is_empty());
                assert_eq!(processor.pending_skip_samples, 0);
                assert!(!processor.has_buffered_signal());

                let update = processor
                    .process_block(&block)
                    .expect("expected new source snapshot");
                assert!(update.reset);
                assert!(
                    update
                        .new_columns
                        .iter()
                        .all(SpectrogramColumn::is_quiescent)
                );
                assert!(!processor.reset);
            }
        }
    }

    #[test]
    fn retained_history_matches_full_suffix() {
        let mut full_cfg = cfg(64, 16, false);
        full_cfg.history_length = 32;
        let mut capped_cfg = full_cfg;
        capped_cfg.history_length = 3;
        let samples: Vec<_> = (0..192)
            .map(|i| ((i * i + 3 * i) as f32 * 0.017).sin())
            .collect();

        let full = process_samples(full_cfg, &samples);
        let capped = process_samples(capped_cfg, &samples);
        let expected = &full.new_columns[full.new_columns.len() - capped.new_columns.len()..];

        assert_eq!(capped.new_columns.len(), capped_cfg.history_length);
        assert_ne!(full.new_columns[0], expected[0]);
        assert_eq!(expected, capped.new_columns);
    }

    #[test]
    fn hops_larger_than_the_window_are_block_partition_independent() {
        let cfg = SpectrogramConfig {
            sample_rate: 32.0,
            fft_size: 8,
            hop_size: 16,
            window: WindowKind::Rectangular,
            history_length: 32,
            use_reassignment: false,
            ..Default::default()
        };
        let samples: Vec<_> = (0..29).map(|i| (i as f32 * 0.73).sin()).collect();

        let whole = process_samples(cfg, &samples).new_columns;
        let mut processor = SpectrogramProcessor::new(cfg);
        let mut partitioned = Vec::new();
        for chunk in samples.chunks(8) {
            if let Some(update) = processor.process_block(&AudioBlock::new(chunk, 1, 32.0)) {
                partitioned.extend(update.new_columns);
            }
        }

        assert_eq!(whole, partitioned);
    }

    #[test]
    fn classic_retention_budget_uses_packed_column_width() {
        let points = (16_384 * 32 / 2 + 1) as u32;
        let packed_stride = points.div_ceil(2) as usize * std::mem::size_of::<u32>();
        assert_eq!(
            history_columns(ColumnKind::Classic, points, u32::MAX as usize),
            SPECTROGRAM_HISTORY_BYTE_BUDGET / packed_stride
        );
        assert_eq!(history_columns(ColumnKind::Classic, 100_000_000, 1), 1);
    }

    #[test]
    fn fft_rebuild_keeps_newest_pending_audio() {
        let mut p = SpectrogramProcessor::new(cfg(64, 16, false));
        p.prepare();
        let samples: Vec<_> = (0..200).map(|i| i as f32).collect();
        p.push_audio(&AudioBlock::new(&samples, 1, DEFAULT_SAMPLE_RATE));
        let mut next = p.config();
        next.fft_size = 16;
        p.update_config(next);

        assert_eq!(
            p.audio_buffer.iter().copied().collect::<Vec<_>>(),
            samples[168..]
        );
    }

    #[test]
    fn silent_input_advances_transparent_columns() {
        let samples = vec![0.0; block_len(64) + 256];
        let floor = pack_classic_db(DB_FLOOR);

        let classic = process_samples(cfg(64, 16, false), &samples);
        assert_eq!(classic.new_columns.len(), 4);
        assert!(
            classic
                .new_columns
                .iter()
                .all(|col| classic_mags(col).iter().all(|&mag| mag == floor))
        );

        let reassigned = process_samples(cfg(64, 16, true), &samples);
        assert_eq!(reassigned.new_columns.len(), 4 + REASSIGNED_HISTORY_MARGIN);
        assert!(
            reassigned
                .new_columns
                .iter()
                .all(|col| reassigned_points(col).is_empty())
        );
    }

    #[test]
    fn reassignment_places_peak_frequency_time_and_power() {
        let cfg = SpectrogramConfig {
            zero_padding_factor: 4,
            ..cfg(2048, 512, true)
        };
        let len = block_len(cfg.fft_size);
        let expected_time = -((cfg.fft_size / 2 + 1) as f32) / cfg.hop_size as f32;

        for bin in [3.4, 10.25, 50.25, 200.75, 800.4] {
            let freq = bin * cfg.sample_rate / cfg.fft_size as f32;
            let update = process_sine(cfg, freq, len);
            let points = reassigned_points(update.new_columns.last().unwrap());
            let peak = peak_point(points);

            assert!(
                (peak.freq_hz - freq).abs() < 2.0,
                "reassigned freq {:.4} vs expected {freq:.4}",
                peak.freq_hz
            );
            assert!(
                (peak.time_offset - expected_time).abs() < 5.0e-4,
                "time offset {:.4} vs expected {expected_time:.4}",
                peak.time_offset
            );
            let accumulated_power = points.iter().map(|point| point.power).sum::<f32>();
            let power = accumulated_power * update.reassigned_power_scale;
            assert!((power - 1.0).abs() < 0.01, "deposited {power} power");
            assert!(points.len() < update.fft_size / 2 + 1);
        }
    }

    #[test]
    fn reassignment_resolves_a_low_fractional_fft_bin() {
        let config = SpectrogramConfig {
            zero_padding_factor: 4,
            ..cfg(2048, 512, true)
        };
        let frequency = 1.37 * config.sample_rate / config.fft_size as f32;
        let update = process_sine(config, frequency, block_len(config.fft_size));
        let peak = peak_point(reassigned_points(update.new_columns.last().unwrap()));

        assert!((peak.freq_hz - frequency).abs() < 2.0);
    }

    #[test]
    fn reassignment_removes_constant_dc_without_allocating_points() {
        let config = cfg(64, 16, true);
        let update = process_samples(config, &vec![0.25; block_len(64) + 64]);

        for column in &update.new_columns {
            assert_eq!(reassigned_points(column).capacity(), 0);
        }
    }

    #[test]
    fn reassigned_times_stay_within_the_window() {
        let config = SpectrogramConfig {
            history_length: 512,
            ..cfg(256, 32, true)
        };
        let mut state = 1_u32;
        let noise: Vec<f32> = (0..16_384)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                state as f32 / u32::MAX as f32 - 0.5
            })
            .collect();
        let update = process_samples(config, &noise);
        let half = config.fft_size as f32 * 0.5;
        let hops = |samples: f32| (samples - half - 1.0) / config.hop_size as f32;
        // The smoothed Hann base window adds one nonzero tap after the window.
        let (earliest, latest) = (hops(-half - 0.5), hops(half + 0.5));

        let times = update.new_columns.iter().flat_map(|column| {
            reassigned_points(column)
                .iter()
                .map(|point| point.time_offset)
        });
        assert!(times.clone().count() > 10_000);
        for time in times {
            assert!((earliest..=latest).contains(&time), "time offset {time}");
        }
    }

    #[test]
    fn reassignment_localizes_a_centered_impulse_in_time() {
        let config = cfg(256, 32, true);
        let (read_len, lookahead) =
            SpectrogramProcessor::hilbert_layout(config.fft_size, config.sample_rate);
        let position = config.fft_size / 2;
        // Later columns from the same block leave the first one reading the block's middle.
        let mut samples = vec![0.0; read_len + 3 * config.hop_size];
        samples[read_len - lookahead - config.fft_size + position] = 1.0;
        let update = process_samples(config, &samples);
        let points = reassigned_points(&update.new_columns[0]);
        let expected = (position as f32 - config.fft_size as f32 - 1.0) / config.hop_size as f32;

        assert_eq!(update.new_columns.len(), 4);
        assert!(!points.is_empty() && points.capacity() == update.points_per_column);
        assert!(
            points
                .iter()
                .all(|point| (point.time_offset - expected).abs() < 1.0e-4)
        );
    }
}
