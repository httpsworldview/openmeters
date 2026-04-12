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
// autocorrelation. 512 is the crossover point where FFT overhead is
// amortized by the larger inner loop savings; below this the direct
// method is faster due to cache locality and no FFT setup cost.
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

impl std::fmt::Debug for PitchDetector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PitchDetector")
            .field("diff", &self.difference_function.len())
            .field("cmean", &self.cumulative_mean_normalized.len())
            .field("last_cmnd_min", &self.last_cmnd_min)
            .field("fft_size", &self.fft_size)
            .field("has_fft", &self.fft_forward.is_some())
            .finish()
    }
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
        if len == 0 {
            return;
        }

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

        self.sine_prefix_sum.clear();
        self.cosine_prefix_sum.clear();
        self.phase_sine.clear();
        self.phase_cosine.clear();

        self.sine_prefix_sum.resize(len + 1, 0.0);
        self.cosine_prefix_sum.resize(len + 1, 0.0);
        self.phase_sine.resize(len + 1, 0.0);
        self.phase_cosine.resize(len + 1, 0.0);

        let step = std::f32::consts::TAU / period;
        let (step_sine, step_cosine) = step.sin_cos();

        let mut sine_value = 0.0_f32;
        let mut cosine_value = 1.0_f32;

        self.phase_sine[0] = sine_value;
        self.phase_cosine[0] = cosine_value;

        for (i, &sample) in data.iter().take(len).enumerate() {
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

#[derive(Debug, Clone)]
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
                self.mono_buffer.reserve(available);
                if channel_count == 1 {
                    self.mono_buffer.extend_from_slice(&data[..available]);
                } else {
                    let scale = 1.0 / channel_count as f32;
                    for i in 0..available {
                        let idx = i * channel_count;
                        let sum: f32 = (0..channel_count).map(|c| data[idx + c]).sum();
                        self.mono_buffer.push(sum * scale);
                    }
                }

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
        self.history.clear();
        self.last_pitch = None;
        self.mono_buffer.clear();
        self.trigger_scratch.clear();
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
    use std::time::Instant;

    fn make_block(samples: &[f32], channels: usize, sample_rate: f32) -> AudioBlock<'_> {
        AudioBlock::new(samples, channels, sample_rate, Instant::now())
    }

    fn sine_samples(freq: f32, rate: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|n| (std::f32::consts::TAU * freq * n as f32 / rate).sin())
            .collect()
    }

    fn generate_signal(
        freq: f32,
        rate: f32,
        frames: usize,
        harmonics: &[(f32, f32)],
        noise: f32,
    ) -> Vec<f32> {
        (0..frames)
            .map(|n| {
                let t = n as f32 / rate;
                let mut s = (std::f32::consts::TAU * freq * t).sin();
                for &(mult, amp) in harmonics {
                    s += amp * (std::f32::consts::TAU * freq * mult * t).sin();
                }
                if noise > 0.0 {
                    let pseudo = ((n as f32 * 1.618033) % 1.0) * 2.0 - 1.0;
                    s += pseudo * noise;
                }
                s
            })
            .collect()
    }

    #[test]
    fn produces_downsampled_snapshot_when_buffer_ready() {
        let config = OscilloscopeConfig {
            segment_duration: 0.01,
            trigger_mode: TriggerMode::ZeroCrossing,
            ..Default::default()
        };
        let mut processor = OscilloscopeProcessor::new(config);
        let frames = (config.sample_rate * config.segment_duration).round() as usize;
        let samples: Vec<f32> = (0..frames)
            .flat_map(|f| {
                let s = (f as f32 / frames as f32 * std::f32::consts::TAU).sin();
                [s, -s]
            })
            .collect();

        let Some(s) = processor.process_block(&make_block(&samples, 2, DEFAULT_SAMPLE_RATE)) else {
            panic!("expected snapshot");
        };
        assert_eq!(s.channels, 2);
        assert!(s.samples_per_channel > 0 && s.samples_per_channel <= 4096);
        assert_eq!(s.samples.len(), s.samples_per_channel * 2);
    }

    #[test]
    fn pitch_detection() {
        let mut detector = PitchDetector::new();
        let rate = 48_000.0;

        for freq in [41.0, 110.0, 440.0, 1000.0, 4000.0] {
            let samples = sine_samples(freq, rate, (rate * 0.1) as usize);
            let detected = detector
                .detect_pitch(&samples, rate)
                .expect("Failed to detect pitch");
            let error = (detected - freq).abs() / freq;
            let pct = error * 100.0;
            assert!(
                error < 0.02,
                "Detected {detected}Hz, expected {freq}Hz (error {pct:.1}%)"
            );
        }
    }

    #[test]
    fn parabolic_interpolation() {
        let y = |x: f32| (x - 5.3_f32).powi(2);
        let refined = parabolic_refine(y(4.0), y(5.0), y(6.0), 5);
        assert!((refined - 5.3).abs() < 0.001);
    }

    #[test]
    fn zero_crossing_both_directions_and_stereo() {
        let rate = 48_000.0;
        let mono = sine_samples(440.0, rate, 4800);

        // backward
        let c = find_rising_zero_crossing(&mono, 1, (0..=3840).rev()).unwrap();
        assert!(mono[c] > 0.0 && mono[c - 1] <= 0.0);

        // forward
        let c = find_rising_zero_crossing(&mono, 1, 0..=4799).unwrap();
        assert!(mono[c] > 0.0 && mono[c - 1] <= 0.0);

        // stereo
        let stereo: Vec<f32> = mono.iter().flat_map(|&s| [s, s]).collect();
        let c = find_rising_zero_crossing(&stereo, 2, (0..=3840).rev()).unwrap();
        let m = (stereo[c * 2] + stereo[c * 2 + 1]) / 2.0;
        let p = (stereo[(c - 1) * 2] + stereo[(c - 1) * 2 + 1]) / 2.0;
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
        let rate = config.sample_rate;
        let samples = sine_samples(440.0, rate, (rate * 0.1) as usize);
        let snap = processor
            .process_block(&make_block(&samples, 1, rate))
            .expect("expected snapshot");

        let n = snap.samples_per_channel;
        assert!(snap.samples[0] > 0.0 && snap.samples[0] < 0.15, "left edge");
        assert!(snap.samples[n - 1].abs() < 0.15, "right edge");
    }

    /// Measures pitch detection consistency across sequential blocks.
    #[test]
    fn pitch_consistency_benchmark() {
        let rate = 48_000.0;
        let block_frames = 2048;
        let num_blocks = 40;

        type PitchScenario = (&'static str, f32, &'static [(f32, f32)]);
        let scenarios: &[PitchScenario] = &[
            ("440Hz pure", 440.0, &[]),
            ("100Hz pure", 100.0, &[]),
            ("440Hz harmonics", 440.0, &[(2.0, 0.5), (3.0, 0.25)]),
        ];

        for &(name, freq, harmonics) in scenarios {
            let config = OscilloscopeConfig {
                sample_rate: rate,
                segment_duration: 0.02,
                trigger_mode: TriggerMode::Stable { num_cycles: 2 },
            };
            let mut processor = OscilloscopeProcessor::new(config);

            let total_frames = block_frames * num_blocks;
            let full_signal = generate_signal(freq, rate, total_frames, harmonics, 0.0);

            let mut pitches = Vec::new();
            for block_idx in 0..num_blocks {
                let offset = block_idx * block_frames;
                let block_data = &full_signal[offset..offset + block_frames];
                processor.process_block(&make_block(block_data, 1, rate));

                if let Some(pitch) = processor.last_pitch {
                    pitches.push(pitch);
                }
            }

            assert!(!pitches.is_empty(), "[{name}] no pitches detected");

            let octave_errors = pitches
                .windows(2)
                .filter(|w| !(0.55..=1.8).contains(&(w[1] / w[0])))
                .count();

            let mean = pitches.iter().sum::<f32>() / pitches.len() as f32;
            let variance =
                pitches.iter().map(|p| (p - mean).powi(2)).sum::<f32>() / pitches.len() as f32;
            let std_dev = variance.sqrt();
            let rel_std = std_dev / freq;

            eprintln!(
                "[{name}] pitches={}, mean={mean:.2}Hz, stddev={std_dev:.4}Hz, rel_std={rel_std:.6}, octave_errors={octave_errors}",
                pitches.len()
            );

            assert_eq!(
                octave_errors, 0,
                "[{name}] had {octave_errors} octave errors"
            );
            assert!(
                rel_std < 0.05,
                "[{name}] relative stddev {rel_std:.4} exceeds 5%"
            );
        }
    }

    /// Measures trigger stability by feeding sequential blocks of a continuous
    /// signal and checking how much the first sample varies
    /// between consecutive snapshots.
    #[test]
    fn trigger_stability() {
        let rate = 48_000.0_f32;
        let num_blocks = 60;
        let warmup = 10;

        type TriggerScenario = (&'static str, f32, f32, &'static [(f32, f32)], usize);
        let scenarios: &[TriggerScenario] = &[
            ("440Hz clean", 440.0, 0.0, &[], 1024),
            ("100Hz clean", 100.0, 0.0, &[], 1024),
            ("2000Hz clean", 2000.0, 0.0, &[], 1024),
            ("440Hz noisy", 440.0, 0.05, &[], 1024),
            (
                "440Hz+880Hz harmonics",
                440.0,
                0.0,
                &[(2.0, 0.5), (3.0, 0.25)],
                1024,
            ),
            (
                "82Hz low E string",
                82.41,
                0.01,
                &[(2.0, 0.6), (3.0, 0.4), (4.0, 0.2), (5.0, 0.1)],
                1024,
            ),
            (
                "440Hz rich harmonics+noise",
                440.0,
                0.03,
                &[(2.0, 0.5), (3.0, 0.25), (4.0, 0.12)],
                1024,
            ),
            ("440Hz small blocks", 440.0, 0.0, &[], 256),
            (
                "200Hz sawtooth-like",
                200.0,
                0.0,
                &[
                    (2.0, 0.5),
                    (3.0, 0.33),
                    (4.0, 0.25),
                    (5.0, 0.2),
                    (6.0, 0.167),
                ],
                1024,
            ),
        ];

        for &(name, freq, noise, harmonics, block_frames) in scenarios {
            let config = OscilloscopeConfig {
                sample_rate: rate,
                segment_duration: 0.02,
                trigger_mode: TriggerMode::Stable { num_cycles: 2 },
            };
            let mut processor = OscilloscopeProcessor::new(config);

            let total_frames = block_frames * num_blocks;
            let full_signal = generate_signal(freq, rate, total_frames, harmonics, noise);

            let mut first_samples = Vec::new();
            for block_idx in 0..num_blocks {
                let offset = block_idx * block_frames;
                let block_data = &full_signal[offset..offset + block_frames];
                if let Some(snap) = processor.process_block(&make_block(block_data, 1, rate))
                    && block_idx >= warmup
                {
                    first_samples.push(snap.samples[0]);
                }
            }

            if first_samples.len() < 5 {
                eprintln!(
                    "[{name}] too few snapshots ({}), skipping",
                    first_samples.len()
                );
                continue;
            }

            let mean = first_samples.iter().sum::<f32>() / first_samples.len() as f32;
            let variance = first_samples
                .iter()
                .map(|v| (v - mean).powi(2))
                .sum::<f32>()
                / first_samples.len() as f32;
            let std_dev = variance.sqrt();
            let max_jump = first_samples
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0_f32, f32::max);

            eprintln!(
                "[{name}] snapshots={}, phase_stddev={std_dev:.6}, max_jump={max_jump:.6}, mean_phase={mean:.4}",
                first_samples.len()
            );

            if noise < 0.05 {
                assert!(
                    std_dev < 0.15,
                    "[{name}] trigger phase too unstable: stddev={std_dev:.4}"
                );
            }
        }
    }

    /// Verifies the oscilloscope locks onto a clean sine quickly and
    /// adapts when the signal changes frequency (including octave jumps).
    #[test]
    fn lock_acquisition_and_frequency_transitions() {
        let rate = 48_000.0_f32;
        let block_frames = 1024_usize;
        let stable_config = OscilloscopeConfig {
            sample_rate: rate,
            segment_duration: 0.02,
            trigger_mode: TriggerMode::Stable { num_cycles: 2 },
        };
        let feed = |processor: &mut OscilloscopeProcessor,
                    signal: &[f32],
                    range: std::ops::Range<usize>| {
            for i in range {
                let offset = i * block_frames;
                processor.process_block(&make_block(
                    &signal[offset..offset + block_frames],
                    1,
                    rate,
                ));
            }
        };

        // lock acquisition on a clean sine
        {
            let mut processor = OscilloscopeProcessor::new(stable_config);
            let num_blocks = 20;
            let signal: Vec<f32> = (0..block_frames * num_blocks)
                .map(|n| (std::f32::consts::TAU * 440.0 * n as f32 / rate).sin())
                .collect();

            let mut first_lock_block = None;
            for block_idx in 0..num_blocks {
                let offset = block_idx * block_frames;
                processor.process_block(&make_block(
                    &signal[offset..offset + block_frames],
                    1,
                    rate,
                ));
                if first_lock_block.is_none() && processor.last_pitch.is_some() {
                    first_lock_block = Some(block_idx);
                }
            }

            let lock_block = first_lock_block.expect("should lock on a clean sine");
            eprintln!("[lock acquisition] locked at block {lock_block}");
            assert!(
                lock_block <= 10,
                "should lock within 10 blocks, took {lock_block}"
            );
        }

        // octave transition 440 -> 880 Hz
        {
            let mut processor = OscilloscopeProcessor::new(stable_config);
            let (warmup_n, after_n) = (20_usize, 20_usize);
            let switch_sample = warmup_n * block_frames;
            let total = (warmup_n + after_n) * block_frames;

            let signal: Vec<f32> = (0..total)
                .map(|n| {
                    let t = n as f32 / rate;
                    if n < switch_sample {
                        (std::f32::consts::TAU * 440.0 * t).sin()
                    } else {
                        let t0 = switch_sample as f32 / rate;
                        let phase0 = std::f32::consts::TAU * 440.0 * t0;
                        (phase0 + std::f32::consts::TAU * 880.0 * (t - t0)).sin()
                    }
                })
                .collect();

            feed(&mut processor, &signal, 0..warmup_n);
            let pre = processor
                .last_pitch
                .expect("should have pitch after warmup");
            assert!(
                (pre - 440.0).abs() < 20.0,
                "pre-transition pitch should be ~440Hz, got {pre:.1}"
            );

            let mut adapted_block = None;
            for block_idx in 0..after_n {
                let offset = (warmup_n + block_idx) * block_frames;
                processor.process_block(&make_block(
                    &signal[offset..offset + block_frames],
                    1,
                    rate,
                ));
                if adapted_block.is_none()
                    && let Some(p) = processor.last_pitch
                    && (p - 880.0).abs() < 50.0
                {
                    adapted_block = Some(block_idx);
                }
            }
            let b = adapted_block.expect("should adapt to 880Hz");
            let final_p = processor.last_pitch.unwrap();
            eprintln!("[octave transition] adapted at block {b}, final pitch: {final_p:.1}Hz");
            assert!(b <= 10, "should adapt within 10 blocks, took {b}");
        }

        // signal onset (silence -> sine)
        {
            let mut processor = OscilloscopeProcessor::new(stable_config);
            let (silence_n, signal_n) = (10_usize, 20_usize);
            let onset_sample = silence_n * block_frames;
            let total = (silence_n + signal_n) * block_frames;

            let signal: Vec<f32> = (0..total)
                .map(|n| {
                    if n < onset_sample {
                        0.0
                    } else {
                        let t = (n - onset_sample) as f32 / rate;
                        (std::f32::consts::TAU * 440.0 * t).sin()
                    }
                })
                .collect();

            feed(&mut processor, &signal, 0..silence_n);
            assert!(
                processor.last_pitch.is_none(),
                "should have no pitch during silence"
            );

            let mut lock_block = None;
            for block_idx in 0..signal_n {
                let offset = (silence_n + block_idx) * block_frames;
                processor.process_block(&make_block(
                    &signal[offset..offset + block_frames],
                    1,
                    rate,
                ));
                if lock_block.is_none() && processor.last_pitch.is_some() {
                    lock_block = Some(block_idx);
                }
            }

            let b = lock_block.expect("should lock after signal onset");
            eprintln!("[signal onset] locked at block {b} after onset");
            assert!(b <= 10, "should lock within 10 blocks of onset, took {b}");
        }
    }
}
