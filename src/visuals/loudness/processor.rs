// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::dsp::{AudioBlock, ChannelPosition, WindowedMeans};
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
type ActiveChannel = (WindowedMeans<1, 4>, [f64; 4], TruePeakMeter);
type ChannelState = Option<ActiveChannel>;

pub(super) const MAX_CHANNELS: usize = crate::dsp::MAX_AUDIO_CHANNELS;

fn channel_weight(position: ChannelPosition) -> f64 {
    match position {
        ChannelPosition::LowFrequency => 0.0,
        ChannelPosition::RearLeft
        | ChannelPosition::RearRight
        | ChannelPosition::SideLeft
        | ChannelPosition::SideRight => 1.41,
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
            positions: [ChannelPosition::Unknown; MAX_CHANNELS],
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
        self.channels.iter_mut().for_each(|channel| *channel = None);
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
                        WindowedMeans::with_leading_zeros(capacities, capacities[WIN_SHORT_TERM]),
                        [0.0; 4],
                        TruePeakMeter::new(sample_rate),
                    ));
                }
                let (windows, filter, true_peak) = channel.as_mut().unwrap();
                let filtered = k_weighted(sample, filter, weighting);
                let power = filtered * filtered;
                windows.push_nonnegative_finite([if power.is_finite() { power } else { 0.0 }]);
                true_peak.process(sample, firs);
            }
        }
        for (_, state, _) in self.channels.iter_mut().flatten() {
            state.iter_mut().for_each(flush_denormal_f64);
        }

        let floor = DEFAULT_FLOOR_DB;
        let mut snapshot = LoudnessSnapshot::with_floor(floor, 0);
        let mut weighted_short_term = 0.0;
        let mut weighted_momentary = 0.0;

        for (channel_index, channel_state) in self.channels.iter_mut().enumerate() {
            let Some((windows, _, true_peak)) = channel_state else { continue };
            let weight = channel_weight(block.positions[channel_index]);
            weighted_short_term += windows.mean(WIN_SHORT_TERM)[0] * weight;
            weighted_momentary += windows.mean(WIN_MOMENTARY)[0] * weight;
            snapshot.rms_fast_db[channel_index] =
                power_to_db(windows.mean(WIN_RMS_FAST)[0] as f32, floor);
            snapshot.rms_slow_db[channel_index] =
                power_to_db(windows.mean(WIN_RMS_SLOW)[0] as f32, floor);
            let peak = std::mem::take(&mut true_peak.peak);
            snapshot.true_peak_db[channel_index] = power_to_db(peak * peak, floor);
        }

        snapshot.short_term_loudness = mean_square_to_lufs(weighted_short_term, floor);
        snapshot.momentary_loudness = mean_square_to_lufs(weighted_momentary, floor);
        snapshot.channel_count = self.channels.len();
        snapshot.positions = block.positions;

        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ebur128::{EbuR128, Mode};

    fn sine_wave(rate: f32, secs: f32, freq: f32, amp: f32) -> Vec<f32> {
        crate::util::audio::sine_wave(freq, rate, (rate * secs) as usize, amp)
    }

    fn assert_loudness_matches_ebur128(
        sample_rate: f32,
        tone_secs: f32,
        leading_secs: f32,
        channels: usize,
    ) {
        let samples: Vec<_> =
            std::iter::repeat_n(0.0, (sample_rate * leading_secs) as usize * channels)
                .chain(sine_wave(sample_rate, tone_secs, 1_000.0, 0.5).into_iter().flat_map(
                    |sample| std::iter::repeat_n(sample, channels),
                ))
                .collect();
        let ours = LoudnessProcessor::new(LoudnessConfig { sample_rate })
            .process_block(&AudioBlock::new(&samples, channels, sample_rate));
        let mut reference = EbuR128::new(channels as u32, sample_rate as u32, Mode::S).unwrap();
        reference.add_frames_f32(&samples).unwrap();

        for (actual, expected) in [
            (f64::from(ours.momentary_loudness), reference.loudness_momentary().unwrap()),
            (f64::from(ours.short_term_loudness), reference.loudness_shortterm().unwrap()),
        ] {
            assert!(
                (actual - expected).abs() < 0.001,
                "{sample_rate}Hz/{channels}ch after {leading_secs}+{tone_secs}s: {actual:.6} vs {expected:.6}"
            );
        }
    }

    #[test]
    fn rolling_mean_square_tracks_average() {
        let mut window = WindowedMeans::<1, 4>::new([4, 2, 1, 4]);
        window.push_nonnegative_finite([1.0]);
        window.push_nonnegative_finite([9.0]);
        assert!((window.mean(0)[0] - 5.0).abs() < f64::EPSILON);

        window.push_nonnegative_finite([16.0]);
        window.push_nonnegative_finite([25.0]);
        window.push_nonnegative_finite([36.0]);
        assert!((window.mean(0)[0] - 21.5).abs() < f64::EPSILON);
        assert!((window.mean(1)[0] - 30.5).abs() < f64::EPSILON);
        assert!((window.mean(2)[0] - 36.0).abs() < f64::EPSILON);
    }

    #[test]
    fn rms_tracks_amplitude() {
        let measure = |amp| {
            let samples = sine_wave(DEFAULT_SAMPLE_RATE, 3.0, 1000.0, amp);
            let block = AudioBlock::new(&samples, 1, DEFAULT_SAMPLE_RATE);
            LoudnessProcessor::new(LoudnessConfig::default())
                .process_block(&block)
                .rms_fast_db[0]
        };
        let delta = measure(0.5) - measure(0.25);
        assert!((5.8..6.3).contains(&delta), "RMS delta was {delta:.4} dB");
    }

    #[test]
    fn loudness_matches_ebur128_across_startup_layouts_and_rates() {
        assert_eq!(window_length(11_025.0, 0.3), 3_308);
        for tone_secs in [0.1, 0.4, 1.0] {
            assert_loudness_matches_ebur128(48_000.0, tone_secs, 0.0, 1);
        }
        for sample_rate in [44_100.0, 48_000.0, 96_000.0] {
            for channels in [2, 4, 5, 6] {
                assert_loudness_matches_ebur128(sample_rate, 4.0, 0.0, channels);
            }
        }
        assert_loudness_matches_ebur128(48_000.0, 0.1, 1.0, 4);
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
