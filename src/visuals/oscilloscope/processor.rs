// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::dsp::AudioBlock;
use crate::util::audio::{self, Channel, DEFAULT_SAMPLE_RATE};
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;

const TRACE_COUNT: usize = 2;

fn parabolic_refine(y_prev: f32, y_curr: f32, y_next: f32, tau: usize) -> f32 {
    let denom = y_prev - 2.0 * y_curr + y_next;
    if denom.abs() < f32::EPSILON { return tau as f32; }
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OscilloscopeConfig {
    pub sample_rate: f32,
    pub segment_duration: f32,
    pub trigger_mode: TriggerMode,
    pub trigger_source: Channel,
    pub channel_1: Channel,
    pub channel_2: Channel,
}

impl Default for OscilloscopeConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            segment_duration: 0.02,
            trigger_mode: TriggerMode::default(),
            trigger_source: Channel::Mid,
            channel_1: Channel::Mid,
            channel_2: Channel::None,
        }
    }
}

enum PeriodTuning {}

impl PeriodTuning {
    const MIN_HZ: f32 = 20.0;
    const MAX_HZ: f32 = 8000.0;
    const PROBE_SECONDS: f32 = 0.1;
    const MIN_SIGNAL_PEAK: f32 = 0.001;
    const MIN_PERIODICITY: f32 = 0.5;
    const PEAK_CUTOFF: f32 = 0.93;
}

#[derive(Clone, Copy)]
struct PeriodEstimate {
    period: f32,
    confidence: f32,
}

struct PeriodFft {
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    input: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    output: Vec<f32>,
    scratch: Vec<Complex<f32>>,
}

impl PeriodFft {
    fn new(size: usize) -> Self {
        let mut planner = RealFftPlanner::new();
        let forward = planner.plan_fft_forward(size);
        let inverse = planner.plan_fft_inverse(size);
        let mut scratch = forward.make_scratch_vec();
        let inv_scratch = inverse.make_scratch_vec();
        if inv_scratch.len() > scratch.len() {
            scratch = inv_scratch;
        }
        Self {
            input: forward.make_input_vec(),
            spectrum: forward.make_output_vec(),
            output: inverse.make_output_vec(),
            scratch,
            forward,
            inverse,
        }
    }
}

#[derive(Default)]
struct PeriodEstimator {
    periodicity: Vec<f32>,
    energy_prefix: Vec<f32>,
    last_peak: f32,
    fft: Option<PeriodFft>,
}

impl PeriodEstimator {
    fn rebuild_fft(&mut self, size: usize) {
        if self.fft.as_ref().is_none_or(|fft| fft.input.len() != size) {
            self.fft = Some(PeriodFft::new(size));
        }
    }

    fn estimate_period(&mut self, samples: &[f32], rate: f32) -> Option<PeriodEstimate> {
        self.last_peak = 0.0;
        if samples.len() < 3 { return None; }

        let mean = samples.iter().sum::<f32>() / samples.len() as f32;
        self.last_peak = samples
            .iter()
            .map(|sample| (sample - mean).abs())
            .fold(0.0, f32::max);
        if self.last_peak < PeriodTuning::MIN_SIGNAL_PEAK { return None; }

        let min_period = (rate / PeriodTuning::MAX_HZ).round().max(2.0) as usize;
        let max_period = ((rate / PeriodTuning::MIN_HZ).round() as usize).min(samples.len() / 2);
        if max_period <= min_period + 1 { return None; }
        self.compute_periodicity(samples, mean, max_period)?;

        let nsdf = &self.periodicity[..=max_period];
        let zero_crossing = (1..=max_period).find(|&tau| nsdf[tau] <= 0.0)?;
        let first_tau = min_period.max(zero_crossing);
        if first_tau >= max_period { return None; }

        let is_candidate = |tau: &usize| {
            nsdf[*tau] >= PeriodTuning::MIN_PERIODICITY
                && nsdf[*tau] >= nsdf[*tau - 1]
                && nsdf[*tau] >= nsdf[*tau + 1]
        };
        let best = (first_tau..max_period)
            .filter(is_candidate)
            .max_by(|&a, &b| nsdf[a].total_cmp(&nsdf[b]))?;
        let cutoff = nsdf[best] * PeriodTuning::PEAK_CUTOFF;
        let peak = (first_tau..=best)
            .find(|tau| is_candidate(tau) && nsdf[*tau] >= cutoff)
            .unwrap_or(best);

        Some(PeriodEstimate {
            period: parabolic_refine(nsdf[peak - 1], nsdf[peak], nsdf[peak + 1], peak),
            confidence: nsdf[peak].clamp(0.0, 1.0),
        })
    }

