// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

// UI wrapper around the oscilloscope DSP processor and renderer.

use super::processor::{
    OscilloscopeConfig, OscilloscopeProcessor as CoreOscilloscopeProcessor, OscilloscopeSnapshot,
};
use super::render::{OscilloscopeParams, OscilloscopePrimitive};
use crate::persistence::settings::{ChannelMode, OscilloscopeSettings};
use crate::util::color;
use crate::visuals::palettes;
use crate::visuals::project_channel_data;
use crate::{vis_processor, visualization_widget};
use iced::Color;

const OSCILLOSCOPE_PALETTE_SIZE: usize = 1;
const MAX_PERSISTENCE: f32 = 0.98;
const FILL_ALPHA: f32 = 0.15;

vis_processor!(
    OscilloscopeProcessor,
    CoreOscilloscopeProcessor,
    OscilloscopeConfig,
    OscilloscopeSnapshot,
    sync_rate
);

#[derive(Debug, Clone)]
pub(crate) struct OscilloscopeState {
    snapshot: OscilloscopeSnapshot,
    style: OscilloscopeStyle,
    persistence: f32,
    channel_mode: ChannelMode,
    key: u64,
}

impl OscilloscopeState {
    pub fn new() -> Self {
        let defaults = OscilloscopeSettings::default();
        Self {
            snapshot: OscilloscopeSnapshot::default(),
            style: OscilloscopeStyle::default(),
            persistence: defaults.persistence,
            channel_mode: defaults.channel_mode,
            key: crate::visuals::next_key(),
        }
    }

    pub fn update_view_settings(&mut self, persistence: f32, channel_mode: ChannelMode) {
        self.persistence = persistence.clamp(0.0, 1.0);
        let mode_changed = self.channel_mode != channel_mode;
        self.channel_mode = channel_mode;
        if mode_changed {
            self.snapshot = Self::project_channels(&self.snapshot, channel_mode);
        }
    }

    pub fn set_palette(&mut self, palette: &[Color; OSCILLOSCOPE_PALETTE_SIZE]) {
        if !color::palettes_equal(&self.style.colors, palette) {
            self.style.colors = *palette;
        }
    }

    pub fn palette(&self) -> &[Color; OSCILLOSCOPE_PALETTE_SIZE] {
        &self.style.colors
    }

    pub fn apply_snapshot(&mut self, snapshot: &OscilloscopeSnapshot) {
        let projected = Self::project_channels(snapshot, self.channel_mode);

        if !projected.samples.is_empty()
            && !self.snapshot.samples.is_empty()
            && projected.samples.len() == self.snapshot.samples.len()
        {
            let persistence = self.persistence.clamp(0.0, MAX_PERSISTENCE);
            if persistence > f32::EPSILON {
                let fresh = 1.0 - persistence;
                for (current, incoming) in self.snapshot.samples.iter_mut().zip(&projected.samples)
                {
                    *current = *current * persistence + incoming * fresh;
                }
                return;
            }
        }

        self.snapshot = projected;
    }

    pub fn channel_mode(&self) -> ChannelMode {
        self.channel_mode
    }

    pub fn persistence(&self) -> f32 {
        self.persistence
    }

    fn project_channels(source: &OscilloscopeSnapshot, mode: ChannelMode) -> OscilloscopeSnapshot {
        let (ch, spc) = (source.channels.max(1), source.samples_per_channel);
        if spc == 0 || source.samples.len() < ch * spc {
            return OscilloscopeSnapshot::default();
        }
        OscilloscopeSnapshot {
            channels: mode.output_channels(ch),
            samples_per_channel: spc,
            samples: project_channel_data(mode, &source.samples, spc, ch),
        }
    }

    pub fn visual_params(&self, bounds: iced::Rectangle) -> Option<OscilloscopeParams> {
        let channels = self.snapshot.channels.max(1);
        let samples_per_channel = self.snapshot.samples_per_channel;
        let required = channels.saturating_mul(samples_per_channel);

        if samples_per_channel < 2 || self.snapshot.samples.len() < required {
            return None;
        }

        let colors = self
            .style
            .colors
            .iter()
            .cycle()
            .take(channels)
            .map(|c| color::color_to_rgba(*c))
            .collect();

        Some(OscilloscopeParams {
            key: self.key,
            bounds,
            channels,
            samples_per_channel,
            samples: self.snapshot.samples.clone(),
            colors,
            fill_alpha: FILL_ALPHA,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OscilloscopeStyle {
    pub colors: [Color; OSCILLOSCOPE_PALETTE_SIZE],
}

impl Default for OscilloscopeStyle {
    fn default() -> Self {
        Self {
            colors: palettes::oscilloscope::COLORS,
        }
    }
}

visualization_widget!(Oscilloscope, OscilloscopeState, OscilloscopePrimitive);
