// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{OscilloscopeSnapshot, TRACE_COUNT};
use super::render::OscilloscopeParams;
use crate::persistence::settings::OscilloscopeSettings;
use crate::util::{
    audio::{Channel, smoothing_weight},
    color::color_to_rgba,
};
use crate::visuals::palettes;
use crate::visuals::render::common::lane_bounds;
use iced::{Color, Rectangle};
use std::sync::Arc;

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

    pub fn update_view_settings(&mut self, settings: &OscilloscopeSettings) {
        self.settings = settings.clone();
        if self.ignores_audio() {
            self.snapshot = OscilloscopeSnapshot::default();
        }
        self.geometry.invalidate();
    }

    crate::visuals::palette_setter!(oscilloscope => geometry);

    pub fn apply_snapshot(&mut self, mut snapshot: OscilloscopeSnapshot) {
        self.geometry.invalidate();
        let time_constant = self.settings.persistence_time_constant.get();
        let weight = smoothing_weight(snapshot.elapsed, time_constant);
        if weight < 1.0
            && !self.snapshot.samples.is_empty()
            && snapshot.epoch == self.snapshot.epoch
            && snapshot.channels == self.snapshot.channels
            && snapshot.samples_per_channel == self.snapshot.samples_per_channel
            && snapshot.slots[..snapshot.channels] == self.snapshot.slots[..self.snapshot.channels]
        {
            // Blended in the incoming buffer: the shown one is shared with the last frame drawn.
            for (current, &previous) in Arc::make_mut(&mut snapshot.samples)
                .iter_mut()
                .zip(self.snapshot.samples.iter())
            {
                let incoming = *current;
                *current = previous + weight * (incoming - previous);
                crate::util::audio::flush_denormal_f32(current);
                // A trace fading into silence ends below half a pixel, so the visual can go quiet.
                if incoming == 0.0 && current.abs() < 1.0e-3 {
                    *current = 0.0;
                }
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
        self.snapshot.channels == channels
            && self.snapshot.samples.iter().all(|&sample| sample == 0.0)
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
                samples: Arc::clone(&self.snapshot.samples),
                colors: self.palette.map(color_to_rgba),
            })
        })
    }
}

crate::visuals::visualization_widget!(Oscilloscope, OscilloscopeState);
