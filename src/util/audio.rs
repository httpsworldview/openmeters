// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

pub mod musical;

mod channel;
mod format;
mod frequency;
mod level;
mod rate;
mod window;

pub(crate) use self::{channel::project_interleaved_channel_into, window::window_coefficients};
pub use self::{
    channel::{Channel, extend_interleaved_history},
    format::{fmt_duration, fmt_freq},
    frequency::{FrequencyScale, hz_to_erb_rate},
    level::{DB_FLOOR, LN_TO_DB, db_to_power, power_to_db, sanitize_negative_db},
    rate::{DEFAULT_SAMPLE_RATE, sample_rates_differ, sanitize_sample_rate},
    window::{WindowKind, apply_window, compute_fft_bin_normalization, copy_dc_removed_from_deque},
};

pub const BAND_SPLITS_HZ: [f32; 2] = [250.0, 4000.0];
