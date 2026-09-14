// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::util::audio::{Channel, flush_denormal_f64, sanitize_sample_rate};

pub const MAX_AUDIO_CHANNELS: usize = 8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ChannelPosition {
    FrontLeft,
    FrontRight,
    FrontCenter,
    LowFrequency,
    RearLeft,
    RearRight,
    SideLeft,
    SideRight,
    Mono,
    Aux(u8),
    #[default]
    Unknown,
}

impl ChannelPosition {
    pub const SURROUND: [Self; MAX_AUDIO_CHANNELS] = [
        Self::FrontLeft,
        Self::FrontRight,
        Self::FrontCenter,
        Self::LowFrequency,
        Self::RearLeft,
        Self::RearRight,
        Self::SideLeft,
        Self::SideRight,
    ];

    pub(crate) fn fallback(channels: usize) -> [Self; MAX_AUDIO_CHANNELS] {
        let channels = channels.min(MAX_AUDIO_CHANNELS);
        let mut positions = [Self::Unknown; MAX_AUDIO_CHANNELS];
        positions[..channels].copy_from_slice(&Self::SURROUND[..channels]);
        match channels {
            1 => positions[0] = Self::Mono,
            4 => positions[2..4].copy_from_slice(&[Self::RearLeft, Self::RearRight]),
            5 => positions[3..5].copy_from_slice(&[Self::RearLeft, Self::RearRight]),
            _ => {}
        }
        positions
    }

    pub(crate) fn normalize(
        channels: usize,
        mut positions: [Self; MAX_AUDIO_CHANNELS],
    ) -> [Self; MAX_AUDIO_CHANNELS] {
        let channels = channels.min(MAX_AUDIO_CHANNELS);
        positions[channels..].fill(Self::Unknown);
        for index in 0..channels {
            if positions[..index].contains(&positions[index]) {
                positions[index] = Self::Unknown;
            }
        }

        let fallback = Self::fallback(channels);
        for index in 0..channels {
            if positions[index] != Self::Unknown {
                continue;
            }
            positions[index] = std::iter::once(fallback[index])
                .chain(fallback)
                .find(|candidate| {
                    *candidate != Self::Unknown && !positions[..channels].contains(candidate)
                })
                .expect("channel fallback must have an unused position");
        }
        positions
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioFormat {
    pub channels: usize,
    pub sample_rate: f32,
    pub generation: u64,
    pub positions: [ChannelPosition; MAX_AUDIO_CHANNELS],
}

impl AudioFormat {
    pub(crate) fn new(
        channels: usize,
        sample_rate: u32,
        generation: u64,
        positions: [ChannelPosition; MAX_AUDIO_CHANNELS],
    ) -> Self {
        let channels = channels.clamp(1, MAX_AUDIO_CHANNELS);
        Self {
            channels,
            sample_rate: sample_rate.max(1) as f32,
            generation,
            positions: ChannelPosition::normalize(channels, positions),
        }
    }

    pub(crate) fn rate(self) -> u64 {
        self.sample_rate.round() as u64
    }
}

pub struct AudioBlock<'a> {
    pub samples: &'a [f32],
    pub channels: usize,
    pub sample_rate: f32,
    pub positions: [ChannelPosition; MAX_AUDIO_CHANNELS],
    stereo: [[f32; 2]; MAX_AUDIO_CHANNELS],
    pub(crate) stereo_channels: usize,
}

fn stereo_indices(channels: usize, positions: [ChannelPosition; MAX_AUDIO_CHANNELS]) -> [usize; 2] {
    let find = |position| {
        positions[..channels]
            .iter()
            .position(|candidate| *candidate == position)
    };
    let explicit_right = find(ChannelPosition::FrontRight);
    let left = find(ChannelPosition::FrontLeft)
        .or_else(|| find(ChannelPosition::Mono))
        .or_else(|| (0..channels).find(|index| Some(*index) != explicit_right))
        .unwrap_or(0);
    let right = explicit_right
        .filter(|index| *index != left)
        .or_else(|| (0..channels).find(|index| *index != left))
        .unwrap_or(left);
    [left, right]
}

fn stereo_matrix(
    channels: usize,
    positions: [ChannelPosition; MAX_AUDIO_CHANNELS],
) -> [[f32; 2]; MAX_AUDIO_CHANNELS] {
    let surround = std::f32::consts::FRAC_1_SQRT_2;
    let mut matrix = [[0.0; 2]; MAX_AUDIO_CHANNELS];
    for (weights, position) in matrix.iter_mut().zip(positions).take(channels) {
        *weights = match position {
            ChannelPosition::FrontLeft => [1.0, 0.0],
            ChannelPosition::FrontRight => [0.0, 1.0],
            ChannelPosition::FrontCenter => [surround; 2],
            ChannelPosition::RearLeft | ChannelPosition::SideLeft => [surround, 0.0],
            ChannelPosition::RearRight | ChannelPosition::SideRight => [0.0, surround],
            ChannelPosition::Mono => [1.0; 2],
            ChannelPosition::LowFrequency | ChannelPosition::Aux(_) | ChannelPosition::Unknown => {
                [0.0; 2]
            }
        };
    }

    let populated = |side| {
        matrix[..channels]
            .iter()
            .any(|weights| weights[side] != 0.0)
    };
    match (populated(0), populated(1)) {
        (false, false) => {
            let [left, right] = stereo_indices(channels, positions);
            matrix[left][0] = 1.0;
            matrix[right][1] = 1.0;
        }
        (false, true) => matrix
            .iter_mut()
            .for_each(|weights| weights[0] = weights[1]),
        (true, false) => matrix
            .iter_mut()
            .for_each(|weights| weights[1] = weights[0]),
        (true, true) => {}
    }
    matrix
}

impl<'a> AudioBlock<'a> {
    #[cfg(test)]
    pub fn new(samples: &'a [f32], channels: usize, sample_rate: f32) -> Self {
        Self::with_positions(
            samples,
            channels,
            sample_rate,
            ChannelPosition::fallback(channels),
        )
    }

