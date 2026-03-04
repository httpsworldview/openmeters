// OpenMeters - an audio analysis and visualization tool
// Copyright (C) 2026  Maika Namuo
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

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
            channels,
            sample_rate,
            timestamp,
        }
    }

    pub fn now(samples: &'a [f32], channels: usize, sample_rate: f32) -> Self {
        Self {
            samples,
            channels,
            sample_rate,
            timestamp: Instant::now(),
        }
    }

    pub fn frame_count(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }
}

pub trait AudioProcessor {
    type Output;

    fn process_block(&mut self, block: &AudioBlock<'_>) -> Option<Self::Output>;
    fn reset(&mut self);
}

pub trait Reconfigurable<Cfg> {
    fn update_config(&mut self, config: Cfg);
}
