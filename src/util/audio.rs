// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

pub mod musical;

mod channel;
mod format;
mod frequency;
mod level;
mod rate;
mod window;

pub use self::{
    channel::Channel,
    format::{fmt_duration, fmt_freq},
    frequency::FrequencyScale,
    level::{DB_FLOOR, LN_TO_DB, db_to_power, power_to_db, sanitize_negative_db},
    rate::{DEFAULT_SAMPLE_RATE, MAX_SAMPLE_RATE, sanitize_sample_rate},
    window::{WindowKind, compute_fft_bin_normalization, copy_dc_removed_windowed_from_deque},
};
pub(crate) use self::{
    level::{flush_denormal_f32, flush_denormal_f64},
    window::window_coefficients,
};

pub const BAND_SPLITS_HZ: [f32; 2] = [200.0, 2000.0];

#[cfg(test)]
pub fn sine_wave(frequency: f32, sample_rate: f32, count: usize, amplitude: f32) -> Vec<f32> {
    (0..count)
        .map(|i| (core::f32::consts::TAU * frequency * i as f32 / sample_rate).sin() * amplitude)
        .collect()
}