    pub fn with_positions(
        samples: &'a [f32],
        channels: usize,
        sample_rate: f32,
        positions: [ChannelPosition; MAX_AUDIO_CHANNELS],
    ) -> Self {
        let channels = channels.clamp(1, MAX_AUDIO_CHANNELS);
        let stereo_channels = (2..channels.min(samples.len()))
            .rfind(|&channel| {
                samples[channel..]
                    .iter()
                    .step_by(channels)
                    .any(|sample| sample.to_bits() != 0)
            })
            .map_or(channels.min(2), |channel| channel + 1);
        Self {
            samples,
            channels,
            sample_rate: sanitize_sample_rate(sample_rate),
            positions,
            stereo: stereo_matrix(channels, positions),
            stereo_channels,
        }
    }

    fn stereo_matrix(&self) -> &[[f32; 2]] {
        &self.stereo[..self.stereo_channels]
    }

    pub fn frame_count(&self) -> usize {
        self.samples.len() / self.channels
    }

    pub fn stereo_frames(
        &self,
    ) -> impl ExactSizeIterator<Item = [f32; 2]> + DoubleEndedIterator + '_ {
        let matrix = self.stereo_matrix();
        self.samples
            .chunks_exact(self.channels)
            .map(move |frame| match matrix {
                [weights] => {
                    let sample = frame[0];
                    [0.0 + sample * weights[0], 0.0 + sample * weights[1]]
                }
                [first, second] => {
                    let sample = frame[0];
                    let [left, right] = [0.0 + sample * first[0], 0.0 + sample * first[1]];
                    let sample = frame[1];
                    [left + sample * second[0], right + sample * second[1]]
                }
                _ => {
                    frame
                        .iter()
                        .zip(matrix)
                        .fold([0.0; 2], |[left, right], (&sample, weights)| {
                            [left + sample * weights[0], right + sample * weights[1]]
                        })
                }
            })
    }

    pub fn projected_frames(
        &self,
        channel: Channel,
    ) -> impl ExactSizeIterator<Item = f32> + DoubleEndedIterator + '_ {
        self.stereo_frames()
            .map(move |stereo| channel.project(stereo))
    }
}

pub struct RunningMeans<const VALUES: usize, const WINDOWS: usize> {
    ring: Ring<VALUES>,
    capacities: [usize; WINDOWS],
    blocks: Box<[[f64; VALUES]]>,
    partial: [f64; VALUES],
    len: usize,
    head: usize,
    count: usize,
}

enum Ring<const VALUES: usize> {
    Compact(Box<[[f32; VALUES]]>),
    Wide(Box<[[f64; VALUES]]>),
}

impl<const VALUES: usize, const WINDOWS: usize> RunningMeans<VALUES, WINDOWS> {
    const BLOCK: usize = 16;
    const ZERO: [f64; VALUES] = [0.0; VALUES];

    pub fn new(capacities: [usize; WINDOWS]) -> Self {
        Self::seeded(capacities, 0)
    }

