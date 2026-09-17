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

// The 49-tap interpolator has 48 nonzero taps; samples cover integer phases.
const TRUE_PEAK_TAPS: usize = 48;
const TRUE_PEAK_4X_DELAY: usize = TRUE_PEAK_TAPS / 4;
const TRUE_PEAK_2X_DELAY: usize = TRUE_PEAK_TAPS / 2;

fn true_peak_coefficient(j: usize, factor: usize) -> f32 {
    let offset = j as f64 - TRUE_PEAK_TAPS as f64 * 0.5;
    let window = 0.5 * (1.0 - (2.0 * PI * j as f64 / TRUE_PEAK_TAPS as f64).cos());
    let x = offset * PI / factor as f64;
    (window * x.sin() / x) as f32
}

type TruePeakFir4x = [[f32; 3]; TRUE_PEAK_4X_DELAY];
type TruePeakFir2x = [f32; TRUE_PEAK_2X_DELAY];
type TruePeakFirs = (TruePeakFir4x, TruePeakFir2x);

static TRUE_PEAK_FIRS: LazyLock<TruePeakFirs> = LazyLock::new(|| {
    (
        std::array::from_fn(|tap| {
            std::array::from_fn(|phase| true_peak_coefficient(tap * 4 + phase + 1, 4))
        }),
        std::array::from_fn(|tap| true_peak_coefficient(tap * 2 + 1, 2)),
    )
});
struct TruePeakMeter {
    delay: [f32; TRUE_PEAK_2X_DELAY * 2],
    write: usize,
    delay_len: usize,
    peak: f32,
}
impl TruePeakMeter {
    fn new(sample_rate: f64) -> Self {
        let delay_len = if sample_rate < 96_000.0 {
            TRUE_PEAK_4X_DELAY
        } else if sample_rate < 192_000.0 {
            TRUE_PEAK_2X_DELAY
        } else {
            0
        };
        Self {
            delay: [0.0; TRUE_PEAK_2X_DELAY * 2],
            write: delay_len,
            delay_len,
            peak: 0.0,
        }
    }

