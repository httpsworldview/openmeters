// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

pub mod musical;

mod frequency;
mod level;
mod window;

pub use self::{
    frequency::FrequencyScale,
    level::{DB_FLOOR, LN_TO_DB, db_to_power, power_to_db, sanitize_negative_db},
    window::{WindowKind, compute_fft_bin_normalization, copy_dc_removed_windowed_from_deque},
};
pub(crate) use self::{
    level::{flush_denormal_f32, flush_denormal_f64},
    window::{mean_f32, window_coefficients},
};

pub const DEFAULT_SAMPLE_RATE: f32 = 48_000.0;
pub const MAX_SAMPLE_RATE: f32 = 768_000.0;
pub const MAX_DSP_BUFFER_LEN: usize = 1 << 20;
pub const BAND_SPLITS_HZ: [f32; 2] = [200.0, 2000.0];

crate::macros::choice_enum!(no_default pub enum Channel {
    Left => "Left",
    Right => "Right",
    Mid => "Mid",
    Side => "Side",
    None => "None",
});

impl Channel {
    pub(crate) fn project(self, [left, right]: [f32; 2]) -> f32 {
        match self {
            Self::Left => left,
            Self::Right => right,
            Self::Mid => (left + right) * 0.5,
            Self::Side => (left - right) * 0.5,
            Self::None => 0.0,
        }
    }
}

pub fn sanitize_sample_rate(sample_rate: f32) -> f32 {
    crate::util::finite_positive(sample_rate)
        .unwrap_or(DEFAULT_SAMPLE_RATE)
        .clamp(1.0, MAX_SAMPLE_RATE)
}

pub fn fmt_freq(frequency: f32) -> String {
    match frequency {
        f if f >= 10_000.0 => format!("{:.1}kHz", f / 1000.0),
        f if f >= 1_000.0 => format!("{:.2}kHz", f / 1000.0),
        f if f >= 100.0 => format!("{f:.1}Hz"),
        f => format!("{f:.2}Hz"),
    }
}

pub fn fmt_duration(seconds: f32) -> String {
    if seconds < 60.0 {
        return format!("{seconds:.2}s");
    }
    let seconds = seconds.round() as u64;
    format!("{}m {}s", seconds / 60, seconds % 60)
}

#[cfg(test)]
pub fn sine_wave(frequency: f32, sample_rate: f32, count: usize, amplitude: f32) -> Vec<f32> {
    (0..count)
        .map(|i| (core::f32::consts::TAU * frequency * i as f32 / sample_rate).sin() * amplitude)
        .collect()
}