    // Count leading silence without pushing individual zero samples.
    pub fn seeded(capacities: [usize; WINDOWS], count: usize) -> Self {
        let capacities = capacities.map(|capacity| capacity.max(1));
        let len = capacities.iter().copied().max().unwrap_or(1);
        Self {
            ring: Ring::Compact(vec![[0.0; VALUES]; len].into_boxed_slice()),
            blocks: vec![Self::ZERO; len.div_ceil(Self::BLOCK) * 2].into_boxed_slice(),
            capacities,
            partial: Self::ZERO,
            len,
            head: count % len,
            count: count.min(len),
        }
    }

    #[inline]
    pub fn push_nonnegative_finite(&mut self, values: [f32; VALUES]) {
        debug_assert!(
            values
                .iter()
                .all(|value| value.is_finite() && *value >= 0.0)
        );
        match &mut self.ring {
            Ring::Compact(buffer) => {
                let slot = &mut buffer[self.head];
                *slot = values;
                for (partial, &sample) in self.partial.iter_mut().zip(slot.iter()) {
                    *partial += f64::from(sample);
                }
            }
            Ring::Wide(buffer) => {
                let wide = values.map(f64::from);
                buffer[self.head] = wide;
                for (partial, &sample) in self.partial.iter_mut().zip(&wide) {
                    *partial += sample;
                }
            }
        }
        self.flush();
    }

    #[inline]
    pub fn push_nonnegative_finite_wide(&mut self, values: [f64; VALUES]) {
        debug_assert!(
            values
                .iter()
                .all(|value| value.is_finite() && *value >= 0.0)
        );
        // Preserve compact history until a sample would overflow binary32.
        if let Ring::Compact(buffer) = &self.ring
            && values.iter().any(|&value| !(value as f32).is_finite())
        {
            self.ring = Ring::Wide(buffer.iter().map(|slot| slot.map(f64::from)).collect());
        }
        match &mut self.ring {
            Ring::Compact(buffer) => {
                let slot = &mut buffer[self.head];
                *slot = values.map(|value| value as f32);
                for (partial, &sample) in self.partial.iter_mut().zip(slot.iter()) {
                    *partial += f64::from(sample);
                }
            }
            Ring::Wide(buffer) => {
                buffer[self.head] = values;
                for (partial, &sample) in self.partial.iter_mut().zip(&values) {
                    *partial += sample;
                }
            }
        }
        self.flush();
    }

    #[inline]
    fn flush(&mut self) {
        self.head += 1;
        if self.head.is_multiple_of(Self::BLOCK) || self.head == self.len {
            let mut node = self.blocks.len() / 2 + (self.head - 1) / Self::BLOCK;
            self.blocks[node] = self.partial;
            self.partial = Self::ZERO;
            while node > 1 {
                node /= 2;
                let left = self.blocks[node * 2];
                let right = self.blocks[node * 2 + 1];
                self.blocks[node] = std::array::from_fn(|i| left[i] + right[i]);
            }
        }
        if self.head == self.len {
            self.head = 0;
        }
        self.count = (self.count + 1).min(self.len);
    }

    #[inline]
    pub fn mean(&self, window: usize) -> [f64; VALUES] {
        let count = self.count.min(self.capacities[window]).max(1);
        // Nonnegative samples make this a valid silence shortcut even when
        // the tree still holds the previous contents of the partial block.
        if self.blocks[1] == Self::ZERO && self.partial == Self::ZERO {
            return Self::ZERO;
        }
        let len = self.len;
        let start = (self.head + len - count) % len;
        let end = start + count;
        let mut sum = Self::ZERO;
        for range in [start..end.min(len), 0..end.saturating_sub(len)] {
            let first = range.start.next_multiple_of(Self::BLOCK).min(range.end);
            let last = (range.end / Self::BLOCK * Self::BLOCK).max(first);
            for index in (range.start..first).chain(last..range.end) {
                let samples = match &self.ring {
                    Ring::Compact(buffer) => buffer[index].map(f64::from),
                    Ring::Wide(buffer) => buffer[index],
                };
                for (sum, &sample) in sum.iter_mut().zip(&samples) {
                    *sum += sample;
                }
            }
            let mut left = self.blocks.len() / 2 + first / Self::BLOCK;
            let mut right = self.blocks.len() / 2 + last / Self::BLOCK;
            while left < right {
                if left % 2 == 1 {
                    for (sum, &part) in sum.iter_mut().zip(&self.blocks[left]) {
                        *sum += part;
                    }
                    left += 1;
                }
                if right % 2 == 1 {
                    right -= 1;
                    for (sum, &part) in sum.iter_mut().zip(&self.blocks[right]) {
                        *sum += part;
                    }
                }
                left /= 2;
                right /= 2;
            }
        }
        let count = count as f64;
        sum.map(|value| value / count)
    }
}

