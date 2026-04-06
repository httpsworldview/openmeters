// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::palette::PaletteEvent;
use super::widgets::{
    SliderRange, labeled_pick_list, labeled_slider, set_if_changed, update_f32_range,
};
use crate::persistence::settings::{Channel, OscilloscopeSettings};
use crate::ui::theme;
use crate::visuals::oscilloscope::processor::TriggerMode;
use crate::visuals::registry::VisualKind;
use iced::Element;
use iced::widget::column;

settings_pane!(
    OscilloscopeSettingsPane,
    OscilloscopeSettings,
    VisualKind::Oscilloscope,
    theme::oscilloscope,
    Oscilloscope,
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

#[derive(Debug, Clone, Copy)]
pub enum Message {
    SegmentDuration(f32),
    Persistence(f32),
    TriggerMode(TriggerPreset),
    NumCycles(usize),
    Channel1(Channel),
    Channel2(Channel),
    Palette(PaletteEvent),
}

impl OscilloscopeSettingsPane {
    fn view(&self) -> Element<'_, Message> {
        let mode = self.settings.trigger_mode;
        let preset = TriggerPreset::from_mode(mode);
        let dur_label = if preset == TriggerPreset::Stable {
            "Segment duration (fallback)"
        } else {
            "Segment duration"
        };

        let mut content = column![
            labeled_pick_list(
                "Mode",
                &TriggerPreset::ALL,
                Some(preset),
                Message::TriggerMode
            ),
            labeled_pick_list(
                "Channel 1",
                Channel::ALL,
                Some(self.settings.channel_1),
                Message::Channel1
            ),
            labeled_pick_list(
                "Channel 2",
                Channel::ALL,
                Some(self.settings.channel_2),
                Message::Channel2
            ),
        ]
        .spacing(16);

        if let TriggerMode::Stable { num_cycles } = mode {
            content = content.push(labeled_slider(
                "Cycles",
                num_cycles as f32,
                num_cycles.to_string(),
                NUM_CYCLES_RANGE,
                |v| Message::NumCycles(v as usize),
            ));
        }

        content
            .push(labeled_slider(
                dur_label,
                self.settings.segment_duration,
                format!("{:.1} ms", self.settings.segment_duration * 1000.0),
                SEGMENT_DURATION_RANGE,
                Message::SegmentDuration,
            ))
            .push(labeled_slider(
                "Persistence",
                self.settings.persistence,
                format!("{:.2}", self.settings.persistence),
                PERSISTENCE_RANGE,
                Message::Persistence,
            ))
            .push(super::palette_section(&self.palette, Message::Palette))
            .into()
    }

    fn handle(&mut self, msg: &Message) -> bool {
        match *msg {
            Message::SegmentDuration(v) => update_f32_range(
                &mut self.settings.segment_duration,
                v,
                SEGMENT_DURATION_RANGE,
            ),
            Message::Persistence(v) => {
                update_f32_range(&mut self.settings.persistence, v, PERSISTENCE_RANGE)
            }
            Message::TriggerMode(preset) => {
                let mode = match preset {
                    TriggerPreset::Stable => TriggerMode::Stable {
                        num_cycles: self.num_cycles,
                    },
                    TriggerPreset::ZeroCrossing => TriggerMode::ZeroCrossing,
                };
                set_if_changed(&mut self.settings.trigger_mode, mode)
            }
            Message::NumCycles(c) => match self.settings.trigger_mode {
                TriggerMode::Stable { .. } => {
                    let clamped =
                        c.clamp(NUM_CYCLES_RANGE.min as usize, NUM_CYCLES_RANGE.max as usize);
                    self.num_cycles = clamped;
                    set_if_changed(
                        &mut self.settings.trigger_mode,
                        TriggerMode::Stable {
                            num_cycles: clamped,
                        },
                    )
                }
                TriggerMode::ZeroCrossing => false,
            },
            Message::Channel1(ch) => set_if_changed(&mut self.settings.channel_1, ch),
            Message::Channel2(ch) => set_if_changed(&mut self.settings.channel_2, ch),
            Message::Palette(e) => self.palette.update(e),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TriggerPreset {
    ZeroCrossing,
    Stable,
}

impl TriggerPreset {
    const ALL: [Self; 2] = [Self::ZeroCrossing, Self::Stable];

    fn from_mode(mode: TriggerMode) -> Self {
        match mode {
            TriggerMode::ZeroCrossing => Self::ZeroCrossing,
            TriggerMode::Stable { .. } => Self::Stable,
        }
    }
}

impl std::fmt::Display for TriggerPreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ZeroCrossing => "Zero-crossing",
            Self::Stable => "Stable",
        })
    }
}
