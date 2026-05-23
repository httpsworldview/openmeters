// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{
    OscilloscopeConfig, OscilloscopeProcessor as CoreOscilloscopeProcessor, OscilloscopeSnapshot,
};
use super::render::{OscilloscopeParams, OscilloscopePrimitive};
use crate::persistence::settings::OscilloscopeSettings;
use crate::util::audio::{Channel, project_planar_channels};
use crate::util::color::color_to_rgba;
use crate::visuals::palettes;
use iced::Color;

const OSCILLOSCOPE_PALETTE_SIZE: usize = 1;
const MAX_PERSISTENCE: f32 = 0.98;
const FILL_ALPHA: f32 = 0.15;

crate::visuals::vis_processor!(
    OscilloscopeProcessor,
    CoreOscilloscopeProcessor,
    OscilloscopeConfig,
    OscilloscopeSnapshot,
    sync_rate
);

#[derive(Debug, Clone)]
pub(crate) struct OscilloscopeState {
    snapshot: OscilloscopeSnapshot,
    pub(crate) style: OscilloscopeStyle,
    pub(crate) persistence: f32,
    pub(crate) channel_1: Channel,
    pub(crate) channel_2: Channel,
    key: u64,
}

impl OscilloscopeState {
    pub fn new() -> Self {
        let defaults = OscilloscopeSettings::default();
        Self {
            snapshot: OscilloscopeSnapshot::default(),
            style: OscilloscopeStyle::default(),
            persistence: defaults.persistence,
            channel_1: defaults.channel_1,
            channel_2: defaults.channel_2,
            key: crate::visuals::next_key(),
        }
    }

    pub fn update_view_settings(
        &mut self,
        persistence: f32,
        channel_1: Channel,
        channel_2: Channel,
    ) {
        self.persistence = persistence.clamp(0.0, 1.0);
        let changed = self.channel_1 != channel_1 || self.channel_2 != channel_2;
        self.channel_1 = channel_1;
        self.channel_2 = channel_2;
        if changed {
            self.snapshot = Self::project_channels(&self.snapshot, channel_1, channel_2);
        }
    }

    pub fn set_palette(&mut self, palette: &[Color; OSCILLOSCOPE_PALETTE_SIZE]) {
        self.style.colors = *palette;
    }

    pub fn apply_snapshot(&mut self, snapshot: OscilloscopeSnapshot) {
        let projected = Self::project_channels(&snapshot, self.channel_1, self.channel_2);

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

    fn project_channels(
        source: &OscilloscopeSnapshot,
        ch1: Channel,
        ch2: Channel,
    ) -> OscilloscopeSnapshot {
        let (ch, spc) = (source.channels.max(1), source.samples_per_channel);
        if spc == 0 || source.samples.len() < ch * spc {
            return OscilloscopeSnapshot::default();
        }
        let samples = project_planar_channels([ch1, ch2], &source.samples, spc, ch);
        OscilloscopeSnapshot {
            channels: samples.len() / spc,
            samples_per_channel: spc,
            samples,
        }
    }

    pub fn visual_params(&self, bounds: iced::Rectangle) -> Option<OscilloscopeParams> {
        let channels = self.snapshot.channels;
        if channels == 0 {
            return None;
        }
        let samples_per_channel = self.snapshot.samples_per_channel;
        let required = channels.saturating_mul(samples_per_channel);

        if samples_per_channel < 2 || self.snapshot.samples.len() < required {
            return None;
        }

        Some(OscilloscopeParams {
            key: self.key,
            bounds,
            channels,
            samples_per_channel,
            samples: self.snapshot.samples.clone(),
            color: color_to_rgba(self.style.colors[0]),
            fill_alpha: FILL_ALPHA,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OscilloscopeStyle {
    pub(crate) colors: [Color; OSCILLOSCOPE_PALETTE_SIZE],
}

impl Default for OscilloscopeStyle {
    fn default() -> Self {
        Self {
            colors: palettes::oscilloscope::COLORS,
        }
    }
}

crate::visuals::visualization_widget!(Oscilloscope, OscilloscopeState, OscilloscopePrimitive);
