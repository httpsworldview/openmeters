// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use std::collections::VecDeque;

crate::macros::choice_enum!(pub enum WindowKind {
    Rectangular => "Rectangular",
    Hann => "Hann",
    Hamming => "Hamming",
    Blackman => "Blackman",
    BlackmanHarris => "Blackman-Harris",
});

impl WindowKind {
    // Periodic window: sum of terms[k] * cos(2 pi k n / len).
    pub(crate) const fn cosine_terms(self) -> &'static [f64] {
        match self {
            Self::Rectangular => &[1.0],
            Self::Hann => &[0.5, -0.5],
            Self::Hamming => &[25.0 / 46.0, -21.0 / 46.0],
            Self::Blackman => &[0.42, -0.5, 0.08],
            Self::BlackmanHarris => &[0.35875, -0.48829, 0.14128, -0.01168],
        }
    }

    // |W(offset) / W(0)| for a tone `offset` bins away, in the large-length limit.
    pub(crate) fn response(self, offset: f64) -> f64 {
        let (c0, terms) = self.cosine_terms().split_first().unwrap();
        let x = core::f64::consts::PI * offset;
        let sinc = if x == 0.0 { 1.0 } else { x.sin() / x };
        let d2 = offset * offset;
        let shape: f64 = (1..)
            .zip(terms)
            .map(|(k, term)| term / c0 * d2 / (d2 - f64::from(k * k)))
            .sum();
        (sinc * (1.0 + shape)).abs()
    }

    pub(crate) fn coefficients(self, len: usize) -> Vec<f32> {
        if len <= 1 {
            return vec![1.0; len];
        }
        let mut window = vec![0.0; len];
        for n in 0..=len / 2 {
            let phi = core::f64::consts::TAU * n as f64 / len as f64;
            let value = self
                .cosine_terms()
                .iter()
                .enumerate()
                .fold(0.0, |sum, (k, &c)| c.mul_add((phi * k as f64).cos(), sum))
                as f32;
            window[n] = value;
            window[(len - n) % len] = value;
        }
        window
    }
}

// Kaiser window at `z` in [-1, 1].
pub(crate) fn kaiser(beta: f64, z: f64) -> f64 {
    let bessel_i0 = |x: f64| {
        let (mut term, mut sum) = (1.0, 1.0);
        for k in 1..40 {
            term *= x * x / (4.0 * f64::from(k * k));
            sum += term;
        }
        sum
    };
    bessel_i0(beta * (1.0 - z * z).max(0.0).sqrt()) / bessel_i0(beta)
}

// Wider sums reduce DC-removal error.
pub(crate) fn mean_f32(samples: &[f32]) -> f32 {
    let (chunks, remainder) = samples.as_chunks::<4>();
    let mut sums = [0.0_f64; 4];
    for &[a, b, c, d] in chunks {
        sums[0] += f64::from(a);
        sums[1] += f64::from(b);
        sums[2] += f64::from(c);
        sums[3] += f64::from(d);
    }
    let sum = (sums[0] + sums[1])
        + (sums[2] + sums[3])
        + remainder
            .iter()
            .map(|&sample| f64::from(sample))
            .sum::<f64>();
    (sum / samples.len() as f64) as f32
}

// A window's own weighted mean zeroes its DC bin but also takes tones in its main lobe, so
// callers with a longer view of the stream pass their own estimate.
pub fn copy_dc_removed_windowed(dst: &mut [f32], src: &[f32], window: &[f32], dc: Option<f32>) {
    for ((sample, &source), &weight) in dst.iter_mut().zip(src).zip(window) {
        *sample = source * weight;
    }
    let mean = dc.unwrap_or_else(|| mean_f32(dst) / mean_f32(window));
    for (sample, &weight) in dst.iter_mut().zip(window) {
        *sample -= mean * weight;
    }
}

// Eight running sums let the compiler vectorize a sum it may not reorder. Offsets from a sample
// of the signal keep the mean of a constant signal exact.
fn weighted_offsets(weights: &[f32], samples: &[f32], center: f32) -> f32 {
    let (weights, weight_tail) = weights.as_chunks::<8>();
    let (samples, sample_tail) = samples.as_chunks::<8>();
    let mut sums = [0.0_f32; 8];
    for (weights, samples) in weights.iter().zip(samples) {
        sums = std::array::from_fn(|i| sums[i] + weights[i] * (samples[i] - center));
    }
    let tail = std::iter::zip(weight_tail, sample_tail).map(|(w, x)| w * (x - center));
    sums.into_iter().chain(tail).sum()
}

pub(crate) fn weighted_mean(weights: &[f32], samples: &[f32]) -> f32 {
    samples[0] + weighted_offsets(weights, samples, samples[0])
}

pub(crate) fn normalized_blackman_harris(len: usize) -> Vec<f32> {
    let weights = WindowKind::BlackmanHarris.coefficients(len);
    let sum: f64 = weights.iter().map(|&weight| f64::from(weight)).sum();
    weights
        .iter()
        .map(|&weight| (f64::from(weight) / sum) as f32)
        .collect()
}

// Up to three windows of samples a spectrum has moved past. A Blackman-Harris mean over four
// windows takes in only tones within a bin of DC.
#[derive(Debug, Default)]
pub struct DcHistory {
    samples: VecDeque<f32>,
    weights: Vec<f32>,
}

impl DcHistory {
    pub fn new(window_len: usize) -> Self {
        Self {
            samples: VecDeque::new(),
            weights: normalized_blackman_harris(4 * window_len),
        }
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }

    pub fn extend(&mut self, samples: impl IntoIterator<Item = f32>) {
        self.samples.extend(samples);
        let excess = self
            .samples
            .len()
            .saturating_sub(self.weights.len() / 4 * 3);
        self.samples.drain(..excess);
    }

    // None leaves the window's own mean: until the history is full, and in a silent window, so
    // an earlier offset never shows there.
    pub fn dc(&mut self, window: &[f32]) -> Option<f32> {
        if window.iter().all(|&sample| sample == 0.0) {
            return None;
        }
        let history = &*self.samples.make_contiguous();
        let (past, current) = self.weights.split_at(history.len());
        let center = *history.first()?;
        (current.len() == window.len()).then(|| {
            let past = weighted_offsets(past, history, center);
            center + past + weighted_offsets(current, window, center)
        })
    }
}

pub fn compute_fft_bin_normalization(window: &[f32], fft_size: usize) -> Vec<f32> {
    let bins = fft_size / 2 + 1;
    let sum: f64 = window.iter().map(|&weight| f64::from(weight)).sum();
    let dc_scale = sum.recip().powi(2) as f32;
    let mut norms = vec![4.0 * dc_scale; bins];
    norms[0] = dc_scale;
    if fft_size.is_multiple_of(2) {
        norms[bins - 1] = dc_scale;
    }
    norms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_window_math_is_precise_and_periodic() {
        let samples = vec![0.1; 16_384];
        let mut output = vec![0.0; samples.len()];
        copy_dc_removed_windowed(&mut output, &samples, &vec![1.0; samples.len()], None);
        assert!(output.iter().all(|&sample| sample == 0.0));
        let hann = WindowKind::Hann.coefficients(8);
        assert_eq!(hann[1], hann[7]);
        assert_eq!(hann[0], 0.0);
        assert!((hann[4] - 1.0).abs() < 1.0e-6);
        assert!((hann[7] - 0.146_446_5).abs() < 1.0e-6);
    }
}
