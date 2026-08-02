// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo
pub mod audio;
pub mod color;

pub fn finite_or(value: f32, default: f32) -> f32 {
    if value.is_finite() { value } else { default }
}

pub fn finite_positive(value: f32) -> Option<f32> {
    (value.is_finite() && value > 0.0).then_some(value)
}

pub(crate) fn unpoison<T>(lock: std::sync::LockResult<T>) -> T {
    lock.unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
