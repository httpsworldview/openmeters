// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::dsp::{AudioBlock, ChannelPosition, RunningMeans, SampleRate};
use crate::util::audio::{flush_denormal_f64, kaiser};
use std::{f64::consts::PI, sync::LazyLock};

// BS.1770 LUFS offset.
const LOUDNESS_OFFSET: f64 = -0.691;
// Analysis floor, not the visible meter floor.
pub(super) const DEFAULT_FLOOR_DB: f32 = -99.9;

// K-weighted windows (seconds): short-term, momentary, fast RMS, slow RMS.
const DEFAULT_WINDOWS: [f32; 4] = [3.0, 0.4, 0.3, 1.0];
const SHORT_TERM: usize = 0;
const MOMENTARY: usize = 1;
const RMS_FAST: usize = 2;
const RMS_SLOW: usize = 3;

// Gating blocks overlap by 75 %; Tech 3342 samples short-term loudness at least as often.
const HOP_SECS: f32 = 0.1;
// Tech 3341: LRA is not yet stable for the first 60 s of a measurement.
const STABLE_HOPS: u64 = 600;
const ABSOLUTE_GATE_LUFS: f64 = -70.0;
const HISTOGRAM_BINS: usize = 10_000;
const HISTOGRAM_BINS_PER_LU: f64 = 100.0;

// Numerator, denominator, output gain.
type KWeighting = ([f64; 5], [f64; 5], f64);

fn k_weighting_coefficients(fs: f64) -> KWeighting {
    let (f0, g, q) = (
        1_681.974_450_955_533,
        3.999_843_853_973_347,
        0.707_175_236_955_419_6,
    );
    // Past Nyquist the prewarp would turn unstable, so rates that low keep the filters below it.
    let k = (PI * (f0 / fs).min(0.49)).tan();
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
    let warp = |fs: f64| {
        let k = (PI * (f0 / fs).min(0.49)).tan();
        (k, 1.0 + k / q + k * k)
    };
    let (k, a0) = warp(fs);
    // The 48 kHz table leaves this numerator unnormalized; Annex 1 keeps that response.
    let gain = warp(48_000.0).1 / a0;
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
    // An output gain leaves the 4th-order coefficients, and their precision near DC, alone.
    (conv(pb, rb), conv(pa, ra), gain)
}

fn mean_square_to_db(mean_square: f64, offset: f64) -> f32 {
    if mean_square > 0.0 {
        mean_square
            .log10()
            .mul_add(10.0, offset)
            .max(f64::from(DEFAULT_FLOOR_DB)) as f32
    } else {
        DEFAULT_FLOOR_DB
    }
}

fn window_length(sample_rate: SampleRate, window_secs: f32) -> usize {
    (sample_rate.hz() * window_secs).round().max(1.0) as usize
}

const TRUE_PEAK_TAPS: usize = 48;
const TRUE_PEAK_PHASES: usize = 8;
const TRUE_PEAK_BATCH_FRAMES: usize = 256;
const TRUE_PEAK_BUFFER_LEN: usize = TRUE_PEAK_BATCH_FRAMES * TRUE_PEAK_PHASES + 2;
type TruePeakFir = [[f32; TRUE_PEAK_PHASES]; TRUE_PEAK_TAPS];
const _: () = assert!(TRUE_PEAK_TAPS.is_multiple_of(2));

