// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::util::audio::{flush_denormal_f32, sanitize_sample_rate};

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
        let mut positions = [Self::Unknown; MAX_AUDIO_CHANNELS];
        match channels {
            1 => positions[0] = Self::Mono,
            4 => positions[..4].copy_from_slice(&[
                Self::FrontLeft,
                Self::FrontRight,
                Self::RearLeft,
                Self::RearRight,
            ]),
            5 => positions[..5].copy_from_slice(&[
                Self::FrontLeft,
                Self::FrontRight,
                Self::FrontCenter,
                Self::RearLeft,
                Self::RearRight,
            ]),
            _ => positions
                .iter_mut()
                .zip(Self::SURROUND)
                .take(channels)
                .for_each(|(position, fallback)| *position = fallback),
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
            if positions[index] == Self::Unknown || positions[..index].contains(&positions[index]) {
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
                .chain(Self::SURROUND)
                .chain((0..MAX_AUDIO_CHANNELS as u8).map(Self::Aux))
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
        self.sample_rate.round().max(1.0) as u64
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AudioBlock<'a> {
    pub samples: &'a [f32],
    pub channels: usize,
    pub sample_rate: f32,
    pub positions: [ChannelPosition; MAX_AUDIO_CHANNELS],
    stereo: [[f32; 2]; MAX_AUDIO_CHANNELS],
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

pub(crate) fn stereo_matrix(
    channels: usize,
    positions: [ChannelPosition; MAX_AUDIO_CHANNELS],
) -> [[f32; 2]; MAX_AUDIO_CHANNELS] {
    let channels = channels.clamp(1, MAX_AUDIO_CHANNELS);
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
        let channels = channels.clamp(1, MAX_AUDIO_CHANNELS);
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
        Self {
            samples,
            channels,
            sample_rate: sanitize_sample_rate(sample_rate),
            positions,
            stereo: stereo_matrix(channels, positions),
        }
    }

    pub fn stereo_matrix(&self) -> &[[f32; 2]; MAX_AUDIO_CHANNELS] {
        &self.stereo
    }

    pub fn frame_count(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }

    pub fn is_empty(&self) -> bool {
        self.frame_count() == 0
    }
}

/// Running means for several values over one or more independently sized windows.
/// All windows share the ring sized for the longest duration.
#[derive(Debug)]
pub struct WindowedMeans<T, const VALUES: usize, const WINDOWS: usize> {
    buffer: Box<[[T; VALUES]]>,
    capacities: [usize; WINDOWS],
    sums: [[f64; VALUES]; WINDOWS],
    head: usize,
    count: usize,
}

impl<T, const VALUES: usize, const WINDOWS: usize> WindowedMeans<T, VALUES, WINDOWS>
where
    T: Copy + Default + Into<f64>,
{
    pub fn new(capacities: [usize; WINDOWS]) -> Self {
        let capacities = capacities.map(|capacity| capacity.max(1));
        let len = capacities.iter().copied().max().unwrap_or(1);
        Self {
            buffer: vec![[T::default(); VALUES]; len].into_boxed_slice(),
            capacities,
            sums: [[0.0; VALUES]; WINDOWS],
            head: 0,
            count: 0,
        }
    }

    pub fn push(&mut self, values: [T; VALUES]) {
        let len = self.buffer.len();
        for (window, &capacity) in self.sums.iter_mut().zip(&self.capacities) {
            let old = (self.count >= capacity).then(|| {
                let index = if self.head >= capacity {
                    self.head - capacity
                } else {
                    self.head + len - capacity
                };
                &self.buffer[index]
            });
            for value in 0..VALUES {
                window[value] += values[value].into() - old.map_or(0.0, |old| old[value].into());
            }
        }
        self.advance(values);
    }

    fn advance(&mut self, values: [T; VALUES]) {
        let len = self.buffer.len();
        self.buffer[self.head] = values;
        self.head += 1;
        if self.head == len {
            self.head = 0;
        }
        self.count = (self.count + 1).min(len);
    }

    pub fn mean(&self, window: usize) -> [f64; VALUES] {
        let count = self.count.min(self.capacities[window]).max(1);
        self.sums[window].map(|sum| sum / count as f64)
    }

    pub fn clear(&mut self) {
        self.sums = [[0.0; VALUES]; WINDOWS];
        self.head = 0;
        self.count = 0;
    }
}

#[derive(Debug, Clone, Copy)]
pub enum FilterKind {
    LowPass,
    HighPass,
}

#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}

impl Biquad {
    pub fn new(kind: FilterKind, sample_rate: f32, frequency: f32) -> Self {
        let ratio = (frequency / sample_rate).clamp(1.0e-6, 0.49);
        let (sin, cos) = (core::f32::consts::TAU * ratio).sin_cos();
        let alpha = sin * core::f32::consts::FRAC_1_SQRT_2;
        let gain = match kind {
            FilterKind::LowPass => 1.0 - cos,
            FilterKind::HighPass => 1.0 + cos,
        };
        let inv_a0 = 1.0 / (1.0 + alpha);
        Self {
            b: [
                gain * 0.5 * inv_a0,
                gain * inv_a0
                    * if matches!(kind, FilterKind::HighPass) {
                        -1.0
                    } else {
                        1.0
                    },
                gain * 0.5 * inv_a0,
            ],
            a: [-2.0 * cos * inv_a0, (1.0 - alpha) * inv_a0],
            z: [0.0; 2],
        }
    }

    pub fn process(&mut self, sample: f32) -> f32 {
        let output = self.b[0].mul_add(sample, self.z[0]);
        self.z[0] = self.b[1] * sample - self.a[0] * output + self.z[1];
        self.z[1] = self.b[2] * sample - self.a[1] * output;
        if output.is_finite() {
            output
        } else {
            self.z = [0.0; 2];
            0.0
        }
    }

    pub fn flush_denormals(&mut self) {
        self.z.iter_mut().for_each(flush_denormal_f32);
    }

    pub fn clear(&mut self) {
        self.z = [0.0; 2];
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LinkwitzRiley([Biquad; 2]);

impl LinkwitzRiley {
    pub fn new(kind: FilterKind, sample_rate: f32, frequency: f32) -> Self {
        Self([Biquad::new(kind, sample_rate, frequency); 2])
    }

    pub fn process(&mut self, sample: f32) -> f32 {
        self.0
            .iter_mut()
            .fold(sample, |value, filter| filter.process(value))
    }

    pub fn flush_denormals(&mut self) {
        self.0.iter_mut().for_each(Biquad::flush_denormals);
    }

    pub fn clear(&mut self) {
        self.0.iter_mut().for_each(Biquad::clear);
    }
}

pub trait CrossoverFilter: Sized {
    type Sample: Copy;
    fn new(kind: FilterKind, sample_rate: f32, frequency: f32) -> Self;
    fn process(&mut self, sample: Self::Sample) -> Self::Sample;
    fn flush_denormals(&mut self);
    fn clear(&mut self);
}

impl CrossoverFilter for Biquad {
    type Sample = f32;
    fn new(kind: FilterKind, sample_rate: f32, frequency: f32) -> Self {
        Self::new(kind, sample_rate, frequency)
    }
    fn process(&mut self, sample: f32) -> f32 {
        self.process(sample)
    }
    fn flush_denormals(&mut self) {
        self.flush_denormals();
    }
    fn clear(&mut self) {
        self.clear();
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ThreeBand<F: CrossoverFilter> {
    low: F,
    above_low: F,
    mid: F,
    high: F,
    cascade_high: bool,
}

impl<F: CrossoverFilter> ThreeBand<F> {
    fn new(sample_rate: f32, [low, high]: [f32; 2], cascade_high: bool) -> Self {
        Self {
            low: F::new(FilterKind::LowPass, sample_rate, low),
            above_low: F::new(FilterKind::HighPass, sample_rate, low),
            mid: F::new(FilterKind::LowPass, sample_rate, high),
            high: F::new(FilterKind::HighPass, sample_rate, high),
            cascade_high,
        }
    }

    pub fn parallel(sample_rate: f32, splits: [f32; 2]) -> Self {
        Self::new(sample_rate, splits, false)
    }

    pub fn cascaded(sample_rate: f32, splits: [f32; 2]) -> Self {
        Self::new(sample_rate, splits, true)
    }

    pub fn process(&mut self, sample: F::Sample) -> [F::Sample; 3] {
        let low = self.low.process(sample);
        let above_low = self.above_low.process(sample);
        let high_input = if self.cascade_high { above_low } else { sample };
        [
            low,
            self.mid.process(above_low),
            self.high.process(high_input),
        ]
    }

    fn for_each_filter(&mut self, mut action: impl FnMut(&mut F)) {
        for filter in [
            &mut self.low,
            &mut self.above_low,
            &mut self.mid,
            &mut self.high,
        ] {
            action(filter);
        }
    }

    pub fn flush_denormals(&mut self) {
        self.for_each_filter(F::flush_denormals);
    }

    pub fn clear(&mut self) {
        self.for_each_filter(F::clear);
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
        let matrix = block.stereo_matrix();
        let mixed = samples
            .iter()
            .zip(matrix)
            .fold([0.0; 2], |mixed, (&sample, weights)| {
                [
                    mixed[0] + sample * weights[0],
                    mixed[1] + sample * weights[1],
                ]
            });
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
            &AudioBlock::with_positions(&[], 2, 48_000.0, unsupported).stereo_matrix()[..2],
            &[[1.0, 0.0], [0.0, 1.0]]
        );
    }

    #[test]
    fn running_means_clear_without_reallocating_or_replaying_old_values() {
        let mut means = WindowedMeans::<f32, 1, 2>::new([2, 4]);
        for value in [1.0, 2.0, 3.0, 4.0] {
            means.push([value]);
        }
        let storage = means.buffer.as_ptr();
        means.clear();
        assert_eq!(means.mean(0), [0.0]);
        assert_eq!(means.mean(1), [0.0]);
        assert_eq!(means.buffer.as_ptr(), storage);

        means.push([10.0]);
        means.push([20.0]);
        assert_eq!(means.mean(0), [15.0]);
        assert_eq!(means.mean(1), [15.0]);
    }

    #[test]
    fn biquad_clear_matches_fresh_filter_state() {
        let mut used = Biquad::new(FilterKind::LowPass, 48_000.0, 1_000.0);
        let mut fresh = used;
        used.process(1.0);
        used.clear();
        assert_eq!(used.process(0.25), fresh.process(0.25));
    }
}
