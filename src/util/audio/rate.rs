// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::util::finite_positive;

pub const DEFAULT_SAMPLE_RATE: f32 = 48_000.0;

pub fn sanitize_sample_rate(sample_rate: f32) -> f32 {
    finite_positive(sample_rate).unwrap_or(DEFAULT_SAMPLE_RATE)
}