// 8x Kaiser-windowed sinc; 24-input-sample delay.
static TRUE_PEAK_FIR: LazyLock<TruePeakFir> = LazyLock::new(|| {
    let radius = TRUE_PEAK_TAPS as f64 / 2.0;
    let mut coefficients: TruePeakFir = std::array::from_fn(|tap| {
        std::array::from_fn(|phase| {
            if phase == 0 {
                return if tap == TRUE_PEAK_TAPS / 2 { 1.0 } else { 0.0 };
            }
            let x = tap as f64 - radius + phase as f64 / TRUE_PEAK_PHASES as f64;
            let sinc = (PI * x).sin() / (PI * x);
            (sinc * kaiser(8.6, x / radius)) as f32
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
        self.write = self.write.checked_sub(1).unwrap_or(TRUE_PEAK_TAPS - 1);
        let pos = self.write;
        self.delay[pos] = sample;
        self.delay[pos + TRUE_PEAK_TAPS] = sample;
        let input = &self.delay[pos..pos + TRUE_PEAK_TAPS];
        // Even/odd tap sums shorten dependency chains while phases stay SIMD-friendly.
        let mut sums = [[0.0; TRUE_PEAK_PHASES]; 2];
        for (samples, rows) in input
            .as_chunks::<2>()
            .0
            .iter()
            .zip(coefficients.as_chunks::<2>().0)
        {
            for lane in 0..2 {
                for phase in 0..TRUE_PEAK_PHASES {
                    // Avoid software FMA calls on CPUs without hardware support.
                    sums[lane][phase] += samples[lane] * rows[lane][phase];
                }
            }
        }
        let output: [f32; TRUE_PEAK_PHASES] =
            std::array::from_fn(|phase| sums.iter().map(|sum| sum[phase]).sum());
        self.points.extend_from_slice(&output);
        let level = sample.abs();
        self.peak = if level > self.peak { level } else { self.peak };
        if self.points.len() == TRUE_PEAK_BUFFER_LEN {
            self.refine();
        }
    }

    fn refine(&mut self) {
        let n = self.points.len() - 2;
        // Independent maxima and unconditional assignments keep reductions vectorizable.
        let mut peaks = [self.peak; TRUE_PEAK_PHASES];
        for points in self
            .points
            .windows(TRUE_PEAK_PHASES + 2)
            .step_by(TRUE_PEAK_PHASES)
        {
            for (i, peak) in peaks.iter_mut().enumerate() {
                let [a, b, c] = [points[i], points[i + 1], points[i + 2]];
                let curve = (b - a) + (b - c);
                let delta = c - a;
                // A flat triple has no vertex; never extrapolate beyond half a phase.
                let denominator = if curve == 0.0 { 1.0 } else { curve };
                #[expect(
                    clippy::manual_clamp,
                    reason = "min/max also bound NaN ratios after overflow"
                )]
                let offset = (delta * 0.5 / denominator).max(-0.5).min(0.5);
                let value = (b + offset * 0.5 * (delta - offset * curve)).abs();
                let candidate = if value > c.abs() { value } else { c.abs() };
                *peak = if candidate > *peak { candidate } else { *peak };
            }
        }
        for value in peaks {
            self.peak = if value > self.peak { value } else { self.peak };
        }
        // The untouched final two points bridge batches and audio blocks.
        self.points.copy_within(n.., 0);
        self.points.truncate(2);
    }
}

fn k_weighted(sample: f32, state: &mut [f64; 4], coefficients: &KWeighting) -> f64 {
    let (b, a, gain) = coefficients;
    let x = f64::from(sample);
    let y = b[0] * x + state[0];
    state[0] = b[1] * x + state[1] - a[1] * y;
    state[1] = b[2] * x + state[2] - a[2] * y;
    state[2] = b[3] * x + state[3] - a[3] * y;
    state[3] = b[4] * x - a[4] * y;
    y * gain
}

type EnergyWindows = RunningMeans<1, 4>;
type ChannelState = (EnergyWindows, [f64; 4], TruePeakMeter);

#[cold]
fn new_channel(sample_rate: SampleRate) -> ChannelState {
    let capacities = DEFAULT_WINDOWS.map(|window| window_length(sample_rate, window));
    (
        EnergyWindows::new(capacities),
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

fn weighted_energy(
    channels: &[Option<ChannelState>],
    positions: &[ChannelPosition; MAX_CHANNELS],
    window: usize,
) -> f64 {
    channels
        .iter()
        .zip(positions)
        .filter_map(|(channel, &position)| {
            Some(channel.as_ref()?.0.mean(window)[0] * channel_weight(position))
        })
        .sum()
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Programme {
    pub integrated: Option<f32>,
    // 10th and 95th percentiles
    pub range: Option<[f32; 2]>,
    pub stable: bool,
}

type Bin = (u64, f64);

fn total(bins: &[Bin]) -> Bin {
    bins.iter().fold((0, 0.0), |(n, sum), &(count, energy)| {
        (n + count, sum + energy)
    })
}

fn bin_loudness((count, energy): Bin) -> Option<f32> {
    (count > 0).then(|| mean_square_to_db(energy / count as f64, LOUDNESS_OFFSET))
}

struct Histogram(Box<[Bin]>);

impl Default for Histogram {
    fn default() -> Self {
        Self(vec![(0, 0.0); HISTOGRAM_BINS].into())
    }
}

impl Histogram {
    fn insert(&mut self, energy: f64) -> bool {
        let lufs = f64::from(mean_square_to_db(energy, LOUDNESS_OFFSET));
        let bin = (lufs - ABSOLUTE_GATE_LUFS) * HISTOGRAM_BINS_PER_LU;
        if bin <= 0.0 {
            return false;
        }
        let (count, sum) = &mut self.0[(bin as usize).min(HISTOGRAM_BINS - 1)];
        *count += 1;
        *sum += energy;
        true
    }

    fn gated(&self, ratio: f64) -> &[Bin] {
        let (count, energy) = total(&self.0);
        let gate = energy / count as f64 * ratio;
        let first = self
            .0
            .iter()
            .position(|&(count, energy)| energy > gate * count as f64);
        &self.0[first.unwrap_or(HISTOGRAM_BINS)..]
    }

    fn range(&self) -> Option<[f32; 2]> {
        let bins = self.gated(0.01);
        let last = total(bins).0.checked_sub(1)?;
        let level = |percent| {
            let rank = (last * percent + 50) / 100;
            let mut below = 0;
            bin_loudness(*bins.iter().find(|bin| {
                below += bin.0;
                rank < below
            })?)
        };
        Some([level(10)?, level(95)?])
    }
}

#[derive(Default)]
struct Measurement {
    blocks: Histogram,
    short_term: Histogram,
    hops: u64,
    since_hop: usize,
    programme: Programme,
}

impl Measurement {
    fn hop(
        &mut self,
        channels: &[Option<ChannelState>],
        positions: &[ChannelPosition; MAX_CHANNELS],
    ) {
        self.hops += 1;
        self.programme.stable = self.hops >= STABLE_HOPS;
        let energy_of = |window: usize| {
            let hops = (DEFAULT_WINDOWS[window] / HOP_SECS).round() as u64;
            (self.hops >= hops).then(|| weighted_energy(channels, positions, window))
        };
        if energy_of(MOMENTARY).is_some_and(|energy| self.blocks.insert(energy)) {
            self.programme.integrated = bin_loudness(total(self.blocks.gated(0.1)));
        }
        if energy_of(SHORT_TERM).is_some_and(|energy| self.short_term.insert(energy)) {
            self.programme.range = self.short_term.range();
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LoudnessSnapshot {
    pub elapsed: f32,
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
        pub sample_rate: SampleRate = SampleRate::DEFAULT,
    }
}

pub struct LoudnessProcessor {
    config: LoudnessConfig,
    channels: Vec<Option<ChannelState>>,
    weighting: KWeighting,
    measurement: Measurement,
}

impl LoudnessProcessor {
    pub fn new(config: LoudnessConfig) -> Self {
        Self {
            weighting: k_weighting_coefficients(f64::from(config.sample_rate.hz())),
            channels: Vec::new(),
            measurement: Measurement::default(),
            config,
        }
    }

    pub fn reset_audio(&mut self) {
        self.channels.fill_with(|| None);
    }

    pub fn reset_measurement(&mut self) {
        self.measurement = Measurement::default();
    }

    pub fn process_block(&mut self, block: &AudioBlock<'_>) -> (LoudnessSnapshot, Programme) {
        let sample_rate = block.sample_rate;
        if self.config.sample_rate != sample_rate {
            self.config.sample_rate = sample_rate;
            self.weighting = k_weighting_coefficients(f64::from(sample_rate.hz()));
            self.channels.clear();
        }
        if self.channels.len() != block.channels {
            self.channels = (0..block.channels).map(|_| None).collect();
        }

        let weighting = &self.weighting;
        let firs = &*TRUE_PEAK_FIR;
        let active_channels = block.stereo_channels.max(
            self.channels
                .iter()
                .rposition(Option::is_some)
                .map_or(0, |i| i + 1),
        );
        let hop = window_length(sample_rate, HOP_SECS);
        for frame in block.samples.chunks_exact(block.channels) {
            for (channel, &sample) in self.channels[..active_channels].iter_mut().zip(frame) {
                if channel.is_none() {
                    if sample == 0.0 {
                        continue;
                    }
                    *channel = Some(new_channel(sample_rate));
                }
                let (windows, filter, true_peak) = channel.as_mut().unwrap();
                let filtered = k_weighted(sample, filter, weighting);
                let power = filtered * filtered;
                windows.push_nonnegative_finite_wide([if power.is_finite() { power } else { 0.0 }]);
                true_peak.process(sample, firs);
            }
            let measurement = &mut self.measurement;
            measurement.since_hop += 1;
            if measurement.since_hop >= hop {
                measurement.since_hop = 0;
                measurement.hop(&self.channels, &block.positions);
            }
        }
        for (_, state, true_peak) in self.channels.iter_mut().flatten() {
            true_peak.refine();
            state.iter_mut().for_each(flush_denormal_f64);
        }

        let mut snapshot = LoudnessSnapshot::with_floor(DEFAULT_FLOOR_DB, self.channels.len());
        for (channel_index, channel_state) in self.channels.iter_mut().enumerate() {
            let Some((windows, _, true_peak)) = channel_state else {
                continue;
            };
            let [rms_fast, rms_slow] = [RMS_FAST, RMS_SLOW].map(|window| windows.mean(window)[0]);
            snapshot.rms_fast_db[channel_index] = mean_square_to_db(rms_fast, 0.0);
            snapshot.rms_slow_db[channel_index] = mean_square_to_db(rms_slow, 0.0);
            let peak = f64::from(std::mem::take(&mut true_peak.peak));
            snapshot.true_peak_db[channel_index] = mean_square_to_db(peak * peak, 0.0);
        }

        let loudness = |window| {
            let energy = weighted_energy(&self.channels, &block.positions, window);
            mean_square_to_db(energy, LOUDNESS_OFFSET)
        };
        snapshot.elapsed = block.duration();
        snapshot.short_term_loudness = loudness(SHORT_TERM);
        snapshot.momentary_loudness = loudness(MOMENTARY);
        snapshot.positions = block.positions;

        (snapshot, self.measurement.programme)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::AudioFormat;
    use crate::util::audio::sine_wave;
    use ebur128::{Channel as ReferenceChannel, EbuR128, Mode};
    use rustfft::{FftPlanner, num_complex::Complex64};
    use std::collections::{BTreeSet, VecDeque};

    const SEVEN_ONE_REFERENCE: [ReferenceChannel; 8] = {
        use ReferenceChannel::*;
        [Left, Right, Center, Unused, Mp135, Mm135, Mp090, Mm090]
    };
    const RATE: SampleRate = SampleRate::DEFAULT;

    fn assert_loudness_matches_ebur128(
        samples: &[f32],
        format: AudioFormat,
        channel_map: Option<&[ReferenceChannel]>,
        transitions: &[f32],
    ) {
        let block = AudioBlock::new(samples, format);
        let config = LoudnessConfig {
            sample_rate: block.sample_rate,
        };
        let mut processor = LoudnessProcessor::new(config);
        let rate = block.sample_rate.get();
        let mut reference = EbuR128::new(block.channels as u32, rate, Mode::S | Mode::I).unwrap();
        if let Some(channel_map) = channel_map {
            reference.set_channel_map(channel_map).unwrap();
        }
        let hop = window_length(block.sample_rate, HOP_SECS);
        let short_term_length = window_length(block.sample_rate, DEFAULT_WINDOWS[SHORT_TERM]);
        // Reference queries rescan entire windows; concentrate them at transitions and window expiry.
        let checkpoints: BTreeSet<_> = transitions
            .iter()
            .flat_map(|&start| {
                [0.0, 0.1, 0.4, 1.0, 3.0]
                    .map(|delay| ((start + delay) * block.sample_rate.hz()).round() as usize)
            })
            .flat_map(|frame| [frame.saturating_sub(1), frame, frame + 1])
            .chain([1, 18, 273, block.frame_count()])
            .chain((hop..=block.frame_count()).step_by(hop))
            .filter(|&frame| frame > 0 && frame <= block.frame_count())
            .collect();
        let mut chunks = [1, 17, 255, 1_024, 4_093].into_iter().cycle();
        let mut frame = 0;
        let (mut snapshot, mut programme) = Default::default();
        let mut short_term = Vec::new();
        for checkpoint in checkpoints {
            while frame < checkpoint {
                let end = (frame + chunks.next().unwrap()).min(checkpoint);
                let chunk = &block.samples[frame * block.channels..end * block.channels];
                (snapshot, programme) = processor.process_block(&AudioBlock::new(chunk, format));
                reference.add_frames_f32(chunk).unwrap();
                frame = end;
            }
            let context = format!(
                "{rate} Hz, {:?}, frame={frame}",
                &block.positions[..block.channels]
            );
            for (metric, actual, expected) in [
                (
                    "momentary",
                    snapshot.momentary_loudness,
                    reference.loudness_momentary().unwrap(),
                ),
                (
                    "short-term",
                    snapshot.short_term_loudness,
                    reference.loudness_shortterm().unwrap(),
                ),
            ] {
                let expected = expected.max(f64::from(DEFAULT_FLOOR_DB));
                assert!(
                    (f64::from(actual) - expected).abs() < 1.0e-4,
                    "{context}, {metric}: {actual:.9} vs {expected:.9} LUFS"
                );
            }
            if frame % hop == 0 && frame >= short_term_length {
                short_term.push(reference.loudness_shortterm().unwrap());
            }
            let integrated = reference.loudness_global().unwrap();
            let range = tech_3342_range(&short_term);
            assert_eq!(
                programme.integrated.is_some(),
                integrated.is_finite(),
                "{context}"
            );
            if let Some(actual) = programme.integrated {
                let error = f64::from(actual) - integrated;
                assert!(error.abs() < 1.0e-4, "{context}: I off by {error:e} LU");
            }
            assert_eq!(programme.range.is_some(), range.is_some(), "{context}");
            for (actual, expected) in programme
                .range
                .into_iter()
                .flatten()
                .zip(range.into_iter().flatten())
            {
                let error = f64::from(actual) - expected;
                assert!(
                    error.abs() < 0.01 + 1.0e-4,
                    "{context}: LRA percentile off by {error:e} LU"
                );
            }
        }
        let (whole, whole_programme) = LoudnessProcessor::new(config).process_block(&block);
        assert_eq!(snapshot.momentary_loudness, whole.momentary_loudness);
        assert_eq!(snapshot.short_term_loudness, whole.short_term_loudness);
        assert_eq!(snapshot.rms_fast_db, whole.rms_fast_db);
        assert_eq!(snapshot.rms_slow_db, whole.rms_slow_db);
        assert_eq!(programme, whole_programme);
    }

    // EBU Tech 3342's MATLAB reference implementation
    fn tech_3342_range(short_term_loudness: &[f64]) -> Option<[f64; 2]> {
        let abs_gated: Vec<_> = (short_term_loudness.iter().copied())
            .filter(|&loudness| loudness >= -70.0)
            .collect();
        let power = abs_gated
            .iter()
            .map(|l| 10.0_f64.powf(l / 10.0))
            .sum::<f64>();
        let integrated = 10.0 * (power / abs_gated.len() as f64).log10();
        let mut rel_gated: Vec<_> = abs_gated
            .into_iter()
            .filter(|&loudness| loudness >= integrated - 20.0)
            .collect();
        rel_gated.sort_by(f64::total_cmp);
        let n = rel_gated.len();
        let percentile =
            |p: f64| rel_gated[((n - 1) as f64 * p / 100.0 + 1.0).round() as usize - 1];
        (n > 0).then(|| [percentile(10.0), percentile(95.0)])
    }

    #[test]
    fn loudness_range_meets_ebu_tech_3342_cases() {
        for (levels, expected) in [
            (&[-20.0, -30.0][..], 10.0),
            (&[-20.0, -15.0], 5.0),
            (&[-40.0, -20.0], 20.0),
            (&[-50.0, -35.0, -20.0, -35.0, -50.0], 15.0),
        ] {
            let samples: Vec<_> = (levels.iter())
                .flat_map(|dbfs: &f32| {
                    let amplitude = 10.0_f32.powf(dbfs / 20.0) * std::f32::consts::SQRT_2;
                    sine_wave(1_000.0, RATE.hz(), 20 * RATE.get() as usize, amplitude)
                })
                .collect();
            let [low, high] = LoudnessProcessor::new(LoudnessConfig::default())
                .process_block(&AudioBlock::fallback(&samples, 1, RATE))
                .1
                .range
                .unwrap();
            let actual = high - low;
            assert!((actual - expected).abs() <= 1.0, "{levels:?}: {actual} LU");
        }
    }

    #[test]
    fn programme_outlives_timeline_resets_until_its_own_reset() {
        fn process(
            processor: &mut LoudnessProcessor,
            samples: &[f32],
            format: AudioFormat,
        ) -> Programme {
            processor.process_block(&AudioBlock::new(samples, format)).1
        }
        let (hop, block, short_term) = (4_800, 19_200, 144_000);
        let mono = AudioFormat::fallback(1, RATE);
        let tone = sine_wave(1_000.0, RATE.hz(), short_term, 0.5);
        let mut processor = LoudnessProcessor::new(LoudnessConfig::default());
        let programme = process(&mut processor, &tone, mono);
        assert!(programme.range.is_some() && !programme.stable);

        processor.reset_audio();
        let fast = AudioFormat::fallback(1, SampleRate::must(96_000));
        // The gap leaves more than a 48 kHz hop pending, which the next block must still close.
        let gap = vec![0.0; short_term + hop];
        assert_eq!(process(&mut processor, &gap, fast), programme);
        assert_ne!(process(&mut processor, &tone[..48_000], mono), programme);

        processor.reset_measurement();
        let restarted = process(&mut processor, &tone[..block - 1], mono);
        assert_eq!(restarted, Programme::default());
        assert_ne!(
            process(&mut processor, &tone[block - 1..block], mono),
            restarted
        );

        let silence = vec![0.0; STABLE_HOPS as usize * hop - block];
        let (unstable, last) = silence.split_at(silence.len() - 1);
        assert!(!process(&mut processor, unstable, mono).stable);
        assert!(process(&mut processor, last, mono).stable);
    }

    fn assert_energy_windows_match_direct_sums(
        capacities: [usize; 4],
        powers: impl IntoIterator<Item = f64>,
        relative_tolerance: f64,
    ) {
        let mut windows = EnergyWindows::new(capacities);
        let capacities = capacities.map(|capacity| capacity.max(1));
        let len = capacities.into_iter().max().unwrap();
        let mut history = VecDeque::from(vec![0.0; len]);
        for window in 0..4 {
            assert_eq!(windows.mean(window)[0], 0.0);
        }
        for (i, power) in powers.into_iter().enumerate() {
            windows.push_nonnegative_finite_wide([power]);
            history.push_back(power);
            history.pop_front();
            for (window, &capacity) in capacities.iter().enumerate() {
                let expected = history.iter().rev().take(capacity).sum::<f64>() / capacity as f64;
                let actual = windows.mean(window)[0];
                assert!(
                    (actual - expected).abs() <= relative_tolerance * expected,
                    "{capacities:?}, sample={i}, window={window}: {actual:e} != {expected:e}"
                );
            }
        }
    }

    #[test]
    fn energy_windows_match_direct_sums_across_ring_boundaries() {
        for len in 0_usize..=256 {
            let capacities = [len, len / 2, 1, len * 3 / 4];
            // Dyadic inputs and their sums are exact, so no tolerance is needed.
            let powers = (0..len * 3 + 33)
                .map(|i| {
                    if i % (len + 1) < len / 2 {
                        0.0
                    } else {
                        ((i * 991 + len * 17) % 65536) as f64 / 65536.0
                    }
                })
                .chain(std::iter::repeat_n(0.0, len + 1));
            assert_energy_windows_match_direct_sums(capacities, powers, 0.0);
        }
    }

    #[test]
    fn energy_windows_preserve_small_values_through_wide_fallback_and_wraps() {
        for len in [
            1, 2, 3, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, 257, 513,
        ] {
            let capacities = [len, (len / 2).max(1), (len / 3).max(1), 1];
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
            assert_energy_windows_match_direct_sums(capacities, powers, 1.0e-13);
        }
    }

    #[test]
    fn energy_windows_match_reference_at_audio_sample_rates() {
        for rate in [
            8_000, 11_025, 22_050, 44_100, 48_000, 96_000, 192_000, 768_000,
        ] {
            let sample_rate = SampleRate::must(rate);
            let capacities = DEFAULT_WINDOWS.map(|secs| window_length(sample_rate, secs));
            let len = capacities[0];
            let mut windows = EnergyWindows::new(capacities);
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
                            "{rate} Hz, frame={frame}, window={window}"
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
        let response = (1.535_124_859_586_97 - 2.691_696_189_406_38 * z
            + 1.198_392_810_852_85 * z * z)
            / (1.0 - 1.690_659_293_182_41 * z + 0.732_480_774_215_85 * z * z)
            * (1.0 - 2.0 * z + z * z)
            / (1.0 - 1.990_047_454_833_98 * z + 0.990_072_250_366_21 * z * z);
        let rms_db = 10.0 * (amplitude * amplitude * response.norm_sqr() / 2.0).log10();
        let peak_db = 20.0 * amplitude.log10();
        let weights = [1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.41, 1.41]; // Annex 3 Tables 4/5.
        // Final 3 s window after 1 s warm-up.
        let tone: Vec<_> = (0..4 * 48_000)
            .map(|frame| (phase_step * frame as f64).sin() as f32 * amplitude as f32)
            .collect();
        let cases = [
            vec![4.0],
            vec![2.0],
            vec![1.0e21],
            vec![0.0; 8],
            vec![0.0, 0.0, 0.0, 7.0, 0.0, 0.0, 0.0, 0.0],
            vec![0.0, 0.0, 0.0, 0.0, 1.0, -1.0, 0.0, 0.0],
            vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, -1.0],
            vec![1.0; 8],
            vec![1.0, -0.5, 0.25, 7.0, 2.0, -0.125, 0.5, -1.0],
        ]
        .into_iter()
        .chain((0..8).map(|channel| {
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
            let samples: Vec<_> = tone
                .iter()
                .flat_map(|sample| gains.iter().map(move |gain| sample * gain))
                .collect();
            let (snapshot, _) = LoudnessProcessor::new(LoudnessConfig::default())
                .process_block(&AudioBlock::fallback(&samples, gains.len(), RATE));
            let weighted_gain: f64 = gains
                .iter()
                .zip(weights)
                .map(|(&gain, weight)| f64::from(gain).powi(2) * weight)
                .sum();
            let expected = rms_db - 0.691 + 10.0 * weighted_gain.log10();
            for (metric, actual) in [
                ("momentary LUFS", snapshot.momentary_loudness),
                ("short-term LUFS", snapshot.short_term_loudness),
            ] {
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
        // Annex 1: other rates keep the 48 kHz response; warping the shelf costs < 0.01 dB here.
        for rate in [44_100, 96_000, 192_000] {
            let sample_rate = SampleRate::must(rate);
            let tone = sine_wave(
                1_000.0,
                sample_rate.hz(),
                rate as usize * 4,
                amplitude as f32,
            );
            let (snapshot, _) = LoudnessProcessor::new(LoudnessConfig { sample_rate })
                .process_block(&AudioBlock::fallback(&tone, 1, sample_rate));
            let error = f64::from(snapshot.rms_slow_db[0]) - rms_db;
            assert!(error.abs() < 0.01, "{rate} Hz: {error:+.4} dB");
        }
    }

    #[test]
    fn loudness_matches_ebur128_across_startup_layouts_and_rates() {
        let check_tone = |leading_secs, channels, amplitude| {
            let rate = RATE.hz();
            let samples: Vec<_> =
                std::iter::repeat_n(0.0, (rate * leading_secs) as usize * channels)
                    .chain(
                        sine_wave(1_000.0, rate, (rate * 4.0) as usize, amplitude)
                            .into_iter()
                            .flat_map(|sample| std::iter::repeat_n(sample, channels)),
                    )
                    .collect();
            let format = AudioFormat::fallback(channels, RATE);
            assert_loudness_matches_ebur128(&samples, format, None, &[leading_secs]);
        };
        assert_eq!(window_length(SampleRate::must(11_025), 0.3), 3_308);
        check_tone(1.0, 4, 0.5);
        // libebur128 shares the 48 kHz table exactly but drifts from it at other rates.
        check_tone(0.0, 2, 0.5);
        // About -64 LUFS, so the absolute gate rather than the relative one drops the fade-in.
        check_tone(1.0, 2, 0.000_6);

        let rate = RATE.get() as usize;
        let gains = [0.0625, 0.03125, 0.015625, 0.5, 0.125, -0.0625, 0.25, -0.125];
        let frequencies = [83.0, 137.0, 701.0, 37.0, 997.0, 1_523.0, 503.0, 7_901.0];
        let signal: Vec<[f32; 8]> = (0..rate * 6 + rate * 2 / 5)
            .map(|frame| {
                std::array::from_fn(|channel| {
                    let active = match frame / rate {
                        0 => frame >= rate / 4 && matches!(channel, 4 | 5),
                        1 => matches!(channel, 6 | 7),
                        2 => true,
                        3..=5 => channel == 3,
                        _ => matches!(channel, 4 | 5),
                    };
                    if !active {
                        return 0.0;
                    }
                    let phase = 2.0 * PI * frequencies[channel] * frame as f64 / rate as f64;
                    phase.sin() as f32 * gains[channel]
                })
            })
            .collect();
        for order in [[0, 1, 2, 3, 4, 5, 6, 7], [7, 4, 2, 0, 6, 5, 1, 3]] {
            let samples: Vec<_> = signal
                .iter()
                .flat_map(|frame| order.map(|channel| frame[channel]))
                .collect();
            let positions = order.map(|channel| ChannelPosition::SURROUND[channel]);
            assert_loudness_matches_ebur128(
                &samples,
                AudioFormat::new(8, RATE, 0, positions).unwrap(),
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
            let mut layout: Vec<_> = ChannelPosition::fallback(channels)
                .into_iter()
                .zip(weights)
                .collect();
            // Rotations in both directions put each role in every slot without exhaustive permutations.
            for _ in 0..2 {
                for _ in 0..channels {
                    // Inactive entries must not turn a 5.1 layout into 7.1.
                    let positions = std::array::from_fn(|index| {
                        layout
                            .get(index)
                            .map_or(ChannelPosition::SideLeft, |entry| entry.0)
                    });
                    let format = AudioFormat::new(channels, RATE, 0, positions).unwrap();
                    let block = AudioBlock::new(&[], format);
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
            peaks.push(std::mem::take(&mut meter.peak));
        }
        peaks
    }

    #[test]
    fn true_peak_batches_agree_through_wraps_silence_and_extreme_levels() {
        let mut seed = 17_u32;
        let mut samples: Vec<f32> = (0..4096)
            .map(|i| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                if i % 131 < 65 {
                    0.0
                } else {
                    (seed as i32 as f32) / i32::MAX as f32
                }
            })
            .collect();
        for value in [
            0.0,
            -0.0,
            1.0,
            -1.0,
            1.0e-22,
            -1.0e-22,
            1.0e-35,
            -1.0e-35,
            1.0e12,
            -1.0e12,
            f32::MAX,
        ] {
            samples.extend(std::iter::repeat_n(value, TRUE_PEAK_TAPS * 2));
            samples.extend(std::iter::repeat_n(0.0, TRUE_PEAK_TAPS * 2));
        }
        let expected = measure_true_peak(&samples, 1);
        assert!(
            expected[expected.len() - TRUE_PEAK_TAPS..]
                .iter()
                .all(|peak| *peak == 0.0)
        );
        for frames in [
            1,
            2,
            7,
            31,
            TRUE_PEAK_BATCH_FRAMES - 1,
            TRUE_PEAK_BATCH_FRAMES,
            TRUE_PEAK_BATCH_FRAMES + 1,
            TRUE_PEAK_BATCH_FRAMES * 2 + 1,
            TRUE_PEAK_BATCH_FRAMES * 16,
            samples.len(),
        ] {
            let actual = measure_true_peak(&samples, frames);
            assert_eq!(actual.len(), samples.len().div_ceil(frames));
            for (i, (&actual, expected)) in actual.iter().zip(expected.chunks(frames)).enumerate() {
                let expected = expected.iter().copied().fold(0.0_f32, f32::max);
                let sample_peak = samples[i * frames..((i + 1) * frames).min(samples.len())]
                    .iter()
                    .copied()
                    .map(f32::abs)
                    .fold(0.0_f32, f32::max);
                assert!(
                    !actual.is_nan() && actual >= sample_peak,
                    "sample peak lost: frames={frames}, chunk={i}"
                );
                assert!(
                    actual == expected
                        || (actual.is_finite()
                            && expected.is_finite()
                            && (actual - expected).abs()
                                <= 2.0e-6 * actual.max(expected) + 1.0e-37),
                    "frames={frames}, chunk={i}: {actual} vs {expected}"
                );
            }
        }
        let dc = measure_true_peak(&[1.0; TRUE_PEAK_TAPS * 3], 1);
        assert!(
            dc[TRUE_PEAK_TAPS..]
                .iter()
                .all(|peak| (*peak - 1.0).abs() < 1.0e-6)
        );
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
                let tones: Vec<_> = (0..12)
                    .map(|_| {
                        (
                            1 + (random() * (max_bin - 1) as f64) as usize,
                            0.2 + random() * 0.8,
                            random() * 2.0 * PI,
                        )
                    })
                    .collect();
                let samples: Vec<_> = (0..PERIOD * 2)
                    .map(|frame| {
                        tones
                            .iter()
                            .map(|&(bin, amplitude, phase)| {
                                amplitude
                                    * (2.0 * PI * bin as f64 * frame as f64 / PERIOD as f64 + phase)
                                        .sin()
                            })
                            .sum::<f64>() as f32
                    })
                    .collect();
                let mut spectrum = vec![Complex64::default(); PERIOD * OVERSAMPLE];
                for &(bin, amplitude, phase) in &tones {
                    let coefficient = Complex64::from_polar(amplitude * 0.5, phase - PI * 0.5);
                    spectrum[bin] += coefficient;
                    spectrum[PERIOD * OVERSAMPLE - bin] += coefficient.conj();
                }
                inverse.process(&mut spectrum);
                let reference = spectrum.iter().map(|x| x.re.abs()).fold(0.0_f64, f64::max);
                let coarser = spectrum
                    .iter()
                    .step_by(2)
                    .map(|x| x.re.abs())
                    .fold(0.0_f64, f64::max);
                assert!(
                    20.0 * (reference / coarser).log10() < 0.0002,
                    "oracle did not converge"
                );
                let peaks = measure_true_peak(&samples, 1);
                let peak = peaks[PERIOD..].iter().copied().fold(0.0_f32, f32::max);
                let error = 20.0 * (f64::from(peak) / reference).log10();
                assert!(error.abs() < 0.01, "{rate} Hz, case={case}: {error} dB");
            }
        }
    }

    #[test]
    fn true_peak_matches_analytic_phase_tones_at_standard_rates() {
        for rate in [44_100, 48_000, 96_000, 192_000] {
            let sample_rate = SampleRate::must(rate);
            let mono = AudioFormat::fallback(1, sample_rate);
            for divisor in [3.0, 4.0, 6.0, 8.0, 12.0, 16.0] {
                for phase in [0.0, PI / 32.0, PI / 8.0, PI / 4.0] {
                    let samples: Vec<_> = (-128..1024)
                        .map(|i| (2.0 * PI * f64::from(i) / divisor + phase).sin() as f32 * 1.41)
                        .collect();
                    let mut processor = LoudnessProcessor::new(LoudnessConfig { sample_rate });
                    processor.process_block(&AudioBlock::new(&samples[..128], mono));
                    let peak = processor
                        .process_block(&AudioBlock::new(&samples[128..], mono))
                        .0
                        .true_peak_db[0];
                    let expected = 20.0 * 1.41_f64.log10();
                    assert!(
                        (f64::from(peak) - expected).abs() < 0.005,
                        "{rate} Hz, divisor={divisor}, phase={phase}: {peak} vs {expected} dBTP"
                    );
                }
            }
        }
    }

    #[test]
    fn true_peak_preserves_channels_and_tails_across_blocks_and_resets() {
        for rate in [44_100, 48_000, 96_000, 192_000] {
            let sample_rate = SampleRate::must(rate);
            let wide = AudioFormat::fallback(MAX_CHANNELS, sample_rate);
            let gains = [2.0_f32, 0.0, -1.5, 1.0, 0.5, -0.125, 0.25, -0.75];
            let silence = [0.0; TRUE_PEAK_TAPS * 2 * MAX_CHANNELS];
            let mut processor = LoudnessProcessor::new(LoudnessConfig { sample_rate });
            // Put the two impulses in separate blocks, then measure only their delayed tail.
            for samples in [&gains[..], &[], &gains[..]] {
                processor.process_block(&AudioBlock::new(samples, wide));
            }
            let peak = processor
                .process_block(&AudioBlock::new(&silence, wide))
                .0
                .true_peak_db;
            for (channel, (&peak, gain)) in peak.iter().zip(gains).enumerate() {
                // sinc(t) + sinc(t - 1) peaks at 4/pi, including on the LFE channel.
                let expected = (20.0 * (f64::from(gain).abs() * 4.0 / PI).log10())
                    .max(f64::from(DEFAULT_FLOOR_DB));
                assert!(
                    (f64::from(peak) - expected).abs() < 0.02,
                    "{rate} Hz, channel={channel}: {peak} vs {expected} dBTP"
                );
            }
            assert_eq!(peak[1], DEFAULT_FLOOR_DB);

            assert_eq!(
                processor
                    .process_block(&AudioBlock::new(&silence, wide))
                    .0
                    .true_peak_db,
                [DEFAULT_FLOOR_DB; MAX_CHANNELS]
            );
            for change in 0..3 {
                processor.process_block(&AudioBlock::new(&[1.0; MAX_CHANNELS], wide));
                let format = match change {
                    0 => {
                        processor.reset_audio();
                        wide
                    }
                    1 => AudioFormat::fallback(MAX_CHANNELS, SampleRate::must(rate + 1)),
                    _ => AudioFormat::fallback(1, sample_rate),
                };
                assert_eq!(
                    processor
                        .process_block(&AudioBlock::new(&silence, format))
                        .0
                        .true_peak_db,
                    [DEFAULT_FLOOR_DB; MAX_CHANNELS]
                );
            }
        }
    }
}
