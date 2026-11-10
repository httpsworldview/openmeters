// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{fmt_ms, set, settings_messages, settings_pane, settings_view};
use crate::persistence::settings::OscilloscopeSettings;
use crate::ui::widgets::{SliderRange, pick, toggle};
use crate::util::audio::Channel;
use crate::visuals::oscilloscope::processor::{DEFAULT_STABLE_CYCLES, TriggerMode};
use std::fmt;

settings_pane!(
    OscilloscopeSettings,
    extra_from_settings(settings) {
        num_cycles: usize = match settings.trigger_mode {
            TriggerMode::Stable { num_cycles } => num_cycles,
            TriggerMode::ZeroCrossing => DEFAULT_STABLE_CYCLES,
        },
    }
);

const DURATION_RANGE: SliderRange = SliderRange::new(0.005, 0.1, 0.001);
const PERSISTENCE_RANGE: SliderRange = SliderRange::new(0.0, 1.0, 0.01);
const CYCLES_RANGE: SliderRange = SliderRange::new(1.0, 4.0, 1.0);

#[derive(Clone, Copy, PartialEq)]
struct TriggerSourceChoice(Channel);

impl fmt::Display for TriggerSourceChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            Channel::None => "Channel-dependent",
            channel => channel.label(),
        })
    }
}

settings_messages!(pane, settings, value {
    SegmentDuration(f32) => settings.segment_duration.set(value);
    Persistence(f32) => settings.persistence_time_constant.set(value);
    Preset(TriggerPreset) => {
        let mode = match value {
            TriggerPreset::Stable => TriggerMode::Stable { num_cycles: pane.num_cycles },
            TriggerPreset::ZeroCrossing => TriggerMode::ZeroCrossing,
        };
        set(&mut settings.trigger_mode, mode)
    };
    NumCycles(usize) => match settings.trigger_mode {
        TriggerMode::Stable { .. } => {
            pane.num_cycles = value;
            set(&mut settings.trigger_mode, TriggerMode::Stable { num_cycles: value })
        }
        TriggerMode::ZeroCrossing => false,
    };
    TriggerSource(Channel) => set(&mut settings.trigger_source, value);
    Channel1(Channel) => set(&mut settings.channel_1, value);
    Channel2(Channel) => set(&mut settings.channel_2, value);
    Stacked(bool) => set(&mut settings.stacked, value);
});

settings_view! {
    pane as settings {
        let (preset, duration_label, cycles) = match settings.trigger_mode {
            TriggerMode::ZeroCrossing => (TriggerPreset::ZeroCrossing, "Segment duration", None),
            TriggerMode::Stable { num_cycles } => (
                TriggerPreset::Stable,
                "Segment duration (fallback)",
                Some(slider!(
                    "Cycles", num_cycles as f32, CYCLES_RANGE,
                    |value| NumCycles(value.round() as usize), num_cycles.to_string()
                )),
            ),
        };
        let channels = Channel::ALL.iter().copied().filter(|&channel| channel != Channel::None);
        let sources: Vec<_> = [Channel::None].into_iter().chain(channels).map(TriggerSourceChoice).collect();
        let trigger = form!(
            pick("Mode", TriggerPreset::ALL, preset, Preset);
            pick(
                "Trigger source", sources, TriggerSourceChoice(settings.trigger_source),
                |choice| TriggerSource(choice.0)
            );
            cycles;
            slider!(
                duration_label, settings.segment_duration.get(), DURATION_RANGE, SegmentDuration,
                fmt_ms(settings.segment_duration.get(), 1)
            );
        );
    }
    "Signal" => form!(
        pick("Channel 1", Channel::ALL, settings.channel_1, Channel1);
        pick("Channel 2", Channel::ALL, settings.channel_2, Channel2);
    );
    "Trigger" => trigger;
    "Display" => form!(
        toggle("Stacked", settings.stacked, Stacked);
        slider!(
            "Persistence", settings.persistence_time_constant.get(), PERSISTENCE_RANGE,
            Persistence, fmt_ms(settings.persistence_time_constant.get(), 0)
        );
    );
}

crate::macros::choice_enum!(pub(in crate::ui) enum TriggerPreset {
    ZeroCrossing => "Zero-crossing",
    Stable => "Stable",
});
