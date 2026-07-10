// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{set, set_f32};
use crate::persistence::settings::OscilloscopeSettings;
use crate::ui::widgets::{SliderRange, pick, toggle};
use crate::util::audio::Channel;
use crate::visuals::oscilloscope::processor::TriggerMode;
use std::fmt;

settings_pane!(
    OscilloscopeSettings,
    extra_from_settings(settings) {
        num_cycles: usize = match settings.trigger_mode {
            TriggerMode::Stable { num_cycles } => num_cycles,
            TriggerMode::ZeroCrossing => 2,
        },
    }
);

const DURATION_RANGE: SliderRange = SliderRange::new(0.005, 0.1, 0.001);
const PERSISTENCE_RANGE: SliderRange = SliderRange::new(0.0, 1.0, 0.01);
const CYCLES_RANGE: SliderRange = SliderRange::new(1.0, 4.0, 1.0);

#[derive(Clone, Copy, PartialEq)]
struct TriggerSourceChoice(Channel);

impl TriggerSourceChoice {
    const ALL: &'static [Self] = &[
        Self(Channel::None),
        Self(Channel::Left),
        Self(Channel::Right),
        Self(Channel::Mid),
        Self(Channel::Side),
    ];
}

impl fmt::Display for TriggerSourceChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            Channel::None => "Channel-dependent",
            channel => channel.label(),
        })
    }
}

settings_messages!(pane, settings, value {
    SegmentDuration(f32) => set_f32(&mut settings.segment_duration, value, DURATION_RANGE);
    Persistence(f32) => set_f32(&mut settings.persistence, value, PERSISTENCE_RANGE);
    Preset(TriggerPreset) => {
        let mode = match value {
            TriggerPreset::Stable => TriggerMode::Stable { num_cycles: pane.num_cycles },
            TriggerPreset::ZeroCrossing => TriggerMode::ZeroCrossing,
        };
        set(&mut settings.trigger_mode, mode)
    };
    NumCycles(usize) => match settings.trigger_mode {
        TriggerMode::Stable { .. } => {
            let cycles = value.clamp(CYCLES_RANGE.min as usize, CYCLES_RANGE.max as usize);
            pane.num_cycles = cycles;
            set(&mut settings.trigger_mode, TriggerMode::Stable { num_cycles: cycles })
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
        let preset = TriggerPreset::from_mode(settings.trigger_mode);
        let duration_label = match preset {
            TriggerPreset::Stable => "Segment duration (fallback)",
            TriggerPreset::ZeroCrossing => "Segment duration",
        };
        let mut trigger = form!(
            pick("Mode", TriggerPreset::ALL, preset, Preset);
            pick(
                "Trigger source", TriggerSourceChoice::ALL,
                TriggerSourceChoice(settings.trigger_source),
                |choice| TriggerSource(choice.0)
            );
        );
        if let TriggerMode::Stable { num_cycles } = settings.trigger_mode {
            trigger = trigger.push(slider!(
                "Cycles", num_cycles as f32, CYCLES_RANGE,
                |value| NumCycles(value.round() as usize), num_cycles.to_string()
            ));
        }
        trigger = trigger.push(slider!(
            duration_label, settings.segment_duration, DURATION_RANGE, SegmentDuration,
            format!("{:.1} ms", settings.segment_duration * 1000.0)
        ));
    }
    "Signal" => form!(
        pick("Channel 1", Channel::ALL, settings.channel_1, Channel1);
        pick("Channel 2", Channel::ALL, settings.channel_2, Channel2);
    );
    "Trigger" => trigger;
    "Display" => form!(
        toggle("Stacked", settings.stacked, Stacked);
        slider!("Persistence", settings.persistence, PERSISTENCE_RANGE, Persistence, "{:.2}");
    );
}

crate::macros::choice_enum!(no_default all pub(in crate::ui) enum TriggerPreset {
    ZeroCrossing => "Zero-crossing",
    Stable => "Stable",
});

impl TriggerPreset {
    fn from_mode(mode: TriggerMode) -> Self {
        match mode {
            TriggerMode::ZeroCrossing => Self::ZeroCrossing,
            TriggerMode::Stable { .. } => Self::Stable,
        }
    }
}
