// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

pub mod musical;

use super::finite_positive;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, LazyLock, PoisonError, RwLock},
};

pub const DEFAULT_SAMPLE_RATE: f32 = 48_000.0;

pub fn sanitize_sample_rate(sample_rate: f32) -> f32 {
    finite_positive(sample_rate).unwrap_or(DEFAULT_SAMPLE_RATE)
}

pub fn sample_rates_differ(a: f32, b: f32) -> bool {
    (a - b).abs() > f32::EPSILON
}

pub fn sanitize_negative_db(db: f32, default: f32) -> f32 {
    if db.is_finite() && db < 0.0 {
        db
    } else {
        default
    }
}

pub const DB_FLOOR: f32 = -140.0;

pub const BAND_SPLITS_HZ: [f32; 2] = [250.0, 4000.0];

crate::macros::choice_enum!(all pub enum Channel {
    #[default]
    Left => "Left",
    Right => "Right",
    Mid => "Mid",
    Side => "Side",
    None => "None",
});

crate::macros::choice_enum!(all pub enum FrequencyScale {
    Linear => "Linear",
    #[default] Logarithmic => "Logarithmic",
    #[serde(alias = "mel")] Erb => "Erb",
});

// Mirrored in visuals/render/shaders/spectrogram.wgsl.
const LOG_KNEE_HZ: f32 = 20.0;

impl FrequencyScale {
    pub fn freq_at(self, min: f32, max: f32, t: f32) -> f32 {
        self.unscale(lerp(self.scale(min), self.scale(max), t))
    }

    pub fn pos_of(self, min: f32, max: f32, freq: f32) -> f32 {
        let (lo, hi) = (self.scale(min), self.scale(max));
        (self.scale(freq) - lo) / (hi - lo).max(1e-6)
    }

    fn scale(self, hz: f32) -> f32 {
        match self {
            Self::Linear => hz,
            Self::Logarithmic => (hz / LOG_KNEE_HZ).asinh(),
            Self::Erb => hz_to_erb_rate(hz),
        }
    }

    fn unscale(self, x: f32) -> f32 {
        match self {
            Self::Linear => x,
            Self::Logarithmic => LOG_KNEE_HZ * x.sinh(),
            Self::Erb => erb_rate_to_hz(x),
        }
    }
}

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
        let step = core::f32::consts::TAU / (len - 1) as f32;
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

const POWER_EPSILON: f32 = 1.0e-20;

pub const LN_TO_DB: f32 = 4.342_944_8;

pub fn power_to_db(power: f32, floor: f32) -> f32 {
    if power > POWER_EPSILON {
        (power.ln() * LN_TO_DB).max(floor)
    } else {
        floor
    }
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[inline]
fn project_interleaved_frame(frame: &[f32], channels: usize, channel: Channel) -> Option<f32> {
    if channels == 0 || channel == Channel::None {
        return None;
    }
    let len = channels.min(frame.len());
    let left = *frame.first()?;
    let right = if channels > 1 {
        frame.get(1).copied().unwrap_or(left)
    } else {
        left
    };
    match channel {
        Channel::Left => Some(left),
        Channel::Right => Some(right),
        Channel::Mid => Some(frame[..len].iter().sum::<f32>() / len as f32),
        Channel::Side => Some((left - right) * 0.5),
        Channel::None => None,
    }
}

pub(crate) fn project_interleaved_channel_into(
    output: &mut Vec<f32>,
    interleaved: &[f32],
    channels: usize,
    frames: usize,
    channel: Channel,
) -> bool {
    output.clear();
    if channels == 0 || channel == Channel::None {
        return false;
    }
    output.reserve(frames);
    for frame in interleaved.chunks_exact(channels).take(frames) {
        if let Some(sample) = project_interleaved_frame(frame, channels, channel) {
            output.push(sample);
        }
    }
    !output.is_empty()
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

pub fn db_to_power(db: f32) -> f32 {
    const DB_TO_LOG2: f32 = 0.1 * core::f32::consts::LOG2_10;
    (db * DB_TO_LOG2).exp2()
}

pub fn hz_to_erb_rate(hz: f32) -> f32 {
    21.4 * (1.0 + hz / 228.8).log10()
}

pub fn erb_rate_to_hz(erb: f32) -> f32 {
    228.8 * (10.0f32.powf(erb / 21.4) - 1.0)
}

// Maintains an interleaved rolling history, draining whole frames only.
pub fn extend_interleaved_history(
    history: &mut VecDeque<f32>,
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
        f if f >= 10_000.0 => format!("{:.1}kHz", f / 1000.0),
        f if f >= 1_000.0 => format!("{:.2}kHz", f / 1000.0),
        f if f >= 100.0 => format!("{f:.1}Hz"),
        _ => format!("{f:.2}Hz"),
    }
}

pub fn fmt_duration(secs: f32) -> String {
    if secs >= 60.0 {
        format!("{:.0}m {:.0}s", (secs / 60.0).floor(), secs % 60.0)
    } else {
        format!("{secs:.2}s")
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
