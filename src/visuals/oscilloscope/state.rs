// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{OscilloscopeSnapshot, TRACE_COUNT};
use super::render::OscilloscopeParams;
use crate::persistence::settings::OscilloscopeSettings;
use crate::util::{audio::Channel, color::color_to_rgba};
use crate::visuals::palettes;
use iced::Color;
use std::sync::Arc;

const MAX_PERSISTENCE: f32 = 0.98;

pub(crate) struct OscilloscopeState {
    snapshot: OscilloscopeSnapshot,
    pub(in crate::visuals) palette: [Color; TRACE_COUNT],
    pub(in crate::visuals) settings: OscilloscopeSettings,
    geometry: crate::visuals::GeometryKey,
}

impl OscilloscopeState {
    pub fn new() -> Self {
        Self {
            snapshot: OscilloscopeSnapshot::default(),
            palette: palettes::oscilloscope::COLORS,
            settings: OscilloscopeSettings::default(),
            geometry: crate::visuals::GeometryKey::new(),
        }
    }

    pub fn reset_audio(&mut self) {
        self.snapshot = OscilloscopeSnapshot::default();
        self.geometry.invalidate();
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
        self.geometry.invalidate();
    }

    crate::visuals::palette_setter!(TRACE_COUNT => geometry);

    pub fn apply_snapshot(&mut self, snapshot: OscilloscopeSnapshot) {
        self.geometry.invalidate();
        if !self.snapshot.samples.is_empty()
            && snapshot.epoch == self.snapshot.epoch
            && snapshot.channels == self.snapshot.channels
            && snapshot.samples_per_channel == self.snapshot.samples_per_channel
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
                    crate::util::audio::flush_denormal_f32(current);
                }
                return;
            }
        }

        self.snapshot = snapshot;
    }

    pub(in crate::visuals) fn ignores_audio(&self) -> bool {
        [self.settings.channel_1, self.settings.channel_2] == [Channel::None; 2]
    }

    pub(in crate::visuals) fn is_quiescent(&self) -> bool {
        let channels = usize::from(self.settings.channel_1 != Channel::None)
            + usize::from(self.settings.channel_2 != Channel::None);
        self.ignores_audio()
            || (self.snapshot.channels == channels
                && self.snapshot.samples.iter().all(|&sample| sample == 0.0))
    }

    pub fn visual_params(&self, bounds: iced::Rectangle) -> Option<OscilloscopeParams> {
        let channels = self.snapshot.channels;
        if channels == 0 { return None; }
        let samples_per_channel = self.snapshot.samples_per_channel;
        Some(OscilloscopeParams {
            geometry: self.geometry,
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

crate::visuals::visualization_widget!(Oscilloscope, OscilloscopeState);
