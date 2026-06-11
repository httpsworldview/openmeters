// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::util::audio::sanitize_sample_rate;
use std::time::Instant;

#[derive(Debug, Clone, Copy)]
pub struct AudioBlock<'a> {
    pub samples: &'a [f32],
    pub channels: usize,
    pub sample_rate: f32,
    pub timestamp: Instant,
}

impl<'a> AudioBlock<'a> {
    #[cfg(test)]
    pub fn new(samples: &'a [f32], channels: usize, sample_rate: f32, timestamp: Instant) -> Self {
        Self {
            samples,
            channels: channels.max(1),
            sample_rate: sanitize_sample_rate(sample_rate),
            timestamp,
        }
    }

    pub fn now(samples: &'a [f32], channels: usize, sample_rate: f32) -> Self {
        Self {
            samples,
            channels: channels.max(1),
            sample_rate: sanitize_sample_rate(sample_rate),
            timestamp: Instant::now(),
        }
    }

    pub fn frame_count(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }

    pub fn is_empty(&self) -> bool {
        self.frame_count() == 0
    }
}
