pub mod musical;

// Default sample rate (Hz) used throughout the audio pipeline.
// we automatically detect and apply actual rates, this exists
// mainly as a default during init and a fallback.
pub const DEFAULT_SAMPLE_RATE: f32 = 48_000.0;

// decibel conversion constants/utils

// Floor value (dB) below which magnitudes are clamped.
pub const DB_FLOOR: f32 = -140.0;

// Minimum power value to avoid log(0) in dB conversions.
const POWER_EPSILON: f32 = 1.0e-20;

// Natural log to decibel conversion factor: 10 / ln(10) ~= 4.342944819.
const LN_TO_DB: f32 = 4.342_944_8;

// Convert power (magnitude squared) to decibels with a custom floor.
#[inline(always)]
pub fn power_to_db(power: f32, floor: f32) -> f32 {
    if power > POWER_EPSILON {
        (power.ln() * LN_TO_DB).max(floor)
    } else {
        floor
    }
}

#[inline(always)]
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

// Convert dB to linear power: 10^(db/10).
#[inline(always)]
pub fn db_to_power(db: f32) -> f32 {
    const DB_TO_LOG2: f32 = 0.1 * core::f32::consts::LOG2_10;
    (db * DB_TO_LOG2).exp2()
}

// Convert frequency in Hz to mel scale.
#[inline(always)]
pub fn hz_to_mel(hz: f32) -> f32 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

// Convert mel scale to frequency in Hz.
#[inline(always)]
pub fn mel_to_hz(mel: f32) -> f32 {
    700.0 * (10.0f32.powf(mel / 2595.0) - 1.0)
}

// Copy from VecDeque to a contiguous slice, handling wraparound.
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
