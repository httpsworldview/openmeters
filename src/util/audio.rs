// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

pub mod musical;

// Fallback rate during init; actual rate is detected from the audio stream.
pub const DEFAULT_SAMPLE_RATE: f32 = 48_000.0;

pub const DB_FLOOR: f32 = -140.0;

// Avoids log(0) in dB conversions.
const POWER_EPSILON: f32 = 1.0e-20;

// 10 / ln(10) ~= 4.342944819.
pub const LN_TO_DB: f32 = 4.342_944_8;

#[inline]
pub fn power_to_db(power: f32, floor: f32) -> f32 {
    if power > POWER_EPSILON {
        (power.ln() * LN_TO_DB).max(floor)
    } else {
        floor
    }
}

#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn mixdown_into_deque(
    buffer: &mut std::collections::VecDeque<f32>,
    samples: &[f32],
    channels: usize,
) {
    if channels == 0 || samples.is_empty() {
        return;
    }

    if channels == 1 {
        buffer.extend(samples);
        return;
    }

    let frame_count = samples.len() / channels;
    buffer.reserve(frame_count);

    let inv = 1.0 / channels as f32;
    for frame in samples.chunks_exact(channels) {
        let sum: f32 = frame.iter().sum();
        buffer.push_back(sum * inv);
    }
}

#[inline]
pub fn apply_window(buffer: &mut [f32], window: &[f32]) {
    debug_assert_eq!(buffer.len(), window.len());
    for (sample, coeff) in buffer.iter_mut().zip(window.iter()) {
        *sample *= *coeff;
    }
}

pub fn remove_dc(buffer: &mut [f32]) {
    if buffer.is_empty() {
        return;
    }

    let mean = buffer.iter().sum::<f32>() / buffer.len() as f32;
    if mean.abs() <= f32::EPSILON {
        return;
    }

    for sample in buffer.iter_mut() {
        *sample -= mean;
    }
}

#[inline]
pub fn db_to_power(db: f32) -> f32 {
    const DB_TO_LOG2: f32 = 0.1 * core::f32::consts::LOG2_10;
    (db * DB_TO_LOG2).exp2()
}

#[inline]
pub fn hz_to_erb_rate(hz: f32) -> f32 {
    21.4 * (1.0 + hz / 228.8).log10()
}

#[inline]
pub fn erb_rate_to_hz(erb: f32) -> f32 {
    228.8 * (10.0f32.powf(erb / 21.4) - 1.0)
}

#[inline]
pub fn copy_from_deque(dst: &mut [f32], src: &std::collections::VecDeque<f32>) {
    let len = dst.len().min(src.len());
    let (head, tail) = src.as_slices();
    if head.len() >= len {
        dst[..len].copy_from_slice(&head[..len]);
    } else {
        let split = head.len();
        dst[..split].copy_from_slice(head);
        dst[split..len].copy_from_slice(&tail[..len - split]);
    }
}

pub fn extend_interleaved_history(
    history: &mut std::collections::VecDeque<f32>,
    samples: &[f32],
    capacity: usize,
    channels: usize,
) {
    if capacity == 0 || channels == 0 {
        history.clear();
        return;
    }

    if samples.len() >= capacity {
        history.clear();
        history.extend(&samples[samples.len() - capacity..]);
        return;
    }

    let overflow = history.len() + samples.len();
    if overflow > capacity {
        let drain = (overflow - capacity).div_ceil(channels) * channels;
        history.drain(..drain.min(history.len()));
    }
    history.extend(samples);
}

pub fn fmt_freq(f: f32) -> String {
    match f {
        f if f >= 10_000.0 => format!("{:.0} kHz", f / 1000.0),
        f if f >= 1_000.0 => format!("{:.1} kHz", f / 1000.0),
        f if f >= 100.0 => format!("{f:.0} Hz"),
        f if f >= 10.0 => format!("{f:.1} Hz"),
        _ => format!("{f:.2} Hz"),
    }
}

pub fn compute_fft_bin_normalization(window: &[f32], fft_size: usize) -> Vec<f32> {
    let bins = fft_size / 2 + 1;
    if bins == 0 {
        return Vec::new();
    }

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
    if bins > 1 {
        norms[bins - 1] = dc_scale;
    }
    norms
}
