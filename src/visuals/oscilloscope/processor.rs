// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::dsp::{AudioBlock, AudioProcessor, Reconfigurable};
use crate::util::audio::{DEFAULT_SAMPLE_RATE, extend_interleaved_history};
use realfft::{RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;

const PITCH_MIN_HZ: f32 = 20.0;
const PITCH_MAX_HZ: f32 = 8000.0;

// YIN cumulative mean normalized difference (CMND) threshold. A lag
// is accepted as a pitch period when its CMND value drops below
// this. Lower values reject more ambiguous signals; higher values
// accept weaker periodicity.
const PITCH_THRESHOLD: f32 = 0.15;

// Sample count above which the difference function switches from
// O(n*tau) direct computation to O(n log n) FFT-based
// autocorrelation. Below this, the direct method is faster and avoids
// FFT autocorrelation edge differences on short buffers.
const FFT_AUTOCORR_THRESHOLD: usize = 512;

#[inline]
fn parabolic_refine(y_prev: f32, y_curr: f32, y_next: f32, tau: usize) -> f32 {
    let denom = y_prev - 2.0 * y_curr + y_next;
    if denom.abs() < f32::EPSILON {
        return tau as f32;
    }
    let delta = 0.5 * (y_prev - y_next) / denom;
    (tau as f32 + delta.clamp(-1.0, 1.0)).max(1.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TriggerMode {
    ZeroCrossing,
    Stable { num_cycles: usize },
}

impl Default for TriggerMode {
    fn default() -> Self {
        Self::Stable { num_cycles: 2 }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct OscilloscopeConfig {
    pub sample_rate: f32,
    pub segment_duration: f32,
    pub trigger_mode: TriggerMode,
}

impl Default for OscilloscopeConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            segment_duration: 0.02,
            trigger_mode: TriggerMode::default(),
        }
    }
}

#[derive(Clone)]
struct PitchDetector {
    difference_function: Vec<f32>,
    cumulative_mean_normalized: Vec<f32>,
    last_cmnd_min: f32,
    fft_size: usize,
    fft_forward: Option<Arc<dyn RealToComplex<f32>>>,
    fft_inverse: Option<Arc<dyn realfft::ComplexToReal<f32>>>,
    fft_input: Vec<f32>,
    fft_spectrum: Vec<Complex<f32>>,
    fft_output: Vec<f32>,
    fft_scratch: Vec<Complex<f32>>,
}

impl PitchDetector {
    fn new() -> Self {
        Self {
            difference_function: Vec::new(),
            cumulative_mean_normalized: Vec::new(),
            last_cmnd_min: 1.0,
            fft_size: 0,
            fft_forward: None,
            fft_inverse: None,
            fft_input: Vec::new(),
            fft_spectrum: Vec::new(),
            fft_output: Vec::new(),
            fft_scratch: Vec::new(),
        }
    }

    fn rebuild_fft(&mut self, size: usize) {
        if self.fft_size == size && self.fft_forward.is_some() {
            return;
        }
        self.fft_size = size;
        let mut planner = RealFftPlanner::new();
        let forward = planner.plan_fft_forward(size);
        let inverse = planner.plan_fft_inverse(size);
        self.fft_input = forward.make_input_vec();
        self.fft_spectrum = forward.make_output_vec();
        self.fft_output = inverse.make_output_vec();
        self.fft_scratch = forward.make_scratch_vec();
        let inv_scratch = inverse.make_scratch_vec();
        if inv_scratch.len() > self.fft_scratch.len() {
            self.fft_scratch = inv_scratch;
        }
        self.fft_forward = Some(forward);
        self.fft_inverse = Some(inverse);
    }

    fn detect_pitch(&mut self, samples: &[f32], rate: f32) -> Option<f32> {
        if samples.is_empty() {
            return None;
        }
        let min_period = (rate / PITCH_MAX_HZ).max(2.0) as usize;
        let max_period = (rate / PITCH_MIN_HZ).min(samples.len() as f32 / 2.0) as usize;
        if max_period <= min_period || samples.len() < max_period * 2 {
            return None;
        }

        // Compute difference function (YIN step 2)
        self.difference_function.resize(max_period, 0.0);
        let use_fft = samples.len() >= FFT_AUTOCORR_THRESHOLD;
        if !use_fft || !self.compute_diff_fft(samples, max_period) {
            self.compute_diff_direct(samples, max_period);
        }

        // Cumulative mean normalized difference (YIN step 3)
        self.cumulative_mean_normalized.resize(max_period, 0.0);
        self.cumulative_mean_normalized[0] = 1.0;
        let mut sum = 0.0;
        for tau in 1..max_period {
            sum += self.difference_function[tau];
            self.cumulative_mean_normalized[tau] = if sum > f32::EPSILON {
                self.difference_function[tau] * tau as f32 / sum
            } else {
                1.0
            };
        }

        // Find first minimum below threshold (YIN step 4)
        for tau in min_period..max_period - 1 {
            if self.cumulative_mean_normalized[tau] < PITCH_THRESHOLD
                && self.cumulative_mean_normalized[tau] < self.cumulative_mean_normalized[tau + 1]
            {
                self.last_cmnd_min = self.cumulative_mean_normalized[tau];
                return Some(rate / self.refine_tau(tau, max_period));
            }
        }

        // Fallback: absolute minimum if good enough
        let (best_tau, best_val) = (min_period..max_period)
            .map(|t| (t, self.cumulative_mean_normalized[t]))
            .min_by(|a, b| a.1.total_cmp(&b.1))?;
        if best_val < 0.6 {
            self.last_cmnd_min = best_val;
            Some(rate / self.refine_tau(best_tau, max_period))
        } else {
            None
        }
    }

    #[inline]
    fn refine_tau(&self, tau: usize, max_period: usize) -> f32 {
        if tau > 0 && tau + 1 < max_period {
            parabolic_refine(
                self.cumulative_mean_normalized[tau - 1],
                self.cumulative_mean_normalized[tau],
                self.cumulative_mean_normalized[tau + 1],
                tau,
            )
        } else {
            tau as f32
        }
    }

    fn compute_diff_fft(&mut self, samples: &[f32], max_period: usize) -> bool {
        let fft_size = (samples.len() * 2).next_power_of_two();
        self.rebuild_fft(fft_size);

        let (Some(forward), Some(inverse)) = (self.fft_forward.as_ref(), self.fft_inverse.as_ref())
        else {
            return false;
        };

        self.fft_input[..samples.len()].copy_from_slice(samples);
        self.fft_input[samples.len()..].fill(0.0);

        if forward
            .process_with_scratch(
                &mut self.fft_input,
                &mut self.fft_spectrum,
                &mut self.fft_scratch,
            )
            .is_err()
        {
            return false;
        }

        // Power spectrum: |X(f)|^2 -> IFFT gives autocorrelation
        for c in &mut self.fft_spectrum {
            *c = Complex::new(c.norm_sqr(), 0.0);
        }

        if inverse
            .process_with_scratch(
                &mut self.fft_spectrum,
                &mut self.fft_output,
                &mut self.fft_scratch,
            )
            .is_err()
        {
            return false;
        }

        let norm = 1.0 / fft_size as f32;
        let acf_0 = self.fft_output[0] * norm;
        for tau in 0..max_period {
            self.difference_function[tau] = 2.0 * (acf_0 - self.fft_output[tau] * norm);
        }
        true
    }

    fn compute_diff_direct(&mut self, samples: &[f32], max_period: usize) {
        let len = samples.len() - max_period;
        for tau in 0..max_period {
            let mut sum = 0.0_f32;
            for i in 0..len {
                let delta = samples[i] - samples[i + tau];
                sum += delta * delta;
            }
            self.difference_function[tau] = sum;
        }
    }
}

#[derive(Debug, Clone, Default)]
struct TriggerScratch {
    sine_prefix_sum: Vec<f32>,
    cosine_prefix_sum: Vec<f32>,
    phase_sine: Vec<f32>,
    phase_cosine: Vec<f32>,
}

impl TriggerScratch {
    fn clear(&mut self) {
        self.sine_prefix_sum.clear();
        self.cosine_prefix_sum.clear();
        self.phase_sine.clear();
        self.phase_cosine.clear();
    }

    fn prepare(&mut self, data: &[f32], period: f32) {
        let len = data.len();
        let n = len + 1;
        self.clear();
        self.sine_prefix_sum.resize(n, 0.0);
        self.cosine_prefix_sum.resize(n, 0.0);
        self.phase_sine.resize(n, 0.0);
        self.phase_cosine.resize(n, 0.0);

        let step = std::f32::consts::TAU / period;
        let (step_sine, step_cosine) = step.sin_cos();

        let mut sine_value = 0.0_f32;
        let mut cosine_value = 1.0_f32;

        self.phase_sine[0] = sine_value;
        self.phase_cosine[0] = cosine_value;

        for (i, &sample) in data.iter().enumerate() {
            self.sine_prefix_sum[i + 1] = self.sine_prefix_sum[i] + sample * sine_value;
            self.cosine_prefix_sum[i + 1] = self.cosine_prefix_sum[i] + sample * cosine_value;

            let mut next_sine = sine_value * step_cosine + cosine_value * step_sine;
            let mut next_cosine = cosine_value * step_cosine - sine_value * step_sine;

            // Periodically reset to exact values to prevent phase and magnitude drift
            if (i & 127) == 127 {
                let exact_angle = step * (i + 1) as f32;
                (next_sine, next_cosine) = exact_angle.sin_cos();
            }

            sine_value = next_sine;
            cosine_value = next_cosine;

            self.phase_sine[i + 1] = sine_value;
            self.phase_cosine[i + 1] = cosine_value;
        }
    }

    #[inline]
    fn correlation(&self, offset: usize, length: usize) -> f32 {
        debug_assert!(offset + length < self.sine_prefix_sum.len());
        debug_assert!(offset < self.phase_cosine.len());

        let ss = self.sine_prefix_sum[offset + length] - self.sine_prefix_sum[offset];
        let sc = self.cosine_prefix_sum[offset + length] - self.cosine_prefix_sum[offset];
        self.phase_cosine[offset] * ss - self.phase_sine[offset] * sc
    }
}

/// Snaps `new_f` to the octave of `prev_f` when YIN jumps by a
/// factor of ~2 or ~0.5.
#[inline]
fn octave_correct(new_f: f32, prev_f: f32) -> f32 {
    let ratio = new_f / prev_f;
    if (1.9..=2.1).contains(&ratio) {
        new_f * 0.5
    } else if (0.48..=0.52).contains(&ratio) {
        new_f * 2.0
    } else {
        new_f
    }
}

#[inline]
fn find_trigger(
    period: f32,
    cycles: usize,
    available: usize,
    mono: &[f32],
    scratch: &mut TriggerScratch,
) -> (usize, usize, f32) {
    let cycles = cycles.max(1);
    let len = (period * cycles as f32).round() as usize;
    let guard = period.ceil() as usize;
    let window = len.saturating_add(guard);
    let start = available.saturating_sub(window);

    let data = &mono[start..];
    if period < 1.0 || len == 0 {
        return (0, available, 0.0);
    }

    if data.len() <= len {
        return (len, start, 0.0);
    }

    scratch.prepare(data, period);

    let range = data.len() - len;
    let stride = ((period / 4.0) as usize).max(1);

    let mut best = f32::NEG_INFINITY;
    let mut pos = 0;

    // Coarse sweep right-to-left to prioritize newer data.
    let num_steps = range / stride;
    for step in (0..=num_steps).rev() {
        let i = step * stride;
        let corr = scratch.correlation(i, len);
        if corr > best {
            best = corr;
            pos = i;
        }
    }

    let refine_start = pos.saturating_sub(stride);
    let refine_end = (pos + stride).min(range);

    for i in (refine_start..=refine_end).rev() {
        if i == pos || i % stride == 0 {
            continue;
        }
        let corr = scratch.correlation(i, len);
        if corr > best {
            best = corr;
            pos = i;
        }
    }

    let frac = if period > 40.0 && pos > 0 && pos < range {
        let c0 = scratch.correlation(pos - 1, len);
        let c1 = scratch.correlation(pos, len);
        let c2 = scratch.correlation(pos + 1, len);
        let denom = c0 - 2.0 * c1 + c2;
        if denom.abs() > f32::EPSILON {
            (0.5 * (c0 - c2) / denom).clamp(-0.5, 0.5)
        } else {
            0.0
        }
    } else {
        0.0
    };

    (len, start + pos, frac)
}

fn find_rising_zero_crossing(
    interleaved: &[f32],
    channels: usize,
    frames: impl Iterator<Item = usize>,
) -> Option<usize> {
    let scale = 1.0 / channels.max(1) as f32;
    let mono = |f: usize| {
        let b = f * channels;
        (0..channels).map(|c| interleaved[b + c]).sum::<f32>() * scale
    };
    let mut it = frames;
    let first = it.next()?;
    let mut prev_val = mono(first);
    let mut prev_idx = first;
    for f in it {
        let cur = mono(f);
        // Always check in temporal order regardless of iteration direction
        let (lo_val, hi_idx, hi_val) = if f > prev_idx {
            (prev_val, f, cur)
        } else {
            (cur, prev_idx, prev_val)
        };
        if hi_val > 0.0 && lo_val <= 0.0 {
            return Some(hi_idx);
        }
        prev_val = cur;
        prev_idx = f;
    }
    None
}

#[derive(Debug, Clone, Default)]
pub struct OscilloscopeSnapshot {
    pub channels: usize,
    pub samples: Vec<f32>,
    pub samples_per_channel: usize,
}

#[derive(Clone)]
pub struct OscilloscopeProcessor {
    config: OscilloscopeConfig,
    snapshot: OscilloscopeSnapshot,
    history: VecDeque<f32>,
    pitch_detector: PitchDetector,
    last_pitch: Option<f32>,
    mono_buffer: Vec<f32>,
    trigger_scratch: TriggerScratch,
    octave_streak: u32,
}

impl OscilloscopeProcessor {
    pub fn new(config: OscilloscopeConfig) -> Self {
        Self {
            config,
            snapshot: OscilloscopeSnapshot::default(),
            history: VecDeque::new(),
            pitch_detector: PitchDetector::new(),
            last_pitch: None,
            mono_buffer: Vec::new(),
            trigger_scratch: TriggerScratch::default(),
            octave_streak: 0,
        }
    }

    pub fn config(&self) -> OscilloscopeConfig {
        self.config
    }

    fn stabilize_pitch(&mut self, detected: Option<f32>) -> Option<f32> {
        match (detected, self.last_pitch) {
            (Some(new_f), Some(prev_f)) => {
                let corrected = octave_correct(new_f, prev_f);
                let was_corrected = (corrected - new_f).abs() > f32::EPSILON;
                self.octave_streak = if was_corrected {
                    self.octave_streak + 1
                } else {
                    0
                };

                // After 3+ consecutive octave corrections the signal
                // has probably actually changed, so accept it.
                let pitch = if was_corrected && self.octave_streak >= 3 {
                    self.octave_streak = 0;
                    new_f
                } else {
                    corrected
                };

                let ratio = pitch / prev_f;
                if (0.9..=1.1).contains(&ratio) {
                    let cmnd = self.pitch_detector.last_cmnd_min;
                    let confidence = (1.0 - cmnd).clamp(0.0, 1.0);
                    let alpha = 0.15 + 0.50 * confidence;
                    Some(prev_f + alpha * (pitch - prev_f))
                } else {
                    Some(pitch)
                }
            }
            (Some(f), None) => Some(f),
            (None, prev) => prev,
        }
    }
}

impl AudioProcessor for OscilloscopeProcessor {
    type Output = OscilloscopeSnapshot;

    fn process_block(&mut self, block: &AudioBlock<'_>) -> Option<Self::Output> {
        let channel_count = block.channels.max(1);
        if block.frame_count() == 0 {
            return None;
        }

        let sample_rate = block.sample_rate.max(1.0);
        if (self.config.sample_rate - sample_rate).abs() > f32::EPSILON {
            let mut config = self.config;
            config.sample_rate = sample_rate;
            self.update_config(config);
        }

        let base_frames = (self.config.sample_rate * self.config.segment_duration)
            .round()
            .max(1.0) as usize;

        let detection_frames = (self.config.sample_rate * 0.1) as usize;
        let search_range = (self.config.sample_rate / PITCH_MIN_HZ).ceil() as usize;
        let trigger_frames = match self.config.trigger_mode {
            TriggerMode::ZeroCrossing => base_frames + search_range,
            TriggerMode::Stable { num_cycles } => {
                let max_period = (self.config.sample_rate / PITCH_MIN_HZ) as usize;
                max_period * (num_cycles.max(1) + 1)
            }
        };
        let capacity = detection_frames.max(base_frames).max(trigger_frames) * channel_count;

        if !self.history.is_empty() && !self.history.len().is_multiple_of(channel_count) {
            self.history.clear();
            self.last_pitch = None;
        }
        extend_interleaved_history(&mut self.history, block.samples, capacity, channel_count);

        let available = self.history.len() / channel_count;

        let (frames, start, frac_offset) = match self.config.trigger_mode {
            TriggerMode::ZeroCrossing => {
                let frames = base_frames.min(available);
                if frames == 0 {
                    return None;
                }

                let data = self.history.make_contiguous();
                let end = available.saturating_sub(1);
                let right_lo = end.saturating_sub(search_range);
                let right = find_rising_zero_crossing(data, channel_count, (right_lo..=end).rev())
                    .unwrap_or(available);

                let left_lo = right.saturating_sub(frames);
                let left_hi = (left_lo + search_range).min(right.saturating_sub(2));
                let left = find_rising_zero_crossing(data, channel_count, left_lo..=left_hi)
                    .unwrap_or(left_lo);

                (right.saturating_sub(left).max(1), left, 0.0)
            }
            TriggerMode::Stable { num_cycles } => {
                if available < base_frames {
                    return None;
                }

                let data = self.history.make_contiguous();
                self.mono_buffer.clear();
                let scale = 1.0 / channel_count as f32;
                self.mono_buffer.extend(
                    data.chunks_exact(channel_count)
                        .take(available)
                        .map(|frame| frame.iter().sum::<f32>() * scale),
                );

                let detected = self
                    .pitch_detector
                    .detect_pitch(&self.mono_buffer, self.config.sample_rate);

                // Retry on the most recent portion when full-buffer detection fails
                // (e.g. buffer spans a signal transition).
                let detected = detected.or_else(|| {
                    let min_len = (self.config.sample_rate / PITCH_MIN_HZ) as usize * 2;
                    if self.mono_buffer.len() > min_len {
                        let start = self.mono_buffer.len() - min_len;
                        self.pitch_detector
                            .detect_pitch(&self.mono_buffer[start..], self.config.sample_rate)
                    } else {
                        None
                    }
                });

                let freq = self.stabilize_pitch(detected);

                if let Some(f) = freq {
                    self.last_pitch = Some(f);
                    let period = (self.config.sample_rate / f).max(1.0);
                    find_trigger(
                        period,
                        num_cycles,
                        available,
                        &self.mono_buffer,
                        &mut self.trigger_scratch,
                    )
                } else {
                    (base_frames, available.saturating_sub(base_frames), 0.0)
                }
            }
        };

        const TARGET: usize = 4096;
        let target = TARGET.clamp(1, frames);
        let data = self.history.make_contiguous();
        let extract_start = (start * channel_count).min(data.len());
        let extract_len = (frames * channel_count).min(data.len().saturating_sub(extract_start));

        self.snapshot.samples.clear();
        downsample_interleaved(
            &mut self.snapshot.samples,
            &data[extract_start..extract_start + extract_len],
            frames.min(extract_len / channel_count),
            channel_count,
            target,
            frac_offset,
        );
        self.snapshot.channels = channel_count;
        self.snapshot.samples_per_channel = target;

        Some(self.snapshot.clone())
    }

    fn reset(&mut self) {
        self.snapshot = OscilloscopeSnapshot::default();
        self.history = VecDeque::new();
        self.pitch_detector = PitchDetector::new();
        self.last_pitch = None;
        self.mono_buffer = Vec::new();
        self.trigger_scratch = TriggerScratch::default();
        self.octave_streak = 0;
    }
}

impl Reconfigurable<OscilloscopeConfig> for OscilloscopeProcessor {
    fn update_config(&mut self, config: OscilloscopeConfig) {
        self.config = config;
        self.reset();
    }
}

fn downsample_interleaved(
    output: &mut Vec<f32>,
    data: &[f32],
    frames: usize,
    channel_count: usize,
    target: usize,
    frac_offset: f32,
) {
    if frames == 0 || channel_count == 0 || target == 0 {
        return;
    }

    let step = frames as f32 / target as f32;

    for channel in 0..channel_count {
        for i in 0..target {
            let pos = (frac_offset + i as f32 * step).max(0.0);
            let idx = (pos as usize).min(frames - 1);
            let frac = pos - idx as f32;

            let base = idx * channel_count + channel;
            let sample = if frac > f32::EPSILON && idx + 1 < frames {
                crate::util::audio::lerp(data[base], data[base + channel_count], frac)
            } else {
                data[base]
            };

            output.push(sample);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::AudioBlock;
    use std::{ops::Range, time::Instant};

    const RATE: f32 = 48_000.0;
    const BLOCK: usize = 1024;
    const TAU: f32 = std::f32::consts::TAU;

    fn make_block(samples: &[f32], channels: usize, sample_rate: f32) -> AudioBlock<'_> {
        AudioBlock::new(samples, channels, sample_rate, Instant::now())
    }

    fn stable_config() -> OscilloscopeConfig {
        OscilloscopeConfig {
            sample_rate: RATE,
            segment_duration: 0.02,
            trigger_mode: TriggerMode::Stable { num_cycles: 2 },
        }
    }

    fn sine_samples(freq: f32, rate: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|n| (TAU * freq * n as f32 / rate).sin())
            .collect()
    }

    fn feed_blocks(processor: &mut OscilloscopeProcessor, signal: &[f32], blocks: Range<usize>) {
        let _ = first_block_where(processor, signal, blocks, |_| false);
    }

    fn first_block_where(
        processor: &mut OscilloscopeProcessor,
        signal: &[f32],
        blocks: Range<usize>,
        predicate: impl Fn(&OscilloscopeProcessor) -> bool,
    ) -> Option<usize> {
        let start = blocks.start;
        for block in blocks {
            let offset = block * BLOCK;
            processor.process_block(&make_block(&signal[offset..offset + BLOCK], 1, RATE));
            if predicate(processor) {
                return Some(block - start);
            }
        }
        None
    }

    fn assert_lock_within(
        processor: &mut OscilloscopeProcessor,
        signal: &[f32],
        blocks: Range<usize>,
        limit: usize,
        label: &str,
        predicate: impl Fn(&OscilloscopeProcessor) -> bool,
    ) {
        let took = first_block_where(processor, signal, blocks, predicate).expect(label);
        assert!(took <= limit, "{label} within {limit} blocks, took {took}");
    }

    fn frequency_switch_signal(from: f32, to: f32, warmup: usize, after: usize) -> Vec<f32> {
        let switch = warmup * BLOCK;
        (0..BLOCK * (warmup + after))
            .map(|n| {
                let t = n as f32 / RATE;
                if n < switch {
                    (TAU * from * t).sin()
                } else {
                    let t0 = switch as f32 / RATE;
                    let phase0 = TAU * from * t0;
                    (phase0 + TAU * to * (t - t0)).sin()
                }
            })
            .collect()
    }

    fn delayed_sine(freq: f32, silence: usize, signal_blocks: usize) -> Vec<f32> {
        let onset = silence * BLOCK;
        (0..BLOCK * (silence + signal_blocks))
            .map(|n| {
                if n >= onset {
                    (TAU * freq * (n - onset) as f32 / RATE).sin()
                } else {
                    0.0
                }
            })
            .collect()
    }

    fn assert_detects_pitch(
        detector: &mut PitchDetector,
        freq: f32,
        frames: usize,
        max_error: f32,
    ) {
        let samples = sine_samples(freq, RATE, frames);
        let detected = detector.detect_pitch(&samples, RATE).expect("pitch");
        let error = (detected - freq).abs() / freq;
        assert!(error < max_error, "got {detected}Hz, expected {freq}Hz");
    }

    #[test]
    fn pitch_detection() {
        let mut detector = PitchDetector::new();
        for freq in [41.0, 110.0, 440.0, 1000.0, 4000.0] {
            assert_detects_pitch(&mut detector, freq, (RATE * 0.1) as usize, 0.02);
        }
    }

    #[test]
    fn short_buffer_pitch_detection_uses_direct_difference() {
        let mut detector = PitchDetector::new();
        assert_detects_pitch(&mut detector, 1000.0, 256, 0.03);
    }

    #[test]
    fn parabolic_interpolation() {
        let y = |x: f32| (x - 5.3_f32).powi(2);
        assert!((parabolic_refine(y(4.0), y(5.0), y(6.0), 5) - 5.3).abs() < 0.001);
    }

    #[test]
    fn zero_crossing_both_directions_and_stereo() {
        let mono = sine_samples(440.0, RATE, 4800);
        let backward = find_rising_zero_crossing(&mono, 1, (0..=3840).rev()).unwrap();
        let forward = find_rising_zero_crossing(&mono, 1, 0..=4799).unwrap();
        for c in [backward, forward] {
            assert!(mono[c] > 0.0 && mono[c - 1] <= 0.0);
        }

        let stereo: Vec<f32> = mono.iter().flat_map(|&s| [s, s]).collect();
        let c = find_rising_zero_crossing(&stereo, 2, (0..=3840).rev()).unwrap();
        let (m, p) = (
            (stereo[c * 2] + stereo[c * 2 + 1]) * 0.5,
            (stereo[(c - 1) * 2] + stereo[(c - 1) * 2 + 1]) * 0.5,
        );
        assert!(m > 0.0 && p <= 0.0);
    }

    #[test]
    fn zero_crossing_both_edges_near_zero() {
        let config = OscilloscopeConfig {
            segment_duration: 0.01,
            trigger_mode: TriggerMode::ZeroCrossing,
            ..Default::default()
        };
        let mut processor = OscilloscopeProcessor::new(config);
        let mono = sine_samples(
            440.0,
            config.sample_rate,
            (config.sample_rate * 0.1) as usize,
        );
        let samples: Vec<_> = mono.into_iter().flat_map(|s| [s, s]).collect();
        let snap = processor
            .process_block(&make_block(&samples, 2, config.sample_rate))
            .expect("expected snapshot");

        assert_eq!(snap.channels, 2);
        assert!(snap.samples_per_channel > 0 && snap.samples_per_channel <= 4096);
        assert_eq!(snap.samples.len(), snap.samples_per_channel * 2);
        let n = snap.samples_per_channel;
        assert!(snap.samples[0] > 0.0 && snap.samples[0] < 0.15, "left edge");
        assert!(snap.samples[n - 1].abs() < 0.15, "right edge");
    }

    #[test]
    fn lock_acquisition_and_frequency_transitions() {
        let mut processor = OscilloscopeProcessor::new(stable_config());
        let signal = sine_samples(440.0, RATE, BLOCK * 20);
        assert_lock_within(
            &mut processor,
            &signal,
            0..20,
            10,
            "lock on clean sine",
            |p| p.last_pitch.is_some(),
        );

        let mut processor = OscilloscopeProcessor::new(stable_config());
        let (warmup, after) = (20, 20);
        let signal = frequency_switch_signal(440.0, 880.0, warmup, after);
        feed_blocks(&mut processor, &signal, 0..warmup);
        let pre = processor.last_pitch.expect("pitch after warmup");
        assert!(
            (pre - 440.0).abs() < 20.0,
            "pre-transition pitch was {pre:.1}"
        );
        assert_lock_within(
            &mut processor,
            &signal,
            warmup..warmup + after,
            10,
            "adapt to 880Hz",
            |p| {
                p.last_pitch
                    .is_some_and(|pitch| (pitch - 880.0).abs() < 50.0)
            },
        );

        let mut processor = OscilloscopeProcessor::new(stable_config());
        let (silence, signal_blocks) = (10, 20);
        let signal = delayed_sine(440.0, silence, signal_blocks);
        feed_blocks(&mut processor, &signal, 0..silence);
        assert!(
            processor.last_pitch.is_none(),
            "should have no pitch during silence"
        );
        assert_lock_within(
            &mut processor,
            &signal,
            silence..silence + signal_blocks,
            10,
            "lock after signal onset",
            |p| p.last_pitch.is_some(),
        );
    }
}
