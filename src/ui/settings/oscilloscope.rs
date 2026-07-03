// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::widgets::{SliderRange, card, palette_card, pick, set_if_changed, update_f32_range};
use crate::persistence::settings::OscilloscopeSettings;
use crate::util::audio::Channel;
use crate::visuals::oscilloscope::processor::TriggerMode;
use iced::Element;
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

const SEGMENT_DURATION_RANGE: SliderRange = SliderRange::new(0.005, 0.1, 0.001);
const PERSISTENCE_RANGE: SliderRange = SliderRange::new(0.0, 1.0, 0.01);
const NUM_CYCLES_RANGE: SliderRange = SliderRange::new(1.0, 4.0, 1.0);

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

settings_messages!(pane, value {
    SegmentDuration(f32) => update_f32_range(&mut pane.settings.segment_duration, value, SEGMENT_DURATION_RANGE);
    Persistence(f32) => update_f32_range(&mut pane.settings.persistence, value, PERSISTENCE_RANGE);
    TriggerMode(TriggerPreset) => {
        let mode = match value {
            TriggerPreset::Stable => TriggerMode::Stable { num_cycles: pane.num_cycles },
            TriggerPreset::ZeroCrossing => TriggerMode::ZeroCrossing,
        };
        set_if_changed(&mut pane.settings.trigger_mode, mode)
    };
    NumCycles(usize) => match pane.settings.trigger_mode {
        TriggerMode::Stable { .. } => {
            let cycles = value.clamp(NUM_CYCLES_RANGE.min as usize, NUM_CYCLES_RANGE.max as usize);
            pane.num_cycles = cycles;
            set_if_changed(&mut pane.settings.trigger_mode, TriggerMode::Stable { num_cycles: cycles })
        }
        TriggerMode::ZeroCrossing => false,
    };
    TriggerSource(Channel) => set_if_changed(&mut pane.settings.trigger_source, value);
    Channel1(Channel) => set_if_changed(&mut pane.settings.channel_1, value);
    Channel2(Channel) => set_if_changed(&mut pane.settings.channel_2, value);
});

impl Pane {
    pub(super) fn view(&self) -> Element<'_, Message> {
        let preset = TriggerPreset::from_mode(self.settings.trigger_mode);
        let dur_label = match preset {
            TriggerPreset::Stable => "Segment duration (fallback)",
            TriggerPreset::ZeroCrossing => "Segment duration",
        };
        let signal = controls!(8.0;
            pick("Channel 1", Channel::ALL, self.settings.channel_1, Message::Channel1);
            pick("Channel 2", Channel::ALL, self.settings.channel_2, Message::Channel2);
        );
        let mut trigger = controls!(8.0;
            pick("Mode", TriggerPreset::ALL, preset, Message::TriggerMode);
            pick(
                "Trigger source",
                TriggerSourceChoice::ALL,
                TriggerSourceChoice(self.settings.trigger_source),
                |choice| Message::TriggerSource(choice.0)
            );
        );
        if let TriggerMode::Stable { num_cycles } = self.settings.trigger_mode {
            trigger = controls!(trigger;
                slider!(
                    "Cycles",
                    num_cycles as f32,
                    NUM_CYCLES_RANGE,
                    |v| Message::NumCycles(v.round() as usize),
                    num_cycles.to_string()
                );
            );
        }
        trigger = controls!(trigger;
            slider!(
                dur_label, self.settings.segment_duration, SEGMENT_DURATION_RANGE,
                Message::SegmentDuration, format!("{:.1} ms", self.settings.segment_duration * 1000.0)
            );
        );

        controls!(12.0;
            card("Signal", signal);
            card("Trigger", trigger);
            card("Display", controls!(8.0;
                slider!("Persistence", self.settings.persistence, PERSISTENCE_RANGE, Message::Persistence, "{:.2}");
            ));
            palette_card(&self.palette, Message::Palette);
        )
        .into()
    }
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
