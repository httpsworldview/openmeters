// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::util::audio::{flush_denormal_f32, sanitize_sample_rate};

#[derive(Debug, Clone, Copy)]
pub struct AudioBlock<'a> {
    pub samples: &'a [f32],
    pub channels: usize,
    pub sample_rate: f32,
}

impl<'a> AudioBlock<'a> {
    pub fn new(samples: &'a [f32], channels: usize, sample_rate: f32) -> Self {
        Self {
            samples,
            channels: channels.max(1),
            sample_rate: sanitize_sample_rate(sample_rate),
        }
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
}

pub trait CrossoverFilter: Sized {
    type Sample: Copy;
    fn new(kind: FilterKind, sample_rate: f32, frequency: f32) -> Self;
    fn process(&mut self, sample: Self::Sample) -> Self::Sample;
    fn flush_denormals(&mut self);
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

    pub fn flush_denormals(&mut self) {
        for filter in [
            &mut self.low,
            &mut self.above_low,
            &mut self.mid,
            &mut self.high,
        ] {
            filter.flush_denormals();
        }
    }
}
