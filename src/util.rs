// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo
pub mod audio;
pub mod color;

use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Coalesces wakes: at most one is pending, and receivers compete for it.
#[derive(Clone)]
pub struct Notifier(Arc<(async_channel::Sender<()>, async_channel::Receiver<()>)>);

impl Default for Notifier {
    fn default() -> Self {
        Self(Arc::new(async_channel::bounded(1)))
    }
}

// By identity, so clones key the same subscription.
impl Hash for Notifier {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}

impl Notifier {
    pub fn notify(&self) {
        let _ = self.0.0.try_send(());
    }

    pub fn receiver(&self) -> async_channel::Receiver<()> {
        self.0.1.clone()
    }
}

crate::macros::bounded! {
    pub struct Finite(f32) in f32::MIN..=f32::MAX;
}

pub fn finite_or(value: f32, default: f32) -> f32 {
    if value.is_finite() { value } else { default }
}

pub fn finite_positive(value: f32) -> Option<f32> {
    (value.is_finite() && value > 0.0).then_some(value)
}

pub(crate) fn set_if_changed<T: PartialEq>(target: &mut T, value: T) -> bool {
    if *target == value {
        return false;
    }
    *target = value;
    true
}

pub(crate) fn unpoison<T>(lock: std::sync::LockResult<T>) -> T {
    lock.unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
