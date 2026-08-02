// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{OscilloscopeSnapshot, TRACE_COUNT};
use super::render::{OscilloscopeParams, OscilloscopePrimitive};
use crate::persistence::settings::OscilloscopeSettings;
use crate::util::color::color_to_rgba;
use crate::visuals::palettes;
use iced::Color;
use std::sync::Arc;

const MAX_PERSISTENCE: f32 = 0.98;

#[derive(Debug, Clone)]
pub(crate) struct OscilloscopeState {
    snapshot: OscilloscopeSnapshot,
    pub(in crate::visuals) palette: [Color; TRACE_COUNT],
    pub(in crate::visuals) settings: OscilloscopeSettings,
    key: u64,
    geometry_revision: u64,
}

impl OscilloscopeState {
    pub fn new() -> Self {
        Self {
            snapshot: OscilloscopeSnapshot::default(),
            palette: palettes::oscilloscope::COLORS,
            settings: OscilloscopeSettings::default(),
            key: crate::visuals::next_key(),
            geometry_revision: 0,
        }
    }

    pub fn reset_audio(&mut self) {
        self.snapshot = OscilloscopeSnapshot::default();
        self.geometry_revision = self.geometry_revision.wrapping_add(1);
    }

    pub fn update_view_settings(&mut self, settings: &OscilloscopeSettings, reset_snapshot: bool) {
        self.settings = settings.clone();
        self.settings.persistence = if settings.persistence.is_finite() {
            settings.persistence.clamp(0.0, 1.0)
        } else {
            OscilloscopeSettings::default().persistence
        };
        if reset_snapshot {
            self.snapshot = OscilloscopeSnapshot::default();
        }
        self.geometry_revision = self.geometry_revision.wrapping_add(1);
    }

    pub fn set_palette(&mut self, palette: &[Color; TRACE_COUNT]) {
        self.palette = *palette;
        self.geometry_revision = self.geometry_revision.wrapping_add(1);
    }

    pub fn apply_snapshot(&mut self, snapshot: OscilloscopeSnapshot) {
        self.geometry_revision = self.geometry_revision.wrapping_add(1);
        if !snapshot.samples.is_empty()
            && !self.snapshot.samples.is_empty()
            && snapshot.epoch == self.snapshot.epoch
            && snapshot.channels == self.snapshot.channels
            && snapshot.samples_per_channel == self.snapshot.samples_per_channel
            && snapshot.samples.len() == self.snapshot.samples.len()
            && snapshot.slots[..snapshot.channels] == self.snapshot.slots[..self.snapshot.channels]
        {
            let persistence = self.settings.persistence.clamp(0.0, MAX_PERSISTENCE);
            if persistence > f32::EPSILON {
                let fresh = 1.0 - persistence;
                for (current, incoming) in
                    Arc::make_mut(&mut self.snapshot.samples)
                        .iter_mut()
                        .zip(snapshot.samples.iter())
                {
                    *current = *current * persistence + incoming * fresh;
                }
                return;
            }
        }

        self.snapshot = snapshot;
    }

    pub fn visual_params(&self, bounds: iced::Rectangle) -> Option<OscilloscopeParams> {
        let channels = self.snapshot.channels;
        if channels == 0 { return None; }
        let samples_per_channel = self.snapshot.samples_per_channel;
        let required = channels.saturating_mul(samples_per_channel);

        if samples_per_channel < 2 || self.snapshot.samples.len() < required { return None; }

        Some(OscilloscopeParams {
            key: self.key,
            geometry_revision: self.geometry_revision,
            bounds,
            channels,
            samples_per_channel,
            slots: self.snapshot.slots,
            samples: self.snapshot.samples.clone(),
            colors: self.palette.map(color_to_rgba),
            stacked: self.settings.stacked,
        })
    }
}

crate::visuals::visualization_widget!(Oscilloscope, OscilloscopeState, OscilloscopePrimitive);