#[derive(Debug, Clone, Copy)]
struct Biquad<const CHANNELS: usize> {
    b: [f64; 3],
    a: [f64; 2],
    z: [[f64; CHANNELS]; 2],
}

impl<const CHANNELS: usize> Biquad<CHANNELS> {
    fn low_high(sample_rate: f32, frequency: f32) -> [Self; 2] {
        let ratio = (f64::from(frequency) / f64::from(sample_rate)).clamp(1.0e-6, 0.49);
        let (half_sin, half_cos) = (core::f64::consts::PI * ratio).sin_cos();
        let sin = 2.0 * half_sin * half_cos;
        let cos = half_cos.mul_add(half_cos, -half_sin * half_sin);
        let alpha = sin * core::f64::consts::FRAC_1_SQRT_2;
        let inv_a0 = 1.0 / (1.0 + alpha);
        [
            (2.0 * half_sin * half_sin, 1.0),
            (2.0 * half_cos * half_cos, -1.0),
        ]
        .map(|(gain, sign)| Self {
            b: [
                gain * 0.5 * inv_a0,
                gain * inv_a0 * sign,
                gain * 0.5 * inv_a0,
            ],
            a: [-2.0 * cos * inv_a0, (1.0 - alpha) * inv_a0],
            z: [[0.0; CHANNELS]; 2],
        })
    }

    #[inline(always)]
    fn process(&mut self, sample: [f32; CHANNELS]) -> [f32; CHANNELS] {
        let sample = sample.map(f64::from);
        let (b, a) = (self.b, self.a);
        let [z0, z1] = self.z;
        let output: [f64; CHANNELS] = std::array::from_fn(|i| b[0] * sample[i] + z0[i]);
        let mut state = [
            std::array::from_fn(|i| b[1] * sample[i] - a[0] * output[i] + z1[i]),
            std::array::from_fn(|i| b[2] * sample[i] - a[1] * output[i]),
        ];
        let output: [f32; CHANNELS] = std::array::from_fn(|channel| {
            if output[channel].abs() <= f32::MAX as f64 {
                output[channel] as f32
            } else {
                state[0][channel] = 0.0;
                state[1][channel] = 0.0;
                0.0
            }
        });
        self.z = state;
        output
    }

    fn flush_denormals(&mut self) {
        self.z.iter_mut().flatten().for_each(flush_denormal_f64);
    }

    fn clear(&mut self) {
        self.z = [[0.0; CHANNELS]; 2];
    }
}

pub struct ThreeBand<const CHANNELS: usize, const STAGES: usize, const CASCADE_HIGH: bool> {
    filters: [[Biquad<CHANNELS>; STAGES]; 4],
}

