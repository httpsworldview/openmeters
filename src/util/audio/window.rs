// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, LazyLock, PoisonError, RwLock},
};

crate::macros::choice_enum!(no_default all
    #[derive(Hash)]
    pub enum WindowKind {
        Rectangular => "Rectangular",
        Hann => "Hann",
        Hamming => "Hamming",
        Blackman => "Blackman",
        BlackmanHarris => "Blackman-Harris",
    }
);

impl WindowKind {
    fn coefficients(self, len: usize) -> Vec<f32> {
        if len <= 1 {
            return vec![1.0; len];
        }
        let coeffs: &[f32] = match self {
            Self::Rectangular => return vec![1.0; len],
            Self::Hann => &[0.5, -0.5],
            Self::Hamming => &[25.0 / 46.0, -21.0 / 46.0],
            Self::Blackman => &[0.42, -0.5, 0.08],
            Self::BlackmanHarris => &[0.35875, -0.48829, 0.14128, -0.01168],
        };
        let step = core::f32::consts::TAU / len as f32;
        (0..len)
            .map(|n| {
                let phi = n as f32 * step;
                coeffs
                    .iter()
                    .enumerate()
                    .fold(0.0, |sum, (k, &c)| sum + c * (phi * k as f32).cos())
            })
            .collect()
    }
}

type WindowCache = RwLock<HashMap<(WindowKind, usize), Arc<[f32]>>>;

pub(crate) fn window_coefficients(kind: WindowKind, len: usize) -> Arc<[f32]> {
    static CACHE: LazyLock<WindowCache> = LazyLock::new(Default::default);
    if len == 0 {
        return Arc::from([]);
    }
    let key = (kind, len);
    let cache = CACHE.read().unwrap_or_else(PoisonError::into_inner);
    if let Some(window) = cache.get(&key).cloned() {
        return window;
    }
    drop(cache);

    CACHE
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(key)
        .or_insert_with(|| Arc::from(kind.coefficients(len)))
        .clone()
}

pub fn apply_window(buffer: &mut [f32], window: &[f32]) {
    debug_assert_eq!(buffer.len(), window.len());
    for (sample, coeff) in buffer.iter_mut().zip(window.iter()) {
        *sample *= *coeff;
    }
}

/// Copies the front of `src` into `dst` and removes the copied window's DC offset.
pub fn copy_dc_removed_from_deque(dst: &mut [f32], src: &VecDeque<f32>) {
    if dst.is_empty() {
        return;
    }
    assert!(
        dst.len() <= src.len(),
        "destination longer than source deque"
    );
    let len = dst.len();
    let (head, tail) = src.as_slices();
    let split = head.len().min(len);
    dst[..split].copy_from_slice(&head[..split]);
    if split < len {
        dst[split..].copy_from_slice(&tail[..len - split]);
    }
    let mean = dst.iter().sum::<f32>() / len as f32;
    for sample in dst {
        *sample -= mean;
    }
}

pub fn compute_fft_bin_normalization(window: &[f32], fft_size: usize) -> Vec<f32> {
    let bins = fft_size / 2 + 1;
    let window_sum: f32 = window.iter().sum();
    let inv_sum = if window_sum.abs() > f32::EPSILON {
        1.0 / window_sum
    } else if fft_size > 0 {
        1.0 / fft_size as f32
    } else {
        0.0
    };

    let dc_scale = inv_sum * inv_sum;
    let ac_scale = 4.0 * dc_scale;
    let mut norms = vec![ac_scale; bins];
    norms[0] = dc_scale;
    if fft_size.is_multiple_of(2) && bins > 1 {
        norms[bins - 1] = dc_scale;
    }
    norms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_windows_are_periodic() {
        let hann = WindowKind::Hann.coefficients(8);

        assert_eq!(hann[0], 0.0);
        assert!((hann[4] - 1.0).abs() < 1.0e-6);
        assert!((hann[7] - 0.146_446_5).abs() < 1.0e-6);
    }
}
