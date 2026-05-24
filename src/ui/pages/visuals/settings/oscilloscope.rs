// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::widgets::{SliderRange, pick, set_if_changed, slide, update_f32_range};
use crate::persistence::settings::OscilloscopeSettings;
use crate::util::audio::Channel;
use crate::visuals::oscilloscope::processor::TriggerMode;
use crate::visuals::registry::VisualKind;
use iced::Element;

settings_pane!(
    OscilloscopeSettingsPane, OscilloscopeSettings, VisualKind::Oscilloscope, Oscilloscope,
    extra_from_settings(settings) {
        num_cycles: usize = match settings.trigger_mode {
            TriggerMode::Stable { num_cycles } => num_cycles,
            _ => 2,
        },
    }
);

const SEGMENT_DURATION_RANGE: SliderRange = SliderRange::new(0.005, 0.1, 0.001);
const PERSISTENCE_RANGE: SliderRange = SliderRange::new(0.0, 1.0, 0.01);
const NUM_CYCLES_RANGE: SliderRange = SliderRange::new(1.0, 4.0, 1.0);

settings_messages!(OscilloscopeSettingsPane as pane, value {
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
    Channel1(Channel) => set_if_changed(&mut pane.settings.channel_1, value);
    Channel2(Channel) => set_if_changed(&mut pane.settings.channel_2, value);
});

impl OscilloscopeSettingsPane {
    fn view(&self) -> Element<'_, Message> {
        let preset = TriggerPreset::from_mode(self.settings.trigger_mode);
        let dur_label = match preset {
            TriggerPreset::Stable => "Segment duration (fallback)",
            TriggerPreset::ZeroCrossing => "Segment duration",
        };
        let mut content = controls!(16.0;
            pick("Mode", TriggerPreset::ALL, preset, Message::TriggerMode);
            pick("Channel 1", Channel::ALL, self.settings.channel_1, Message::Channel1);
            pick("Channel 2", Channel::ALL, self.settings.channel_2, Message::Channel2);
        );

        if let TriggerMode::Stable { num_cycles } = self.settings.trigger_mode {
            content = content.push(slider!(
                "Cycles",
                num_cycles as f32,
                NUM_CYCLES_RANGE,
                |v| Message::NumCycles(v as usize),
                num_cycles.to_string()
            ));
        }

        controls!(content;
            slider!(
                dur_label, self.settings.segment_duration, SEGMENT_DURATION_RANGE,
                Message::SegmentDuration, format!("{:.1} ms", self.settings.segment_duration * 1000.0)
            );
            slider!("Persistence", self.settings.persistence, PERSISTENCE_RANGE, Message::Persistence, "{:.2}");
            super::palette_section(&self.palette, Message::Palette);
        )
        .into()
    }
}

crate::macros::choice_enum!(no_default all pub(crate) enum TriggerPreset {
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
