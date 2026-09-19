// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::dsp::{AudioBlock, ChannelPosition, RunningMeans};
use crate::util::audio::{
    DEFAULT_SAMPLE_RATE, flush_denormal_f64, power_to_db, sanitize_sample_rate,
};
use std::{f64::consts::PI, sync::LazyLock};

const LOUDNESS_OFFSET: f64 = -0.691;
const DEFAULT_FLOOR_DB: f32 = -99.9;

const DEFAULT_WINDOWS: [f32; 4] = [3.0, 0.4, 0.3, 1.0];

const WIN_SHORT_TERM: usize = 0;
const WIN_MOMENTARY: usize = 1;
const WIN_RMS_FAST: usize = 2;
const WIN_RMS_SLOW: usize = 3;

type KWeighting = ([f64; 5], [f64; 5]);

fn k_weighting_coefficients(fs: f64) -> KWeighting {
    let (f0, g, q) = (
        1_681.974_450_955_533,
        3.999_843_853_973_347,
        0.707_175_236_955_419_6,
    );
    let k = (PI * f0 / fs).tan();
    let vh = 10.0_f64.powf(g / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let pb = [
        (vh + vb * k / q + k * k) / a0,
        2.0 * (k * k - vh) / a0,
        (vh - vb * k / q + k * k) / a0,
    ];
    let pa = [1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0];

    let (f0, q) = (38.135_470_876_024_44, 0.500_327_037_323_877_3);
    let k = (PI * f0 / fs).tan();
    let a0 = 1.0 + k / q + k * k;
    let rb = [1.0, -2.0, 1.0];
    let ra = [1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0];

    let conv = |p: [f64; 3], r: [f64; 3]| {
        [
            p[0] * r[0],
            p[0] * r[1] + p[1] * r[0],
            p[0] * r[2] + p[1] * r[1] + p[2] * r[0],
            p[1] * r[2] + p[2] * r[1],
            p[2] * r[2],
        ]
    };
    (conv(pb, rb), conv(pa, ra))
}

fn mean_square_to_lufs(mean_square: f64, floor: f32) -> f32 {
    if mean_square > 0.0 {
        mean_square
            .log10()
            .mul_add(10.0, LOUDNESS_OFFSET)
            .max(f64::from(floor)) as f32
    } else {
        floor
    }
}

fn window_length(sample_rate: f32, window_secs: f32) -> usize {
    (sample_rate * window_secs).round().max(1.0) as usize
}

const TRUE_PEAK_TAPS: usize = 48;
const TRUE_PEAK_PHASES: usize = 8;
const TRUE_PEAK_BATCH_FRAMES: usize = 256;
const TRUE_PEAK_BUFFER_LEN: usize = TRUE_PEAK_BATCH_FRAMES * TRUE_PEAK_PHASES + 2;
type TruePeakFir = [[f32; TRUE_PEAK_PHASES]; TRUE_PEAK_TAPS];
const _: () = assert!(TRUE_PEAK_TAPS.is_multiple_of(2));

fn bessel_i0(x: f64) -> f64 {
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..40 {
        term *= x * x / (4.0 * f64::from(k * k));
        sum += term;
    }
    sum
}

// 24-source-sample delay centers the sinc
// favor accuracy via 8x reconstruction
static TRUE_PEAK_FIR: LazyLock<TruePeakFir> = LazyLock::new(|| {
    let beta = 8.6;
    let window_scale = 1.0 / bessel_i0(beta);
    let radius = TRUE_PEAK_TAPS as f64 / 2.0;
    let mut coefficients: TruePeakFir = std::array::from_fn(|tap| {
        std::array::from_fn(|phase| {
            if phase == 0 {
                return if tap == TRUE_PEAK_TAPS / 2 { 1.0 } else { 0.0 };
            }
            let x = tap as f64 - radius + phase as f64 / TRUE_PEAK_PHASES as f64;
            let sinc = (PI * x).sin() / (PI * x);
            let z = x / radius;
            let window = bessel_i0(beta * (1.0 - z * z).max(0.0).sqrt()) * window_scale;
            (sinc * window) as f32
        })
    });
    for phase in 0..TRUE_PEAK_PHASES {
        let sum: f32 = coefficients.iter().map(|row| row[phase]).sum();
        for row in &mut coefficients {
            row[phase] /= sum;
        }
    }
    coefficients
});

struct TruePeakMeter {
    delay: [f32; TRUE_PEAK_TAPS * 2],
    write: usize,
    points: Vec<f32>,
    peak: f32,
}

impl TruePeakMeter {
    fn new() -> Self {
        let mut points = Vec::with_capacity(TRUE_PEAK_BUFFER_LEN);
        points.extend_from_slice(&[0.0; 2]);
        Self {
            delay: [0.0; TRUE_PEAK_TAPS * 2],
            write: 0,
            points,
            peak: 0.0,
        }
    }

    #[inline(always)]
    fn process(&mut self, sample: f32, coefficients: &TruePeakFir) {
        self.write = if self.write == 0 { TRUE_PEAK_TAPS - 1 } else { self.write - 1 };
        let pos = self.write;
        self.delay[pos] = sample;
        self.delay[pos + TRUE_PEAK_TAPS] = sample;
        let input = &self.delay[pos..pos + TRUE_PEAK_TAPS];
        // Even/odd tap sums shorten dependency chains while phases stay SIMD-friendly.
        let mut sums = [[0.0; TRUE_PEAK_PHASES]; 2];
        for (samples, rows) in input.chunks_exact(2).zip(coefficients.chunks_exact(2)) {
            for lane in 0..2 {
                for phase in 0..TRUE_PEAK_PHASES {
                    // Avoid software FMA calls on CPUs without hardware support.
                    sums[lane][phase] += samples[lane] * rows[lane][phase];
                }
            }
        }
        let output: [f32; TRUE_PEAK_PHASES] = std::array::from_fn(|phase| {
            sums.iter().map(|sum| sum[phase]).sum()
        });
        self.points.extend_from_slice(&output);
        self.peak = if sample.abs() > self.peak { sample.abs() } else { self.peak };
        if self.points.len() == TRUE_PEAK_BUFFER_LEN {
            self.refine();
        }
    }

    fn refine(&mut self) {
        let n = self.points.len() - 2;
        // Independent maxima and unconditional assignments keep reductions vectorizable.
        let mut peaks = [self.peak; TRUE_PEAK_PHASES];
        for points in self.points.windows(TRUE_PEAK_PHASES + 2).step_by(TRUE_PEAK_PHASES) {
            for (i, peak) in peaks.iter_mut().enumerate() {
                let [a, b, c] = [points[i], points[i + 1], points[i + 2]];
                let curve = (b - a) + (b - c);
                let delta = c - a;
                // A flat triple has no vertex; never extrapolate beyond half a phase.
                let denominator = if curve == 0.0 { 1.0 } else { curve };
                #[expect(clippy::manual_clamp, reason = "min/max also bound NaN ratios after overflow")]
                let offset = (delta * 0.5 / denominator).max(-0.5).min(0.5);
                let value = b + offset * 0.5 * (delta - offset * curve);
                let candidate = if value.abs() > c.abs() { value.abs() } else { c.abs() };
                *peak = if candidate > *peak { candidate } else { *peak };
            }
        }
        self.peak = peaks.into_iter().fold(self.peak, |peak, value| {
            if value > peak { value } else { peak }
        });
        // The untouched final two points bridge batches and audio blocks.
        self.points.copy_within(n.., 0);
        self.points.truncate(2);
    }
}

fn k_weighted(sample: f32, state: &mut [f64; 4], coefficients: &KWeighting) -> f64 {
    let (b, a) = coefficients;
    let x = f64::from(sample);
    let y = b[0] * x + state[0];
    state[0] = b[1] * x + state[1] - a[1] * y;
    state[1] = b[2] * x + state[2] - a[2] * y;
    state[2] = b[3] * x + state[3] - a[3] * y;
    state[3] = b[4] * x - a[4] * y;
    y
}

type EnergyWindows = RunningMeans<1, 4>;
type ChannelState = (EnergyWindows, [f64; 4], TruePeakMeter);

#[cold]
fn new_channel(sample_rate: f32) -> ChannelState {
    let capacities = DEFAULT_WINDOWS.map(|window| window_length(sample_rate, window));
    (
        EnergyWindows::seeded(capacities, capacities[WIN_SHORT_TERM]),
        [0.0; 4],
        TruePeakMeter::new(),
    )
}

pub(super) const MAX_CHANNELS: usize = crate::dsp::MAX_AUDIO_CHANNELS;

fn channel_weight(position: ChannelPosition) -> f64 {
    match position {
        ChannelPosition::LowFrequency => 0.0,
        // BS.1770-5 Annex 3: sides (60..=120 degrees), not 7.1 rears.
        ChannelPosition::SideLeft | ChannelPosition::SideRight => 1.41,
        _ => 1.0,
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LoudnessSnapshot {
    pub short_term_loudness: f32,
    pub momentary_loudness: f32,
    pub rms_fast_db: [f32; MAX_CHANNELS],
    pub rms_slow_db: [f32; MAX_CHANNELS],
    pub true_peak_db: [f32; MAX_CHANNELS],
    pub channel_count: usize,
    pub positions: [ChannelPosition; MAX_CHANNELS],
}

impl LoudnessSnapshot {
    pub(in crate::visuals) fn with_floor(floor_db: f32, channel_count: usize) -> Self {
        Self {
            short_term_loudness: floor_db,
            momentary_loudness: floor_db,
            rms_fast_db: [floor_db; MAX_CHANNELS],
            rms_slow_db: [floor_db; MAX_CHANNELS],
            true_peak_db: [floor_db; MAX_CHANNELS],
            channel_count,
            ..Self::default()
        }
    }
}

crate::macros::default_struct! {
    #[derive(Debug, Clone, Copy)]
    pub struct LoudnessConfig {
        pub sample_rate: f32 = DEFAULT_SAMPLE_RATE,
    }
}

pub struct LoudnessProcessor {
    config: LoudnessConfig,
    channels: Vec<Option<ChannelState>>,
    weighting: KWeighting,
}

impl LoudnessProcessor {
    pub fn new(config: LoudnessConfig) -> Self {
        let sample_rate = f64::from(sanitize_sample_rate(config.sample_rate));
        Self {
            weighting: k_weighting_coefficients(sample_rate),
            channels: Vec::new(),
            config,
        }
    }

    pub fn reset_audio(&mut self) {
        self.channels.fill_with(|| None);
    }

    fn ensure_state(&mut self, channels: usize, sample_rate: f32) {
        let rate_changed = self.config.sample_rate != sample_rate;

        if rate_changed {
            self.config.sample_rate = sample_rate;
            self.weighting = k_weighting_coefficients(f64::from(sample_rate));
        }

        if rate_changed || self.channels.len() != channels {
            self.channels = (0..channels).map(|_| None).collect();
        }
    }

    pub fn process_block(&mut self, block: &AudioBlock<'_>) -> LoudnessSnapshot {
        self.ensure_state(block.channels, block.sample_rate);

        let weighting = &self.weighting;
        let firs = &*TRUE_PEAK_FIR;
        let active_channels = block.stereo_channels.max(
            self.channels.iter().rposition(Option::is_some).map_or(0, |i| i + 1),
        );
        for frame in block.samples.chunks_exact(block.channels) {
            for (channel, &sample) in self.channels[..active_channels].iter_mut().zip(frame) {
                if channel.is_none() {
                    if sample == 0.0 { continue; }
                    *channel = Some(new_channel(self.config.sample_rate));
                }
                let (windows, filter, true_peak) = channel.as_mut().unwrap();
                let filtered = k_weighted(sample, filter, weighting);
                let power = filtered * filtered;
                windows.push_nonnegative_finite_wide([if power.is_finite() { power } else { 0.0 }]);
                true_peak.process(sample, firs);
            }
        }
        for (_, state, true_peak) in self.channels.iter_mut().flatten() {
            true_peak.refine();
            state.iter_mut().for_each(flush_denormal_f64);
        }

        let floor = DEFAULT_FLOOR_DB;
        let mut snapshot = LoudnessSnapshot::with_floor(floor, self.channels.len());
        let mut weighted_short_term = 0.0;
        let mut weighted_momentary = 0.0;

        for (channel_index, channel_state) in self.channels.iter_mut().enumerate() {
            let Some((windows, _, true_peak)) = channel_state else { continue };
            let weight = channel_weight(block.positions[channel_index]);
            let [short_term] = windows.mean(WIN_SHORT_TERM);
            let [momentary] = windows.mean(WIN_MOMENTARY);
            let [rms_fast] = windows.mean(WIN_RMS_FAST);
            let [rms_slow] = windows.mean(WIN_RMS_SLOW);
            weighted_short_term += short_term * weight;
            weighted_momentary += momentary * weight;
            snapshot.rms_fast_db[channel_index] = power_to_db(rms_fast as f32, floor);
            snapshot.rms_slow_db[channel_index] = power_to_db(rms_slow as f32, floor);
            let peak = std::mem::take(&mut true_peak.peak);
            snapshot.true_peak_db[channel_index] = power_to_db(peak * peak, floor);
        }

        snapshot.short_term_loudness = mean_square_to_lufs(weighted_short_term, floor);
        snapshot.momentary_loudness = mean_square_to_lufs(weighted_momentary, floor);
        snapshot.positions = block.positions;

        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ebur128::{Channel as ReferenceChannel, EbuR128, Mode};
    use rustfft::{FftPlanner, num_complex::Complex64};
    use std::collections::{BTreeSet, VecDeque};

    const SEVEN_ONE_REFERENCE: [ReferenceChannel; 8] = {
        use ReferenceChannel::*;
        [Left, Right, Center, Unused, Mp135, Mm135, Mp090, Mm090]
    };

    fn sine_wave(rate: f32, secs: f32, freq: f32, amp: f32) -> Vec<f32> {
        crate::util::audio::sine_wave(freq, rate, (rate * secs) as usize, amp)
    }

    fn assert_loudness_matches_ebur128(
        block: AudioBlock<'_>,
        channel_map: Option<&[ReferenceChannel]>,
        transitions: &[f32],
    ) {
        let config = LoudnessConfig { sample_rate: block.sample_rate };
        let mut processor = LoudnessProcessor::new(config);
        let mut reference = EbuR128::new(block.channels as u32, block.sample_rate as u32, Mode::S).unwrap();
        if let Some(channel_map) = channel_map {
            reference.set_channel_map(channel_map).unwrap();
        }
        // Reference queries rescan entire windows; concentrate them at transitions and window expiry.
        let checkpoints: BTreeSet<_> = transitions.iter()
            .flat_map(|&start| [0.0, 0.1, 0.4, 1.0, 3.0].map(|delay| ((start + delay) * block.sample_rate).round() as usize))
            .flat_map(|frame| [frame.saturating_sub(1), frame, frame + 1])
            .chain([1, 18, 273, block.frame_count()])
            .filter(|&frame| frame > 0 && frame <= block.frame_count())
            .collect();
        let mut chunks = [1, 17, 255, 1_024, 4_093].into_iter().cycle();
        let mut frame = 0;
        let mut snapshot = LoudnessSnapshot::default();
        for checkpoint in checkpoints {
            while frame < checkpoint {
                let end = (frame + chunks.next().unwrap()).min(checkpoint);
                let chunk = &block.samples[frame * block.channels..end * block.channels];
                snapshot = processor.process_block(&AudioBlock::with_positions(
                    chunk, block.channels, block.sample_rate, block.positions,
                ));
                reference.add_frames_f32(chunk).unwrap();
                frame = end;
            }
            for (metric, actual, expected) in [
                ("momentary", snapshot.momentary_loudness, reference.loudness_momentary().unwrap()),
                ("short-term", snapshot.short_term_loudness, reference.loudness_shortterm().unwrap()),
            ] {
                let expected = expected.max(f64::from(DEFAULT_FLOOR_DB));
                assert!(
                    (f64::from(actual) - expected).abs() < 1.0e-4,
                    "{} Hz, {:?}, frame={frame}, {metric}: {actual:.9} vs {expected:.9} LUFS",
                    block.sample_rate, &block.positions[..block.channels],
                );
            }
        }
        let whole = LoudnessProcessor::new(config).process_block(&block);
        assert_eq!(snapshot.momentary_loudness, whole.momentary_loudness);
        assert_eq!(snapshot.short_term_loudness, whole.short_term_loudness);
        assert_eq!(snapshot.rms_fast_db, whole.rms_fast_db);
        assert_eq!(snapshot.rms_slow_db, whole.rms_slow_db);
    }

    #[test]
    fn rolling_mean_square_tracks_average() {
        assert_energy_windows_match_direct_sums(
            [4, 2, 1, 4], 0, [1.0, 9.0, 16.0, 25.0, 36.0, 1.0e100, 1.0, 1.0, 1.0, 1.0], 0.0,
        );
        assert_energy_windows_match_direct_sums([2; 4], 0, [2.0_f64.powi(53), 1.0, 1.0], 0.0);

        let prefix = [1.0e100, 2.0, 1.0e-100, 1.0e-100];
        assert_energy_windows_match_direct_sums([2, 129, 2, 129], 127, prefix, 0.0);
        assert_energy_windows_match_direct_sums(
            [2, 129, 2, 129], 127,
            prefix.into_iter().chain(std::iter::repeat_n(1.0e-100, 1_022)), 1.0e-13,
        );
    }

    fn assert_energy_windows_match_direct_sums(
        capacities: [usize; 4],
        leading: usize,
        powers: impl IntoIterator<Item = f64>,
        relative_tolerance: f64,
    ) {
        let mut windows = EnergyWindows::seeded(capacities, leading);
        let capacities = capacities.map(|capacity| capacity.max(1));
        let len = capacities.into_iter().max().unwrap();
        let mut history = VecDeque::from(vec![0.0; leading.min(len)]);
        for window in 0..4 {
            assert_eq!(windows.mean(window)[0], 0.0);
        }
        for (i, power) in powers.into_iter().enumerate() {
            windows.push_nonnegative_finite_wide([power]);
            history.push_back(power);
            if history.len() > len {
                history.pop_front();
            }
            for (window, &capacity) in capacities.iter().enumerate() {
                let count = capacity.min(history.len());
                let expected = history.iter().rev().take(count).sum::<f64>() / count as f64;
                let actual = windows.mean(window)[0];
                assert!(
                    (actual - expected).abs() <= relative_tolerance * expected,
                    "{capacities:?}, leading={leading}, sample={i}, window={window}: {actual:e} != {expected:e}"
                );
            }
        }
    }

    #[test]
    fn energy_windows_match_direct_sums_across_ring_boundaries() {
        for len in 0_usize..=256 {
            let capacities = [len, len / 2, 1, len * 3 / 4];
            for leading in [0, 1, len / 2, len.saturating_sub(1), len, len + 7] {
                // Dyadic inputs and their sums are exact, so no tolerance is needed.
                let powers = (0..len * 3 + 33)
                    .map(|i| {
                        if i % (len + 1) < len / 2 {
                            0.0
                        } else {
                            ((i * 991 + len * 17 + leading * 13) % 65536) as f64 / 65536.0
                        }
                    })
                    .chain(std::iter::repeat_n(0.0, len + 1));
                assert_energy_windows_match_direct_sums(capacities, leading, powers, 0.0);
            }
        }
    }

    #[test]
    fn energy_windows_preserve_small_values_through_wide_fallback_and_wraps() {
        for len in [
            1, 2, 3, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, 257, 513,
        ] {
            let capacities = [len, (len / 2).max(1), (len / 3).max(1), 1];
            for leading in [0, 15, len - 1, len, len + 1] {
                let powers = (0..len * 3 + 33)
                    .map(|i| {
                        if i % (len + 31) == 0 {
                            f64::from(f32::MAX)
                        } else {
                            1.0
                        }
                    })
                    .chain([1.0e100])
                    .chain(std::iter::repeat_n(1.0e-100, len * 3 + 33))
                    .chain(std::iter::repeat_n(0.0, len + 1));
                assert_energy_windows_match_direct_sums(capacities, leading, powers, 1.0e-13);
            }
        }
    }

    #[test]
    fn energy_windows_match_reference_at_audio_sample_rates() {
        for sample_rate in [
            8_000.0, 11_025.0, 22_050.0, 44_100.0, 48_000.0, 96_000.0, 192_000.0, 768_000.0,
        ] {
            let capacities = DEFAULT_WINDOWS.map(|secs| window_length(sample_rate, secs));
            let len = capacities[WIN_SHORT_TERM];
            let mut windows = EnergyWindows::seeded(capacities, len);
            let mut prefix = Vec::with_capacity(len * 2 + 34);
            prefix.push(0.0);
            for i in 0..len * 2 + 33 {
                // Bounded dyadic powers also make prefix subtraction exact.
                let power = if i % 179 < 20 {
                    0.0
                } else {
                    ((i * 991) % 1024) as f64 / 1024.0
                };
                windows.push_nonnegative_finite_wide([power]);
                prefix.push(prefix[i] + power);
                let frame = i + 1;
                if frame.is_multiple_of(997) || frame % 16 <= 1 || frame > len * 2 {
                    for (window, &capacity) in capacities.iter().enumerate() {
                        let expected = (prefix[frame] - prefix[frame.saturating_sub(capacity)])
                            / capacity as f64;
                        assert_eq!(
                            windows.mean(window)[0],
                            expected,
                            "{sample_rate} Hz, frame={frame}, window={window}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn levels_match_bs1770() {
        let amplitude = 0.125_f64;
        let phase_step = 2.0 * PI * 1_000.0 / 48_000.0;
        let z = Complex64::from_polar(1.0, -phase_step);
        // Independent 48 kHz coefficients from BS.1770-5 Tables 1/2: P = A^2 * |H|^2 / 2.
        let response = (1.535_124_859_586_97 - 2.691_696_189_406_38 * z + 1.198_392_810_852_85 * z * z)
            / (1.0 - 1.690_659_293_182_41 * z + 0.732_480_774_215_85 * z * z)
            * (1.0 - 2.0 * z + z * z) / (1.0 - 1.990_047_454_833_98 * z + 0.990_072_250_366_21 * z * z);
        let rms_db = 10.0 * (amplitude * amplitude * response.norm_sqr() / 2.0).log10();
        let peak_db = 20.0 * amplitude.log10();
        let weights = [1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.41, 1.41]; // Annex 3 Tables 4/5.
        // Four seconds exclude startup from the 3 s window; the reference test covers transients.
        let tone: Vec<_> = (0..4 * 48_000)
            .map(|frame| (phase_step * frame as f64).sin() as f32 * amplitude as f32)
            .collect();
        let cases = [
            vec![4.0], vec![2.0], vec![0.0; 8],
            vec![0.0, 0.0, 0.0, 7.0, 0.0, 0.0, 0.0, 0.0],
            vec![0.0, 0.0, 0.0, 0.0, 1.0, -1.0, 0.0, 0.0],
            vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, -1.0],
            vec![1.0; 8], vec![1.0, -0.5, 0.25, 7.0, 2.0, -0.125, 0.5, -1.0],
        ].into_iter().chain((0..8).map(|channel| {
            let mut gains = vec![0.0; 8];
            gains[channel] = 1.0;
            gains
        }));
        for gains in cases {
            let check = |metric, channel: Option<usize>, actual, expected: f64| {
                let expected = expected.max(f64::from(DEFAULT_FLOOR_DB));
                let tolerance = if expected == f64::from(DEFAULT_FLOOR_DB) {
                    0.0
                } else if metric == "true peak" {
                    // Finite reconstruction has passband ripple; energy windows do not.
                    0.001
                } else {
                    2.0e-5
                };
                assert!(
                    (f64::from(actual) - expected).abs() <= tolerance,
                    "{metric}, channel={channel:?}, gains={gains:?}: {actual:.9} vs {expected:.9}"
                );
            };
            let samples: Vec<_> = tone.iter()
                .flat_map(|sample| gains.iter().map(move |gain| sample * gain)).collect();
            let snapshot = LoudnessProcessor::new(LoudnessConfig::default())
                .process_block(&AudioBlock::new(&samples, gains.len(), 48_000.0));
            let weighted_gain: f64 = gains.iter().zip(weights).map(|(&gain, weight)| f64::from(gain).powi(2) * weight).sum();
            let expected = rms_db - 0.691 + 10.0 * weighted_gain.log10();
            for (metric, actual) in [("momentary LUFS", snapshot.momentary_loudness), ("short-term LUFS", snapshot.short_term_loudness)] {
                check(metric, None, actual, expected);
            }
            for (channel, &gain) in gains.iter().enumerate() {
                let gain_db = 20.0 * f64::from(gain).abs().log10();
                for (metric, actual, baseline) in [
                    ("RMS fast", snapshot.rms_fast_db[channel], rms_db),
                    ("RMS slow", snapshot.rms_slow_db[channel], rms_db),
                    ("true peak", snapshot.true_peak_db[channel], peak_db),
                ] {
                    check(metric, Some(channel), actual, baseline + gain_db);
                }
            }
        }
    }

    #[test]
    fn loudness_matches_ebur128_across_startup_layouts_and_rates() {
        let check_tone = |sample_rate, leading_secs, channels| {
            let samples: Vec<_> = std::iter::repeat_n(0.0, (sample_rate * leading_secs) as usize * channels)
                .chain(sine_wave(sample_rate, 4.0, 1_000.0, 0.5).into_iter()
                    .flat_map(|sample| std::iter::repeat_n(sample, channels)))
                .collect();
            assert_loudness_matches_ebur128(
                AudioBlock::new(&samples, channels, sample_rate),
                None,
                &[leading_secs],
            );
        };
        assert_eq!(window_length(11_025.0, 0.3), 3_308);
        check_tone(48_000.0, 1.0, 4);
        // Layout weights are covered separately; avoid a rate/layout cross-product here.
        for sample_rate in [44_100.0, 48_000.0, 96_000.0] {
            check_tone(sample_rate, 0.0, 2);
        }

        let sample_rate = 48_000.0;
        let rate = sample_rate as usize;
        let gains = [0.0625, 0.03125, 0.015625, 0.5, 0.125, -0.0625, 0.25, -0.125];
        let frequencies = [83.0, 137.0, 701.0, 37.0, 997.0, 1_523.0, 503.0, 7_901.0];
        let signal: Vec<[f32; 8]> = (0..rate * 6 + rate * 2 / 5)
            .map(|frame| std::array::from_fn(|channel| {
                let active = match frame / rate {
                    0 => frame >= rate / 4 && matches!(channel, 4 | 5),
                    1 => matches!(channel, 6 | 7),
                    2 => true,
                    3..=5 => channel == 3,
                    _ => matches!(channel, 4 | 5),
                };
                if !active { return 0.0; }
                let phase = 2.0 * PI * frequencies[channel] * frame as f64 / rate as f64;
                phase.sin() as f32 * gains[channel]
            }))
            .collect();
        for order in [[0, 1, 2, 3, 4, 5, 6, 7], [7, 4, 2, 0, 6, 5, 1, 3]] {
            let samples: Vec<_> = signal.iter().flat_map(|frame| order.map(|channel| frame[channel])).collect();
            assert_loudness_matches_ebur128(
                AudioBlock::with_positions(&samples, 8, sample_rate, order.map(|channel| ChannelPosition::SURROUND[channel])),
                Some(&order.map(|channel| SEVEN_ONE_REFERENCE[channel])),
                &[0.0, 0.25, 1.0, 2.0, 3.0, 6.0],
            );
        }
    }

    #[test]
    fn surround_weights_follow_layout_not_channel_order() {
        for (channels, weights) in [
            (4, &[1.0, 1.0, 1.41, 1.41][..]),
            (5, &[1.0, 1.0, 1.0, 1.41, 1.41]),
            (6, &[1.0, 1.0, 1.0, 0.0, 1.41, 1.41]),
            (8, &[1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.41, 1.41]),
        ] {
            let mut layout: Vec<_> = ChannelPosition::fallback(channels).into_iter().zip(weights).collect();
            // Rotations in both directions put each role in every slot without exhaustive permutations.
            for _ in 0..2 {
                for _ in 0..channels {
                    // Inactive entries must not turn a 5.1 layout into 7.1.
                    let positions = std::array::from_fn(|index| {
                        layout.get(index).map_or(ChannelPosition::SideLeft, |entry| entry.0)
                    });
                    let block = AudioBlock::with_positions(&[], channels, 48_000.0, positions);
                    for (position, &(_, expected)) in block.positions.iter().zip(&layout) {
                        assert_eq!(channel_weight(*position), *expected, "{layout:?}");
                    }
                    let mut resolved = block.positions;
                    ChannelPosition::resolve_surrounds(&mut resolved[..channels]);
                    assert_eq!(resolved, block.positions, "{layout:?}");
                    layout.rotate_left(1);
                }
                layout.reverse();
            }
        }
    }

    fn measure_true_peak(samples: &[f32], frames: usize) -> Vec<f32> {
        let coefficients = &*TRUE_PEAK_FIR;
        let mut meter = TruePeakMeter::new();
        let capacity = meter.points.capacity();
        let mut peaks = Vec::with_capacity(samples.len().div_ceil(frames));
        for chunk in samples.chunks(frames) {
            for &sample in chunk {
                meter.process(sample, coefficients);
                assert!(meter.points.len() <= TRUE_PEAK_BUFFER_LEN);
                assert_eq!(meter.points.capacity(), capacity);
            }
            meter.refine();
            assert_eq!(meter.points.len(), 2);
            peaks.push(std::mem::take(&mut meter.peak));
        }
        peaks
    }

    #[test]
    fn true_peak_batches_agree_through_wraps_silence_and_extreme_levels() {
        for (tap, row) in TRUE_PEAK_FIR.iter().enumerate() {
            assert_eq!(row[0], if tap == TRUE_PEAK_TAPS / 2 { 1.0 } else { 0.0 },
                "integer-phase coefficient at tap {tap}");
        }
        let mut seed = 17_u32;
        let mut samples: Vec<f32> = (0..4096).map(|i| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            if i % 131 < 65 { 0.0 } else { (seed as i32 as f32) / i32::MAX as f32 }
        }).collect();
        for value in [0.0, -0.0, 1.0, -1.0, 1.0e-22, -1.0e-22, 1.0e-35, -1.0e-35, 1.0e12, -1.0e12, f32::MAX] {
            samples.extend(std::iter::repeat_n(value, TRUE_PEAK_TAPS * 2));
            samples.extend(std::iter::repeat_n(0.0, TRUE_PEAK_TAPS * 2));
        }
        let expected = measure_true_peak(&samples, 1);
        assert!(expected[expected.len() - TRUE_PEAK_TAPS..].iter().all(|peak| *peak == 0.0));
        for frames in [
            1, 2, 7, 31,
            TRUE_PEAK_BATCH_FRAMES - 1, TRUE_PEAK_BATCH_FRAMES, TRUE_PEAK_BATCH_FRAMES + 1,
            TRUE_PEAK_BATCH_FRAMES * 2 + 1, TRUE_PEAK_BATCH_FRAMES * 16, samples.len(),
        ] {
            let actual = measure_true_peak(&samples, frames);
            assert_eq!(actual.len(), samples.len().div_ceil(frames));
            for (i, (&actual, expected)) in actual.iter().zip(expected.chunks(frames)).enumerate() {
                let expected = expected.iter().copied().fold(0.0_f32, f32::max);
                let sample_peak = samples[i * frames..((i + 1) * frames).min(samples.len())]
                    .iter().copied().map(f32::abs).fold(0.0_f32, f32::max);
                assert!(!actual.is_nan() && actual >= sample_peak, "sample peak lost: frames={frames}, chunk={i}");
                assert!(actual == expected || (actual.is_finite() && expected.is_finite()
                    && (actual - expected).abs() <= 2.0e-6 * actual.max(expected) + 1.0e-37),
                    "frames={frames}, chunk={i}: {actual} vs {expected}");
            }
        }
        let dc = measure_true_peak(&[1.0; TRUE_PEAK_TAPS * 3], 1);
        assert!(dc[TRUE_PEAK_TAPS..].iter().all(|peak| (*peak - 1.0).abs() < 1.0e-6));
    }

    #[test]
    fn true_peak_audio_band_multitones_match_independent_dense_fourier_reference() {
        const PERIOD: usize = 256;
        const OVERSAMPLE: usize = 256;
        let mut planner = FftPlanner::<f64>::new();
        let inverse = planner.plan_fft_inverse(PERIOD * OVERSAMPLE);
        let mut seed = 20260916_u32;
        let mut random = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            f64::from(seed) / f64::from(u32::MAX)
        };
        for rate in [44_100.0_f64, 48_000.0, 96_000.0, 192_000.0] {
            for case in 0..16 {
                let max_bin = (20_000.0 / rate * PERIOD as f64).floor() as usize;
                let tones: Vec<_> = (0..12).map(|_| (
                    1 + (random() * (max_bin - 1) as f64) as usize,
                    0.2 + random() * 0.8,
                    random() * 2.0 * PI,
                )).collect();
                let samples: Vec<_> = (0..PERIOD * 2).map(|frame| {
                    tones.iter().map(|&(bin, amplitude, phase)| {
                        amplitude * (2.0 * PI * bin as f64 * frame as f64 / PERIOD as f64 + phase).sin()
                    }).sum::<f64>() as f32
                }).collect();
                let mut spectrum = vec![Complex64::default(); PERIOD * OVERSAMPLE];
                for &(bin, amplitude, phase) in &tones {
                    let coefficient = Complex64::from_polar(amplitude * 0.5, phase - PI * 0.5);
                    spectrum[bin] += coefficient;
                    spectrum[PERIOD * OVERSAMPLE - bin] += coefficient.conj();
                }
                inverse.process(&mut spectrum);
                let reference = spectrum.iter().map(|x| x.re.abs()).fold(0.0_f64, f64::max);
                let coarser = spectrum.iter().step_by(2).map(|x| x.re.abs()).fold(0.0_f64, f64::max);
                assert!(20.0 * (reference / coarser).log10() < 0.0002, "oracle did not converge");
                let peaks = measure_true_peak(&samples, 1);
                let peak = peaks[PERIOD..].iter().copied().fold(0.0_f32, f32::max);
                let error = 20.0 * (f64::from(peak) / reference).log10();
                assert!(error.abs() < 0.01, "{rate} Hz, case={case}: {error} dB");
            }
        }
    }

    #[test]
    fn true_peak_matches_analytic_phase_tones_at_standard_rates() {
        for sample_rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            for divisor in [3.0, 4.0, 6.0, 8.0, 12.0, 16.0] {
                for phase in [0.0, PI / 32.0, PI / 8.0, PI / 4.0] {
                    let samples: Vec<_> = (-128..1024)
                        .map(|i| (2.0 * PI * f64::from(i) / divisor + phase).sin() as f32 * 1.41)
                        .collect();
                    let mut processor = LoudnessProcessor::new(LoudnessConfig { sample_rate });
                    processor.process_block(&AudioBlock::new(&samples[..128], 1, sample_rate));
                    let peak = processor.process_block(&AudioBlock::new(&samples[128..], 1, sample_rate))
                        .true_peak_db[0];
                    let expected = 20.0 * 1.41_f64.log10();
                    assert!(
                        (f64::from(peak) - expected).abs() < 0.005,
                        "{sample_rate} Hz, divisor={divisor}, phase={phase}: {peak} vs {expected} dBTP"
                    );
                }
            }
        }
    }

    #[test]
    fn true_peak_preserves_channels_and_tails_across_blocks_and_resets() {
        for sample_rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let gains = [2.0_f32, 0.0, -1.5, 1.0, 0.5, -0.125, 0.25, -0.75];
            let silence = [0.0; TRUE_PEAK_TAPS * 2 * MAX_CHANNELS];
            let mut processor = LoudnessProcessor::new(LoudnessConfig { sample_rate });
            // Put the two impulses in separate blocks, then measure only their delayed tail.
            for samples in [&gains[..], &[], &gains[..]] {
                processor.process_block(&AudioBlock::new(samples, MAX_CHANNELS, sample_rate));
            }
            let peak = processor.process_block(&AudioBlock::new(&silence, MAX_CHANNELS, sample_rate))
                .true_peak_db;
            for (channel, (&peak, gain)) in peak.iter().zip(gains).enumerate() {
                // sinc(t) + sinc(t - 1) peaks at 4/pi, including on the LFE channel.
                let expected = (20.0 * (f64::from(gain).abs() * 4.0 / PI).log10())
                    .max(f64::from(DEFAULT_FLOOR_DB));
                assert!((f64::from(peak) - expected).abs() < 0.02,
                    "{sample_rate} Hz, channel={channel}: {peak} vs {expected} dBTP");
            }
            assert_eq!(peak[1], DEFAULT_FLOOR_DB);

            assert_eq!(processor.process_block(&AudioBlock::new(&silence, MAX_CHANNELS, sample_rate)).true_peak_db,
                       [DEFAULT_FLOOR_DB; MAX_CHANNELS]);
            for change in 0..3 {
                processor.process_block(&AudioBlock::new(&[1.0; MAX_CHANNELS], MAX_CHANNELS, sample_rate));
                let (channels, rate) = match change {
                    0 => { processor.reset_audio(); (MAX_CHANNELS, sample_rate) }
                    1 => (MAX_CHANNELS, sample_rate + 1.0),
                    _ => (1, sample_rate),
                };
                assert_eq!(processor.process_block(&AudioBlock::new(&silence, channels, rate)).true_peak_db,
			   [DEFAULT_FLOOR_DB; MAX_CHANNELS]);
            }
        }
    }
}
