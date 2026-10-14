// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{OscilloscopeSnapshot, TRACE_COUNT};
use super::render::OscilloscopeParams;
use crate::persistence::settings::OscilloscopeSettings;
use crate::util::{audio::Channel, color::color_to_rgba, finite_or};
use crate::visuals::palettes;
use crate::visuals::render::common::lane_bounds;
use iced::{Color, Rectangle};
use std::sync::Arc;

// Keep new snapshots contributing.
const MAX_PERSISTENCE: f32 = 0.98;

crate::macros::default_struct! {
    pub(crate) struct OscilloscopeState {
        snapshot: OscilloscopeSnapshot = OscilloscopeSnapshot::default(),
        pub(in crate::visuals) palette: [Color; TRACE_COUNT] = palettes::oscilloscope::COLORS,
        pub(in crate::visuals) settings: OscilloscopeSettings = OscilloscopeSettings::default(),
        geometry: crate::visuals::GeometryKey = crate::visuals::GeometryKey::new(),
        lane_1_key: u64 = crate::visuals::next_key(),
    }
}

impl OscilloscopeState {
    pub fn reset_audio(&mut self) {
        self.snapshot = OscilloscopeSnapshot::default();
        self.geometry.invalidate();
    }

    pub fn update_view_settings(&mut self, settings: &OscilloscopeSettings, reset_snapshot: bool) {
        let default = OscilloscopeSettings::default().persistence;
        self.settings = settings.clone();
        self.settings.persistence = finite_or(settings.persistence, default).clamp(0.0, 1.0);
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
                for (current, incoming) in Arc::make_mut(&mut self.snapshot.samples)
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

    pub fn visual_params(&self, bounds: Rectangle) -> impl Iterator<Item = OscilloscopeParams> {
        let channels = self.snapshot.channels;
        let stacked = self.settings.stacked;
        let lanes = if stacked { channels.min(1) } else { channels };
        (0..lanes).filter_map(move |lane| {
            Some(OscilloscopeParams {
                geometry: crate::visuals::GeometryKey {
                    id: [self.geometry.id, self.lane_1_key][lane],
                    ..self.geometry
                },
                bounds: lane_bounds(bounds, lanes, lane)?,
                channels: if stacked { 0..channels } else { lane..lane + 1 },
                samples_per_channel: self.snapshot.samples_per_channel,
                slots: self.snapshot.slots,
                samples: self.snapshot.samples.clone(),
                colors: self.palette.map(color_to_rgba),
            })
        })
    }
}

crate::visuals::visualization_widget!(Oscilloscope, OscilloscopeState);