    fn compute_periodicity(&mut self, samples: &[f32], mean: f32, max_lag: usize) -> Option<()> {
        let fft_size = (samples.len() * 2).next_power_of_two();
        self.rebuild_fft(fft_size);
        let Self { periodicity, energy_prefix, fft, .. } = self;
        let fft = fft.as_mut()?;

        energy_prefix.resize(samples.len() + 1, 0.0);
        energy_prefix[0] = 0.0;
        for (i, (dst, &sample)) in fft.input[..samples.len()]
            .iter_mut()
            .zip(samples)
            .enumerate()
        {
            let centered = sample - mean;
            *dst = centered;
            energy_prefix[i + 1] = centered.mul_add(centered, energy_prefix[i]);
        }
        fft.input[samples.len()..].fill(0.0);

        fft.forward
            .process_with_scratch(&mut fft.input, &mut fft.spectrum, &mut fft.scratch)
            .ok()?;

        for bin in &mut fft.spectrum {
            *bin = Complex::new(bin.norm_sqr(), 0.0);
        }

        fft.inverse
            .process_with_scratch(&mut fft.spectrum, &mut fft.output, &mut fft.scratch)
            .ok()?;

        let norm = 1.0 / fft_size as f32;
        periodicity.resize(max_lag + 1, 0.0);
        let total_energy = energy_prefix[samples.len()];
        if total_energy <= f32::EPSILON { return None; }
        for tau in 0..=max_lag {
            let left_energy = energy_prefix[samples.len() - tau];
            let right_energy = total_energy - energy_prefix[tau];
            let denom = left_energy + right_energy;
            periodicity[tau] = if denom > f32::EPSILON {
                2.0 * fft.output[tau] * norm / denom
            } else {
                0.0
            };
        }
        Some(())
    }
}

enum StableTuning {}

impl StableTuning {
    const WINDOW_SECONDS: f32 = 0.04;
    const MIN_CYCLES: f32 = 2.0;
    const SEARCH_PERIODS: f32 = 1.5;
    const NORMALIZE_FLOOR: f32 = 0.01;
    const MEAN_RESPONSIVENESS: f32 = 0.25;
    const EDGE_STRENGTH: f32 = 1.0;
    const BUFFER_STRENGTH: f32 = 1.0;
    const BUFFER_RESPONSIVENESS: f32 = 0.5;
    const BUFFER_FALLOFF_PERIODS: f32 = 0.5;
    const BUFFER_RETUNE_SEMITONES: f32 = 1.0;
    const SLOPE_WIDTH_PERIODS: f32 = 0.25;
    const RESET_BELOW_MATCH: f32 = 0.3;
    const MAX_MISSED_PERIODS: u8 = 4;
}

fn trigger_kernel_len(period: f32, rate: f32) -> usize {
    (rate * StableTuning::WINDOW_SECONDS)
        .max(period * StableTuning::MIN_CYCLES)
        .round()
        .max(2.0) as usize
}

fn normalize_peak(data: &mut [f32]) {
    let peak = data.iter().map(|sample| sample.abs()).fold(0.0, f32::max);
    let scale = 1.0 / peak.max(StableTuning::NORMALIZE_FLOOR);
    data.iter_mut().for_each(|sample| *sample *= scale);
}

fn gaussian(len: usize, index: usize, std: f32) -> f32 {
    if len <= 1 || std <= f32::EPSILON { return 0.0; }
    let center = (len - 1) as f32 * 0.5;
    let x = index as f32 - center;
    (-0.5 * (x / std).powi(2)).exp()
}