    fn process(&mut self, sample: f32, firs: &TruePeakFirs) {
        self.peak = self.peak.max(sample.abs());
        if self.delay_len == 0 {
            return;
        }

        self.write = if self.write == 0 { self.delay_len } else { self.write } - 1;
        let pos = self.write;
        self.delay[pos] = sample;
        self.delay[pos + self.delay_len] = sample;

        if self.delay_len == TRUE_PEAK_4X_DELAY {
            let mut output = [0.0; 3];
            for i in 0..self.delay_len {
                let (sample, coefficients) = (self.delay[pos + i], firs.0[i]);
                for phase in 0..3 {
                    output[phase] += sample * coefficients[phase];
                }
            }
            self.peak = output.into_iter().map(f32::abs).fold(self.peak, f32::max);
        } else {
            let mut output = 0.0;
            for i in 0..self.delay_len {
                output += self.delay[pos + i] * firs.1[i];
            }
            self.peak = self.peak.max(output.abs());
        }
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
type ChannelState = Option<(EnergyWindows, [f64; 4], TruePeakMeter)>;

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
    channels: Vec<ChannelState>,
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

        let sample_rate = f64::from(self.config.sample_rate);
        let weighting = &self.weighting;
        let firs = &*TRUE_PEAK_FIRS;
        let active_channels = block.stereo_channels.max(
            self.channels.iter().rposition(Option::is_some).map_or(0, |i| i + 1),
        );
        for frame in block.samples.chunks_exact(block.channels) {
            for (channel, &sample) in self.channels[..active_channels].iter_mut().zip(frame) {
                if channel.is_none() {
                    if sample == 0.0 { continue; }
                    let capacities =
                        DEFAULT_WINDOWS.map(|window| window_length(self.config.sample_rate, window));
                    *channel = Some((
                        EnergyWindows::seeded(capacities, capacities[WIN_SHORT_TERM]),
                        [0.0; 4],
                        TruePeakMeter::new(sample_rate),
                    ));
                }
                let (windows, filter, true_peak) = channel.as_mut().unwrap();
                let filtered = k_weighted(sample, filter, weighting);
                let power = filtered * filtered;
                windows.push_nonnegative_finite_wide([if power.is_finite() { power } else { 0.0 }]);
                true_peak.process(sample, firs);
            }
        }
        for (_, state, _) in self.channels.iter_mut().flatten() {
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
        let mut window = EnergyWindows::seeded([4, 2, 1, 4], 0);
        window.push_nonnegative_finite_wide([1.0]);
        window.push_nonnegative_finite_wide([9.0]);
        assert!((window.mean(0)[0] - 5.0).abs() < f64::EPSILON);

        window.push_nonnegative_finite_wide([16.0]);
        window.push_nonnegative_finite_wide([25.0]);
        window.push_nonnegative_finite_wide([36.0]);
        assert!((window.mean(0)[0] - 21.5).abs() < f64::EPSILON);
        assert!((window.mean(1)[0] - 30.5).abs() < f64::EPSILON);
        assert!((window.mean(2)[0] - 36.0).abs() < f64::EPSILON);
        window.push_nonnegative_finite_wide([1.0e100]);
        assert_eq!(window.mean(0)[0], 2.5e99);
        for _ in 0..4 {
            window.push_nonnegative_finite_wide([1.0]);
        }
        assert_eq!(window.mean(0)[0], 1.0);

        let mut window = EnergyWindows::seeded([2; 4], 0);
        for value in [2.0_f64.powi(53), 1.0, 1.0] {
            window.push_nonnegative_finite_wide([value]);
        }
        assert_eq!(window.mean(0)[0], 1.0);

        let mut window = EnergyWindows::seeded([2, 129, 2, 129], 127);
        for value in [1.0e100, 2.0, 1.0e-100, 1.0e-100] {
            window.push_nonnegative_finite_wide([value]);
        }
        assert_eq!(window.mean(0)[0], 1.0e-100);
        for _ in 0..1022 {
            window.push_nonnegative_finite_wide([1.0e-100]);
        }
        assert!((window.mean(1)[0] - 1.0e-100).abs() < 1.0e-113);
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
        use rustfft::num_complex::Complex64;

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
                let tolerance = if expected == f64::from(DEFAULT_FLOOR_DB) { 0.0 } else { 2.0e-5 };
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
                (channels == 8).then_some(&SEVEN_ONE_REFERENCE[..]),
                &[leading_secs],
            );
        };
        assert_eq!(window_length(11_025.0, 0.3), 3_308);
        check_tone(48_000.0, 1.0, 4);
        let gains = [0.0625, 0.03125, 0.015625, 0.5, 0.125, -0.0625, 0.25, -0.125];
        let frequencies = [83.0, 137.0, 701.0, 37.0, 997.0, 1_523.0, 503.0, 7_901.0];
        for sample_rate in [44_100.0, 48_000.0, 96_000.0] {
            for channels in [1, 2, 4, 5, 6, 8] {
                check_tone(sample_rate, 0.0, channels);
            }
            let rate = sample_rate as usize;
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

    #[test]
    fn true_peak_matches_ebur128_at_standard_rates() {
        for (sample_rate, delay_len) in [
            (48_000.0_f32, TRUE_PEAK_4X_DELAY),
            (96_000.0, TRUE_PEAK_2X_DELAY),
            (192_000.0, 0),
        ] {
            let meter = TruePeakMeter::new(f64::from(sample_rate));
            assert_eq!(meter.delay_len, delay_len);

            let samples = sine_wave(sample_rate, 0.01, 17_000.0, 0.9);
            let ours = LoudnessProcessor::new(LoudnessConfig { sample_rate })
            .process_block(&AudioBlock::new(&samples, 1, sample_rate))
            .true_peak_db[0] as f64;

            let mut reference =
                EbuR128::new(1, sample_rate as u32, Mode::TRUE_PEAK | Mode::S).unwrap();
            reference.add_frames_f32(&samples).unwrap();
            let expected = 20.0 * reference.true_peak(0).unwrap().log10();
            assert!(
                (ours - expected).abs() < 1.0e-3,
                "{sample_rate} Hz true peak: {ours} vs {expected} dBTP"
            );
        }
    }
}