impl<const CHANNELS: usize, const STAGES: usize, const CASCADE_HIGH: bool>
    ThreeBand<CHANNELS, STAGES, CASCADE_HIGH>
{
    pub fn new(sample_rate: f32, [low, high]: [f32; 2]) -> Self {
        let [low, above_low] = Biquad::low_high(sample_rate, low);
        let [mid, high] = Biquad::low_high(sample_rate, high);
        Self {
            filters: [low, above_low, mid, high].map(|filter| [filter; STAGES]),
        }
    }

    #[inline(always)]
    pub fn process(&mut self, sample: [f32; CHANNELS]) -> [[f32; CHANNELS]; 3] {
        let process = |stages: &mut [Biquad<CHANNELS>; STAGES], sample| {
            stages
                .iter_mut()
                .fold(sample, |sample, filter| filter.process(sample))
        };
        let [low, above_low, mid, high] = &mut self.filters;
        let low = process(low, sample);
        let above_low = process(above_low, sample);
        let high_input = if CASCADE_HIGH { above_low } else { sample };
        [low, process(mid, above_low), process(high, high_input)]
    }

    pub fn flush_denormals(&mut self) {
        self.filters
            .iter_mut()
            .flatten()
            .for_each(Biquad::flush_denormals);
    }

    pub fn clear(&mut self) {
        self.filters.iter_mut().flatten().for_each(Biquad::clear);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_layouts_fill_unknown_and_duplicate_positions_without_collisions() {
        use ChannelPosition::*;

        for (channels, expected) in [
            (1, &[Mono][..]),
            (4, &[FrontLeft, FrontRight, RearLeft, RearRight]),
            (
                6,
                &[
                    FrontLeft,
                    FrontRight,
                    FrontCenter,
                    LowFrequency,
                    RearLeft,
                    RearRight,
                ],
            ),
            (8, &ChannelPosition::SURROUND[..]),
        ] {
            let format = AudioFormat::new(
                channels,
                48_000,
                1,
                [ChannelPosition::Unknown; MAX_AUDIO_CHANNELS],
            );
            assert_eq!(&format.positions[..channels], expected);
        }

        let mut partial = [Unknown; MAX_AUDIO_CHANNELS];
        partial[..2].copy_from_slice(&[FrontRight, Unknown]);
        let format = AudioFormat::new(2, 48_000, 1, partial);
        assert_eq!(&format.positions[..2], &[FrontRight, FrontLeft]);

        partial[..3].copy_from_slice(&[FrontLeft, FrontLeft, FrontRight]);
        let format = AudioFormat::new(3, 48_000, 1, partial);
        assert_eq!(format.positions[0], FrontLeft);
        assert_eq!(format.positions[2], FrontRight);
        assert_eq!(
            format.positions[..3]
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3
        );
    }

    #[test]
    fn stereo_matrix_folds_semantic_channels_and_ignores_lfe() {
        use ChannelPosition::*;

        let samples = [1.0, 2.0, 3.0, 100.0, 4.0, 5.0, 6.0, 7.0];
        let block = AudioBlock::with_positions(
            &samples,
            samples.len(),
            48_000.0,
            ChannelPosition::SURROUND,
        );
        let mixed = block.stereo_frames().next().unwrap();
        let gain = std::f32::consts::FRAC_1_SQRT_2;
        assert_eq!(mixed, [1.0 + gain * 13.0, 2.0 + gain * 15.0]);

        let mono = AudioBlock::with_positions(
            &[0.25],
            1,
            48_000.0,
            [
                Mono, Unknown, Unknown, Unknown, Unknown, Unknown, Unknown, Unknown,
            ],
        );
        assert_eq!(mono.stereo_matrix()[0], [1.0; 2]);

        let mut unsupported = [Unknown; MAX_AUDIO_CHANNELS];
        unsupported[..2].copy_from_slice(&[LowFrequency, Aux(0)]);
        assert_eq!(
            AudioBlock::with_positions(&[], 8, 48_000.0, unsupported).stereo_matrix(),
            &[[1.0, 0.0], [0.0, 1.0]]
        );
    }

    #[test]
    fn common_stereo_paths_match_general_fold() {
        for (samples, channels) in [
            (
                &[0.0, -0.0, f32::from_bits(0x7fc0_1234), f32::INFINITY][..],
                1,
            ),
            (
                &[
                    0.0,
                    -0.0,
                    0.25,
                    -0.5,
                    f32::from_bits(0x7fc0_1234),
                    f32::INFINITY,
                ][..],
                2,
            ),
        ] {
            let block = AudioBlock::new(samples, channels, 48_000.0);
            let matrix = block.stereo_matrix();
            let expected = samples.chunks_exact(channels).map(|frame| {
                frame
                    .iter()
                    .zip(matrix)
                    .fold([0.0; 2], |[left, right], (&sample, weights)| {
                        [left + sample * weights[0], right + sample * weights[1]]
                    })
            });
            for (actual, expected) in block.stereo_frames().zip(expected) {
                for (actual, expected) in actual.into_iter().zip(expected) {
                    if expected.is_nan() {
                        assert!(actual.is_nan());
                    } else {
                        assert_eq!(actual.to_bits(), expected.to_bits());
                    }
                }
            }
        }
    }

    #[test]
    fn nonnegative_running_means_preserve_small_values_after_a_large_value_expires() {
        let mut means = RunningMeans::<3, 1>::new([2]);
        for value in [2.0_f32.powi(53), 1.0, 1.0] {
            means.push_nonnegative_finite([value, value * 2.0, value * 0.25]);
        }
        assert_eq!(means.mean(0), [1.0, 2.0, 0.25]);
    }

    #[test]
    fn windowed_means_match_direct_sums_across_ring_boundaries() {
        for len in [0_usize, 1, 2, 15, 16, 17, 31, 32, 33, 129] {
            let capacities = [len, len / 2, 1, len * 3 / 4];
            for leading in [0, len / 2, len, len + 7] {
                let mut means = RunningMeans::<3, 4>::seeded(capacities, leading);
                let capacities = capacities.map(|capacity| capacity.max(1));
                let len = len.max(1);
                let mut history =
                    std::collections::VecDeque::from(vec![[0.0_f32; 3]; leading.min(len)]);
                for window in 0..4 {
                    assert_eq!(means.mean(window), [0.0; 3]);
                }
                for step in 0..len * 4 + 33 {
                    // Dyadic values keep sums exact, including independent silent lanes.
                    let values = std::array::from_fn(|lane| {
                        if step > len * 3 || (step + lane) % 7 < 3 {
                            0.0
                        } else {
                            ((step * 37 + lane * 13) % 251) as f32 / 256.0
                        }
                    });
                    means.push_nonnegative_finite(values);
                    history.push_back(values);
                    if history.len() > len {
                        history.pop_front();
                    }
                    for (window, capacity) in capacities.into_iter().enumerate() {
                        let count = history.len().min(capacity);
                        let expected = history
                            .iter()
                            .rev()
                            .take(count)
                            .fold([0.0_f64; 3], |acc, values| {
                                std::array::from_fn(|i| acc[i] + f64::from(values[i]))
                            })
                            .map(|sum| sum / count as f64);
                        assert_eq!(
                            means.mean(window),
                            expected,
                            "len={len}, leading={leading}, step={step}, window={window}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn windowed_means_preserve_all_lanes_when_promoting_to_wide() {
        let mut means = RunningMeans::<3, 2>::new([33, 2]);
        for _ in 0..33 {
            means.push_nonnegative_finite([1.0, 2.0, 3.0]);
        }
        means.push_nonnegative_finite_wide([1.0, 2.0, 1.0e100]);
        assert_eq!(means.mean(0), [1.0, 2.0, 1.0e100 / 33.0]);
        assert_eq!(means.mean(1), [1.0, 2.0, 5.0e99]);
        for _ in 0..33 {
            means.push_nonnegative_finite([1.0, 2.0, 3.0]);
        }
        assert_eq!(means.mean(0), [1.0, 2.0, 3.0]);
        for _ in 0..33 {
            means.push_nonnegative_finite_wide([1.0e-100, 2.0e-100, 0.0]);
        }
        for window in 0..2 {
            let expected = [1.0e-100, 2.0e-100, 0.0];
            for (actual, expected) in means.mean(window).into_iter().zip(expected) {
                assert!((actual - expected).abs() <= expected * 1.0e-14);
            }
        }
    }

    #[test]
    fn stereo_biquad_matches_two_scalar_filters() {
        for kind in 0..2 {
            let mut scalar = [Biquad::<1>::low_high(48_000.0, 2_000.0)[kind]; 2];
            let mut stereo = Biquad::<2>::low_high(48_000.0, 2_000.0)[kind];
            for input in [
                [0.0, -0.0],
                [0.25, -0.5],
                [1.0, 0.75],
                [f32::INFINITY, f32::NAN],
                [-0.125, 0.5],
                [f32::INFINITY, 0.375],
                [0.125, f32::NAN],
                [f32::MAX, -f32::MAX],
                [0.0, 0.25],
            ] {
                let expected = [
                    scalar[0].process([input[0]])[0],
                    scalar[1].process([input[1]])[0],
                ];
                let actual = stereo.process(input);
                assert_eq!(actual.map(f32::to_bits), expected.map(f32::to_bits));
            }
        }
        assert!(std::mem::size_of::<Biquad<2>>() < std::mem::size_of::<[Biquad<1>; 2]>());
    }

    #[test]
    fn biquad_response_and_clear_are_precise() {
        use rustfft::num_complex::Complex64;
        let filter = Biquad::<1>::low_high(768_000.0, 200.0)[0];
        let z = Complex64::from_polar(1.0, -core::f64::consts::TAU * 200.0 / 768_000.0);
        let ([b0, b1, b2], [a1, a2]) = (filter.b, filter.a);
        let magnitude = ((b0 + b1 * z + b2 * z * z) / (1.0 + a1 * z + a2 * z * z)).norm();
        assert!((magnitude - core::f64::consts::FRAC_1_SQRT_2).abs() < 1.0e-9);
        let mut used = Biquad::<1>::low_high(48_000.0, 1_000.0)[0];
        let mut fresh = used;
        used.process([1.0]);
        used.clear();
        assert_eq!(used.process([0.25]), fresh.process([0.25]));
    }
}