fn normalized_correlation(pairs: impl Iterator<Item = (f32, f32)>) -> f32 {
    let (mut len, mut sum_x, mut sum_y, mut sum_xx, mut sum_yy, mut sum_xy) =
        (0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for (x, y) in pairs {
        len += 1;
        sum_x += x;
        sum_y += y;
        sum_xx += x * x;
        sum_yy += y * y;
        sum_xy += x * y;
    }
    if len == 0 { return 0.0; }

    let n = len as f32;
    let dot = sum_xy - sum_x * sum_y / n;
    let energy_x = (sum_xx - sum_x * sum_x / n).max(0.0);
    let energy_y = (sum_yy - sum_y * sum_y / n).max(0.0);
    let denom = (energy_x * energy_y).sqrt();
    if denom > f32::EPSILON { (dot / denom).clamp(-1.0, 1.0) } else { 0.0 }
}

fn sample_linear_zero(data: &[f32], pos: f32) -> f32 {
    if data.is_empty() || pos < 0.0 || pos > (data.len() - 1) as f32 { return 0.0; }
    let idx = pos as usize;
    let frac = pos - idx as f32;
    if frac > f32::EPSILON && idx + 1 < data.len() {
        crate::util::audio::lerp(data[idx], data[idx + 1], frac)
    } else {
        data[idx]
    }
}

fn retune_reference(reference: &[f32], old_period: f32, new_period: f32, len: usize) -> Vec<f32> {
    let ratio = new_period / old_period;
    if !ratio.is_finite() || ratio <= f32::EPSILON {
        return vec![0.0; len];
    }

    let old_center = reference.len().saturating_sub(1) as f32 * 0.5;
    let new_center = len.saturating_sub(1) as f32 * 0.5;
    (0..len)
        .map(|i| {
            let pos = old_center + (i as f32 - new_center) / ratio;
            sample_linear_zero(reference, pos)
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct Capture {
    span: f32,
    start: usize,
    frac_offset: f32,
}

#[derive(Default)]
struct StableTrigger {
    estimator: PeriodEstimator,
    period: Option<f32>,
    missed_periods: u8,
    reference: Vec<f32>,
    reference_period: Option<f32>,
    kernel: Vec<f32>,
    work: Vec<f32>,
    candidate: Vec<f32>,
    mean: f32,
}

impl StableTrigger {
    fn unlock(&mut self) {
        self.period = None;
        self.missed_periods = 0;
        self.reference.clear();
        self.reference_period = None;
        self.mean = 0.0;
    }

    fn capture(
        &mut self,
        trace: &[f32],
        sample_rate: f32,
        probe_frames: usize,
        fallback_frames: usize,
        cycles: usize,
    ) -> Capture {
        let probe_len = probe_frames.min(trace.len());
        let detected = if probe_len >= 3 {
            self.estimator
                .estimate_period(&trace[trace.len() - probe_len..], sample_rate)
        } else {
            self.estimator.last_peak = 0.0;
            None
        };

        if probe_len > 0 && self.estimator.last_peak < PeriodTuning::MIN_SIGNAL_PEAK {
            self.unlock();
        }

        self.stabilize(detected)
            .and_then(|estimate| self.locate(trace, estimate, cycles, sample_rate))
            .unwrap_or(Capture {
                span: fallback_frames.saturating_sub(1).max(1) as f32,
                start: trace.len().saturating_sub(fallback_frames),
                frac_offset: 0.0,
            })
    }

    fn stabilize(&mut self, detected: Option<PeriodEstimate>) -> Option<PeriodEstimate> {
        let Some(mut estimate) = detected else {
            let period = self.period?;
            self.missed_periods = self.missed_periods.saturating_add(1);
            if self.missed_periods > StableTuning::MAX_MISSED_PERIODS {
                self.unlock();
                return None;
            }
            return Some(PeriodEstimate { period, confidence: 0.0 });
        };
        self.missed_periods = 0;

        if let Some(prev) = self.period
            && (0.9..=1.1).contains(&(estimate.period / prev))
        {
            estimate.period = prev + 0.35 * (estimate.period - prev);
        }

        self.period = Some(estimate.period);
        Some(estimate)
    }

    fn locate(
        &mut self,
        trace: &[f32],
        estimate: PeriodEstimate,
        cycles: usize,
        rate: f32,
    ) -> Option<Capture> {
        let period = estimate.period.max(1.0);
        let span = period * cycles.max(1) as f32;
        let frames = span.ceil() as usize + 1;
        let len = trigger_kernel_len(period, rate);
        let before = len / 2;
        let after = len - before;
        let right = trace.len().checked_sub(frames.max(after))?;
        if right < before { return None; }

        let search = ((period * StableTuning::SEARCH_PERIODS).round() as usize)
            .max(1)
            .min(len / 2)
            .min(right - before);
        let left = right - search;
        self.prepare(&trace[left - before..right + after], len, period);

        let use_reference = self.reference.iter().any(|sample| sample.abs() > 1.0e-3);
        let (mut offset, mut frac_offset) = self.find_best(search, period, use_reference);
        let confident = estimate.confidence >= PeriodTuning::MIN_PERIODICITY;
        let segment = |offset| &trace[left + offset - before..left + offset - before + len];
        let reset = confident
            && use_reference
            && self.write_candidate(segment(offset), period) < StableTuning::RESET_BELOW_MATCH;
        if reset {
            self.reference.fill(0.0);
            (offset, frac_offset) = self.find_best(search, period, false);
        }
        if confident {
            if !use_reference || reset {
                self.write_candidate(segment(offset), period);
            }
            self.update_reference(period);
        }

        let mut start = left + offset;
        if frac_offset < 0.0 && start > 0 {
            start -= 1;
            frac_offset += 1.0;
        }
        Some(Capture {
            span,
            start,
            frac_offset,
        })
    }

    fn prepare(&mut self, data: &[f32], len: usize, period: f32) {
        self.retune_reference(len, period);

        self.kernel.resize(len, 0.0);
        let midpoint = len / 2;
        let max_width = (midpoint.max(1) as f32 / 3.0).max(1.0);
        let width = (StableTuning::SLOPE_WIDTH_PERIODS * period).clamp(1.0, max_width);
        for (i, value) in self.kernel.iter_mut().enumerate() {
            let side = if i < midpoint { -0.5 } else { 0.5 };
            *value = side * StableTuning::EDGE_STRENGTH * 2.0 * gaussian(len, i, width);
        }

        let mean = data.iter().sum::<f32>() / data.len().max(1) as f32;
        self.mean += StableTuning::MEAN_RESPONSIVENESS * (mean - self.mean);
        self.work.clear();
        self.work.extend(data.iter().map(|sample| sample - self.mean));
    }

    fn find_best(&self, search: usize, period: f32, use_reference: bool) -> (usize, f32) {
        let stride = ((period / 16.0).round() as usize).clamp(1, 128).min(search.max(1));
        let mut best = (search / 2, f32::NEG_INFINITY);
        for offset in (0..=search).rev().step_by(stride).chain([0]) {
            let score = self.score_at(offset, use_reference);
            if score > best.1 {
                best = (offset, score);
            }
        }
        let mut step = stride;
        while step > 1 {
            let next = (step / 4).max(1);
            for offset in (best.0.saturating_sub(step)..=(best.0 + step).min(search))
                .rev()
                .step_by(next)
            {
                let score = self.score_at(offset, use_reference);
                if score > best.1 {
                    best = (offset, score);
                }
            }
            step = next;
        }

        let frac_offset = if best.0 > 0 && best.0 < search {
            (parabolic_refine(
                self.score_at(best.0 - 1, use_reference),
                self.score_at(best.0, use_reference),
                self.score_at(best.0 + 1, use_reference),
                best.0,
            ) - best.0 as f32)
                .clamp(-0.5, 0.5)
        } else {
            0.0
        };
        (best.0, frac_offset)
    }

    fn score_at(&self, offset: usize, use_reference: bool) -> f32 {
        let reference_gain = if use_reference { StableTuning::BUFFER_STRENGTH } else { 0.0 };
        normalized_correlation(
            self.work[offset..offset + self.kernel.len()]
                .iter()
                .zip(&self.kernel)
                .zip(&self.reference)
                .map(|((&x, &slope), &reference)| (x, slope + reference * reference_gain)),
        )
    }

    fn retune_reference(&mut self, len: usize, period: f32) {
        let Some(old_period) = self.reference_period else {
            self.reference.resize(len, 0.0);
            self.reference_period = Some(period);
            return;
        };

        let ratio = period / old_period;
        let semitones = ratio.log2() * 12.0;
        if self.reference.len() != len || semitones.abs() >= StableTuning::BUFFER_RETUNE_SEMITONES {
            self.reference = retune_reference(&self.reference, old_period, period, len);
            self.reference_period = Some(period);
        }
    }

    fn update_reference(&mut self, period: f32) {
        normalize_peak(&mut self.reference);
        for (reference, &candidate) in self.reference.iter_mut().zip(&self.candidate) {
            *reference += StableTuning::BUFFER_RESPONSIVENESS * (candidate - *reference);
        }
        self.reference_period = Some(
            self.reference_period
                .map(|prev| prev + StableTuning::BUFFER_RESPONSIVENESS * (period - prev))
                .unwrap_or(period),
        );
    }

    fn write_candidate(&mut self, segment: &[f32], period: f32) -> f32 {
        let mean = segment.iter().sum::<f32>() / segment.len().max(1) as f32;
        self.candidate.clear();
        self.candidate.extend(segment.iter().map(|sample| sample - mean));
        normalize_peak(&mut self.candidate);

        let std = (period * StableTuning::BUFFER_FALLOFF_PERIODS).max(1.0);
        let len = self.candidate.len();
        for (i, sample) in self.candidate.iter_mut().enumerate() {
            *sample *= gaussian(len, i, std);
        }

        normalized_correlation(self.reference.iter().copied().zip(self.candidate.iter().copied()))
    }
}

fn find_rising_zero_crossing(
    samples: &[f32],
    frames: impl Iterator<Item = usize>,
) -> Option<usize> {
    let sample = |f: usize| samples.get(f).copied();
    let mut it = frames;
    let first = it.next()?;
    let mut prev_val = sample(first)?;
    let mut prev_idx = first;
    for f in it {
        let cur = sample(f)?;
        let (lo_val, hi_idx, hi_val) = if f > prev_idx {
            (prev_val, f, cur)
        } else {
            (cur, prev_idx, prev_val)
        };
        if hi_val > 0.0 && lo_val <= 0.0 { return Some(hi_idx); }
        prev_val = cur;
        prev_idx = f;
    }
    None
}

#[derive(Debug, Clone, Default)]
pub struct OscilloscopeSnapshot {
    pub epoch: u64,
    pub channels: usize,
    pub samples: Vec<f32>,
    pub samples_per_channel: usize,
}

#[derive(Default)]
struct TraceState {
    buffer: Vec<f32>,
    trigger: StableTrigger,
}

pub struct OscilloscopeProcessor {
    config: OscilloscopeConfig,
    snapshot: OscilloscopeSnapshot,
    epoch: u64,
    history: VecDeque<f32>,
    history_channels: Option<usize>,
    traces: [TraceState; TRACE_COUNT],
    source: TraceState,
}

impl OscilloscopeProcessor {
    pub fn new(config: OscilloscopeConfig) -> Self {
        Self {
            config,
            snapshot: OscilloscopeSnapshot::default(),
            epoch: 0,
            history: VecDeque::new(),
            history_channels: None,
            traces: std::array::from_fn(|_| TraceState::default()),
            source: TraceState::default(),
        }
    }

    pub fn config(&self) -> OscilloscopeConfig {
        self.config
    }

    #[cfg(test)]
    fn last_cycle_rate(&self) -> Option<f32> {
        self.source
            .trigger
            .period
            .or_else(|| self.traces.iter().find_map(|trace| trace.trigger.period))
            .map(|period| self.config.sample_rate / period)
    }

    pub fn process_block(&mut self, block: &AudioBlock<'_>) -> Option<OscilloscopeSnapshot> {
        if block.is_empty() { return None; }

        if audio::sample_rates_differ(self.config.sample_rate, block.sample_rate) {
            self.update_config(OscilloscopeConfig {
                sample_rate: block.sample_rate,
                ..self.config
            });
        }

        let channel_count = block.channels;
        if self.history_channels.is_some_and(|channels| channels != channel_count)
            || (!self.history.is_empty() && !self.history.len().is_multiple_of(channel_count))
        {
            self.clear_history();
        }
        self.history_channels = Some(channel_count);

        let base_frames = (self.config.sample_rate * self.config.segment_duration)
            .round()
            .max(1.0) as usize;
        let max_period = (self.config.sample_rate / PeriodTuning::MIN_HZ).ceil() as usize;
        let probe_frames = ((self.config.sample_rate * PeriodTuning::PROBE_SECONDS).round() as usize)
            .max(max_period * 2);
        let trigger_frames = match self.config.trigger_mode {
            TriggerMode::ZeroCrossing => base_frames + max_period,
            TriggerMode::Stable { num_cycles } => {
                stable_history_frames(max_period, num_cycles, self.config.sample_rate)
            }
        };
        let trace_channels = [self.config.channel_1, self.config.channel_2];
        let trigger_source = self.config.trigger_source;
        let samples = &block.samples[..block.frame_count() * channel_count];
        audio::extend_interleaved_history(
            &mut self.history,
            samples,
            probe_frames.max(base_frames).max(trigger_frames) * channel_count,
            channel_count,
        );
        let available = self.history.len() / channel_count;
        let data = self.history.make_contiguous();
        let mode = self.config.trigger_mode;
        let sample_rate = self.config.sample_rate;
        let capture = |trace: &[f32], trigger: &mut StableTrigger| match mode {
            TriggerMode::ZeroCrossing => zero_crossing_capture(trace, base_frames, max_period),
            TriggerMode::Stable { num_cycles } => (trace.len() >= base_frames).then(|| {
                trigger.capture(trace, sample_rate, probe_frames, base_frames, num_cycles)
            }),
        };
        let linked_capture = if audio::project_interleaved_channel_into(
            &mut self.source.buffer,
            data,
            channel_count,
            available,
            trigger_source,
        ) {
            capture(&self.source.buffer, &mut self.source.trigger)
        } else {
            None
        };
        let mut captures = [None; TRACE_COUNT];

        for (slot, (trace, channel)) in self.traces.iter_mut().zip(trace_channels).enumerate() {
            if !audio::project_interleaved_channel_into(
                &mut trace.buffer,
                data,
                channel_count,
                available,
                channel,
            ) {
                continue;
            }

            captures[slot] = linked_capture.or_else(|| capture(&trace.buffer, &mut trace.trigger));
        }

        if captures.iter().all(Option::is_none) { return None; }

        self.write_snapshot(&captures);
        Some(self.snapshot.clone())
    }

    fn clear_history(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.history.clear();
        self.traces.iter_mut().for_each(|trace| {
            trace.buffer.clear();
            trace.trigger.unlock();
        });
        self.source.buffer.clear();
        self.source.trigger.unlock();
    }

    fn write_snapshot(&mut self, captures: &[Option<Capture>; TRACE_COUNT]) {
        const TARGET: usize = 4096;

        let target = captures
            .iter()
            .filter_map(|capture| capture.map(|capture| capture.span.round().max(1.0) as usize + 1))
            .max()
            .unwrap_or(2)
            .clamp(2, TARGET);

        self.snapshot.epoch = self.epoch;
        self.snapshot.samples.clear();
        self.snapshot.channels = 0;
        for (slot, capture) in captures.iter().copied().enumerate() {
            let Some(capture) = capture else { continue };
            if downsample_trace(
                &mut self.snapshot.samples,
                &self.traces[slot].buffer,
                capture,
                target,
            ) {
                self.snapshot.channels += 1;
            }
        }
        self.snapshot.samples_per_channel = if self.snapshot.channels == 0 { 0 } else { target };
    }

    pub fn update_config(&mut self, config: OscilloscopeConfig) {
        if self.config != config {
            let epoch = self.epoch.wrapping_add(1);
            *self = Self::new(config);
            self.epoch = epoch;
        }
    }
}

fn stable_history_frames(max_period: usize, cycles: usize, sample_rate: f32) -> usize {
    let max_period_f = max_period as f32;
    let max_kernel = trigger_kernel_len(max_period_f, sample_rate);
    let max_tail = (max_period * cycles.max(1) + 1).max(max_kernel.div_ceil(2));
    let max_search = (max_period_f * StableTuning::SEARCH_PERIODS).ceil() as usize;
    max_kernel / 2 + max_tail + max_search + 2
}

fn zero_crossing_capture(samples: &[f32], frames: usize, search_range: usize) -> Option<Capture> {
    let frames = frames.min(samples.len());
    if frames == 0 { return None; }

    let end = samples.len().saturating_sub(1);
    let right_lo = end.saturating_sub(search_range);
    let right = find_rising_zero_crossing(samples, (right_lo..=end).rev()).unwrap_or(end);

    let left_lo = right.saturating_sub(frames);
    let left_hi = (left_lo + search_range).min(right.saturating_sub(2));
    let left = find_rising_zero_crossing(samples, left_lo..=left_hi).unwrap_or(left_lo);

    Some(Capture {
        span: right.saturating_sub(left).max(1) as f32,
        start: left,
        frac_offset: 0.0,
    })
}

fn downsample_trace(output: &mut Vec<f32>, data: &[f32], capture: Capture, target: usize) -> bool {
    if target < 2 { return false; }

    let start = capture.start.min(data.len());
    let data = &data[start..];
    if data.len() < 2 { return false; }

    let last = (data.len() - 1) as f32;
    let start_offset = capture.frac_offset.clamp(0.0, last);
    let span = capture.span.min(last - start_offset);
    if !span.is_finite() || span <= 0.0 { return false; }

    let step = span / (target - 1) as f32;
    output.extend((0..target).map(|i| sample_linear_zero(data, start_offset + i as f32 * step)));
    true
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
            ..Default::default()
        }
    }

    fn periodic_samples(freq: f32, rate: f32, frames: usize, f: impl Fn(f32) -> f32) -> Vec<f32> {
        (0..frames).map(|n| f(freq * n as f32 / rate)).collect()
    }

    fn sine_samples(freq: f32, rate: f32, frames: usize) -> Vec<f32> {
        periodic_samples(freq, rate, frames, |cycles| (TAU * cycles).sin())
    }

    fn noise_samples(frames: usize) -> Vec<f32> {
        (0..frames)
            .scan(1_u32, |seed, _| {
                *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                Some((*seed as f32 / u32::MAX as f32) * 2.0 - 1.0)
            })
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
            if predicate(processor) { return Some(block - start); }
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

    fn cycle_rate_switch_signal(from: f32, to: f32, warmup: usize, after: usize) -> Vec<f32> {
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

    fn two_channel_correlation(snap: &OscilloscopeSnapshot) -> f32 {
        assert_eq!(snap.channels, 2);
        let n = snap.samples_per_channel;
        assert_eq!(snap.samples.len(), n * 2);
        let (a, b) = snap.samples.split_at(n);
        let dot: f32 = a.iter().zip(b).map(|(a, b)| a * b).sum();
        let energy = |samples: &[f32]| samples.iter().map(|sample| sample * sample).sum::<f32>();
        dot / (energy(a) * energy(b)).sqrt()
    }

    fn inverted_stereo_capture(config: OscilloscopeConfig) -> (f32, f32) {
        let mut processor = OscilloscopeProcessor::new(config);
        let mono = sine_samples(440.0, RATE, BLOCK * 20);
        let stereo: Vec<f32> = mono.iter().flat_map(|&s| [s, -s]).collect();
        let mut snap = None;
        for block in 0..20 {
            let offset = block * BLOCK * 2;
            snap = processor.process_block(&make_block(
                &stereo[offset..offset + BLOCK * 2],
                2,
                RATE,
            ));
        }
        (
            processor.last_cycle_rate().expect("trigger should lock"),
            two_channel_correlation(&snap.expect("snapshot")),
        )
    }

    fn phase_delta(samples: f32, period: f32) -> f32 {
        (samples + period * 0.5).rem_euclid(period) - period * 0.5
    }

    fn stable_phase_jitter(signal: &[f32], freq: f32, measured: Range<usize>) -> f32 {
        let mut trigger = StableTrigger::default();
        let base_frames = (RATE * stable_config().segment_duration).round() as usize;
        let max_period = (RATE / PeriodTuning::MIN_HZ).ceil() as usize;
        let probe_frames = ((RATE * PeriodTuning::PROBE_SECONDS).round() as usize).max(max_period * 2);
        let history_frames = stable_history_frames(max_period, 2, RATE);
        let period = RATE / freq;
        let (mut first, mut jitter) = (None, 0.0_f32);

        for block in 1..measured.end {
            let end = block * BLOCK;
            let start = end.saturating_sub(history_frames);
            let capture = trigger.capture(&signal[start..end], RATE, probe_frames, base_frames, 2);
            if block >= measured.start && trigger.period.is_some() {
                let pos = start as f32 + capture.start as f32 + capture.frac_offset;
                let first = *first.get_or_insert(pos);
                jitter = jitter.max(phase_delta(pos - first, period).abs());
            }
        }

        assert!(first.is_some());
        jitter
    }

    #[test]
    fn period_estimation() {
        let mut estimator = PeriodEstimator::default();
        let long = (RATE * 0.1) as usize;
        for (freq, frames, max_error) in [
            (41.0, long, 0.02),
            (110.0, long, 0.02),
            (440.0, long, 0.02),
            (1000.0, long, 0.02),
            (4000.0, long, 0.02),
            (8000.0, long, 0.02),
            (1000.0, 256, 0.03),
        ] {
            let estimate = estimator
                .estimate_period(&sine_samples(freq, RATE, frames), RATE)
                .expect("period");
            let detected = RATE / estimate.period;
            let error = (detected - freq).abs() / freq;
            assert!(error < max_error, "got {detected}Hz, expected {freq}Hz");
            assert!(estimate.confidence > 0.9, "confidence was {}", estimate.confidence);
        }

        for (freq, samples) in [
            (110.0, periodic_samples(110.0, RATE, long, |c| 2.0 * c.fract() - 1.0)),
            (440.0, periodic_samples(440.0, RATE, long, |c| {
                if c.fract() < 0.5 { 1.0 } else { -1.0 }
            })),
            (440.0, periodic_samples(440.0, RATE, long, |c| {
                (TAU * c).sin() + 2.0 * (TAU * 2.0 * c).sin()
            })),
        ] {
            let estimate = estimator.estimate_period(&samples, RATE).expect("period");
            let detected = RATE / estimate.period;
            assert!((detected - freq).abs() / freq < 0.03);
            assert!(estimate.confidence >= PeriodTuning::MIN_PERIODICITY);
        }

        assert!(estimator.estimate_period(&noise_samples(long), RATE).is_none());
    }

    #[test]
    fn parabolic_interpolation() {
        let y = |x: f32| (x - 5.3_f32).powi(2);
        assert!((parabolic_refine(y(4.0), y(5.0), y(6.0), 5) - 5.3).abs() < 0.001);
    }

    #[test]
    fn stable_trigger_limits_phase_jitter() {
        let frames = BLOCK * 60;
        for (name, signal) in [
            ("sine", sine_samples(440.0, RATE, frames)),
            (
                "biased_am",
                periodic_samples(440.0, RATE, frames, |c| {
                    (0.6 + 0.4 * (TAU * c / 37.0).sin()) * (TAU * c).sin() + 0.25
                }),
            ),
            ("saw", periodic_samples(440.0, RATE, frames, |c| 2.0 * c.fract() - 1.0)),
            (
                "square",
                periodic_samples(440.0, RATE, frames, |c| {
                    if c.fract() < 0.5 { 1.0 } else { -1.0 }
                }),
            ),
        ] {
            let jitter = stable_phase_jitter(&signal, 440.0, 20..60);
            assert!(jitter < 3.0, "{name} jitter was {jitter:.3} samples");
        }
    }

    #[test]
    fn stable_trigger_retunes_reference_around_center() {
        let peak = |data: &[f32]| {
            data.iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.total_cmp(b))
                .map(|(index, _)| index)
                .unwrap()
        };
        let mut trigger = StableTrigger {
            reference: vec![0.0; 17],
            reference_period: Some(4.0),
            ..Default::default()
        };
        trigger.reference[8] = 0.25;
        trigger.reference[10] = 1.0;

        trigger.retune_reference(17, 8.0);
        assert_eq!(peak(&trigger.reference), 12);
        assert!((trigger.reference[8] - 0.25).abs() < f32::EPSILON);
        assert_eq!(trigger.reference_period, Some(8.0));
    }

    #[test]
    fn stable_correlation_is_shape_based() {
        for work in [
            [1.0, -1.0, 1.0, -1.0, 10.0, -10.0, 0.0, 0.0],
            [11.0, 9.0, 11.0, 9.0, 1.0, -1.0, 0.0, 0.0],
        ] {
            let trigger = StableTrigger {
                kernel: vec![0.0; 4],
                reference: vec![1.0, -1.0, 1.0, -1.0],
                work: Vec::from(work),
                ..Default::default()
            };
            assert_eq!(trigger.find_best(4, 16.0, true).0, 0);
        }

        let mut trigger = StableTrigger {
            reference: vec![11.0, 9.0, 11.0, 9.0],
            ..Default::default()
        };
        assert!(trigger.write_candidate(&[1.0, -1.0, 1.0, -1.0], 1000.0) > 0.99);
    }

    #[test]
    fn zero_crossing_finds_edges_after_channel_projection() {
        let mono = sine_samples(440.0, RATE, 4800);
        for c in [
            find_rising_zero_crossing(&mono, (0..=3840).rev()).unwrap(),
            find_rising_zero_crossing(&mono, 0..=4799).unwrap(),
        ] {
            assert!(mono[c] > 0.0 && mono[c - 1] <= 0.0);
        }

        let mut projected = Vec::new();
        let same_stereo: Vec<f32> = mono.iter().flat_map(|&s| [s, s]).collect();
        assert!(audio::project_interleaved_channel_into(
            &mut projected,
            &same_stereo,
            2,
            mono.len(),
            Channel::Mid,
        ));
        let c = find_rising_zero_crossing(&projected, (0..=3840).rev()).unwrap();
        assert!(projected[c] > 0.0 && projected[c - 1] <= 0.0);

        let inverted: Vec<f32> = mono.iter().flat_map(|&s| [s, -s]).collect();
        for (channel, should_cross) in [(Channel::Mid, false), (Channel::Left, true)] {
            assert!(audio::project_interleaved_channel_into(
                &mut projected,
                &inverted,
                2,
                mono.len(),
                channel,
            ));
            assert_eq!(
                find_rising_zero_crossing(&projected, 0..=4799).is_some(),
                should_cross
            );
        }
    }

    #[test]
    fn zero_crossing_both_edges_near_zero() {
        let config = OscilloscopeConfig {
            segment_duration: 0.01,
            trigger_mode: TriggerMode::ZeroCrossing,
            channel_1: Channel::Left,
            channel_2: Channel::Right,
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
    fn input_channel_count_change_resets_history_and_trigger_lock() {
        let mut processor = OscilloscopeProcessor::new(stable_config());
        let signal = sine_samples(440.0, RATE, BLOCK * 20);
        feed_blocks(&mut processor, &signal, 0..20);
        assert!(processor.last_cycle_rate().is_some());

        let silence = vec![0.0; BLOCK * 2];
        processor.process_block(&make_block(&silence, 2, RATE));

        assert_eq!(processor.history.len(), silence.len());
        assert!(processor.last_cycle_rate().is_none());
    }

    #[test]
    fn stable_lock_has_bounded_aperiodic_holdover() {
        let (warmup, noise) = (20, 20);
        let mut signal = sine_samples(440.0, RATE, BLOCK * warmup);
        signal.extend(noise_samples(BLOCK * noise));

        let mut processor = OscilloscopeProcessor::new(stable_config());
        feed_blocks(&mut processor, &signal, 0..warmup);
        assert!(processor.last_cycle_rate().is_some());

        let noise_start = warmup * BLOCK;
        processor.process_block(&make_block(&signal[noise_start..noise_start + BLOCK], 1, RATE));
        assert!(processor.last_cycle_rate().is_some(), "brief aperiodic input should hold lock");

        let released = first_block_where(&mut processor, &signal, warmup + 1..warmup + noise, |p| {
            p.last_cycle_rate().is_none()
        })
        .expect("sustained aperiodic input should release lock");
        assert!(released <= 8, "release took {released} blocks");
    }

    #[test]
    fn fixed_trigger_source_preserves_visible_channel_phase() {
        let config = OscilloscopeConfig {
            trigger_source: Channel::Left,
            channel_1: Channel::Left,
            channel_2: Channel::Right,
            ..stable_config()
        };
        let (detected, corr) = inverted_stereo_capture(config);
        assert!((detected - 440.0).abs() < 20.0, "detected {detected}");
        assert!(
            corr < -0.9,
            "linked trigger should preserve inverted stereo phase, got {corr}"
        );
    }

    #[test]
    fn lock_acquisition_and_cycle_rate_transitions() {
        let mut processor = OscilloscopeProcessor::new(stable_config());
        let signal = sine_samples(440.0, RATE, BLOCK * 20);
        assert_lock_within(
            &mut processor,
            &signal,
            0..20,
            10,
            "lock on clean sine",
            |p| p.last_cycle_rate().is_some(),
        );

        let mut processor = OscilloscopeProcessor::new(stable_config());
        let (warmup, after) = (20, 20);
        let signal = cycle_rate_switch_signal(440.0, 880.0, warmup, after);
        feed_blocks(&mut processor, &signal, 0..warmup);
        let pre = processor.last_cycle_rate().expect("cycle rate after warmup");
        assert!(
            (pre - 440.0).abs() < 20.0,
            "pre-transition cycle rate was {pre:.1}"
        );
        assert_lock_within(
            &mut processor,
            &signal,
            warmup..warmup + after,
            10,
            "adapt to 880Hz",
            |p| {
                p.last_cycle_rate()
                    .is_some_and(|cycle_rate| (cycle_rate - 880.0).abs() < 50.0)
            },
        );

        let mut processor = OscilloscopeProcessor::new(stable_config());
        let (silence, signal_blocks) = (10, 20);
        let signal = delayed_sine(440.0, silence, signal_blocks);
        feed_blocks(&mut processor, &signal, 0..silence);
        assert!(
            processor.last_cycle_rate().is_none(),
            "should have no cycle rate during silence"
        );
        assert_lock_within(
            &mut processor,
            &signal,
            silence..silence + signal_blocks,
            10,
            "lock after signal onset",
            |p| p.last_cycle_rate().is_some(),
        );
    }
}
