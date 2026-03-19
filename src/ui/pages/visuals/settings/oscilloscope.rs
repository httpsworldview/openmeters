// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::CHANNEL_OPTIONS;
use super::palette::PaletteEvent;
use super::widgets::{
    SliderRange, labeled_pick_list, labeled_slider, set_if_changed, update_f32_range,
};
use crate::persistence::settings::{ChannelMode, OscilloscopeSettings};
use crate::ui::theme;
use crate::visuals::oscilloscope::processor::TriggerMode;
use crate::visuals::registry::VisualKind;
use iced::Element;
use iced::widget::column;

fn extract_num_cycles(mode: TriggerMode) -> usize {
    match mode {
        TriggerMode::Stable { num_cycles } => num_cycles,
        _ => 1,
    }
}

settings_pane!(
    OscilloscopeSettingsPane,
    OscilloscopeSettings,
    VisualKind::Oscilloscope,
    theme::oscilloscope,
    Oscilloscope,
    extra_from_settings(settings) {
        num_cycles: usize = extract_num_cycles(settings.trigger_mode),
    }
);

const SEGMENT_DURATION_RANGE: SliderRange = SliderRange::new(0.005, 0.1, 0.001);
const PERSISTENCE_RANGE: SliderRange = SliderRange::new(0.0, 1.0, 0.01);
const NUM_CYCLES_RANGE: SliderRange = SliderRange::new(1.0, 4.0, 1.0);

#[derive(Debug, Clone, Copy)]
pub enum Message {
    SegmentDuration(f32),
    Persistence(f32),
    TriggerMode(bool), // true = stable
    NumCycles(usize),
    ChannelMode(ChannelMode),
    Palette(PaletteEvent),
}

impl OscilloscopeSettingsPane {
    fn view(&self) -> Element<'_, Message> {
        let mode = self.settings.trigger_mode;
        let is_stable = matches!(mode, TriggerMode::Stable { .. });
        let dur_label = if is_stable {
            "Segment duration (fallback)"
        } else {
            "Segment duration"
        };

        let mut content = column![
            labeled_pick_list(
                "Mode",
                &["Zero-crossing", "Stable"],
                Some(if is_stable { "Stable" } else { "Zero-crossing" }),
                |l| Message::TriggerMode(l == "Stable")
            ),
            labeled_pick_list(
                "Channels",
                &CHANNEL_OPTIONS,
                Some(self.settings.channel_mode),
                Message::ChannelMode
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
            Message::TriggerMode(stable) => {
                let mode = if stable {
                    TriggerMode::Stable {
                        num_cycles: self.num_cycles,
                    }
                } else {
                    TriggerMode::ZeroCrossing
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
            Message::ChannelMode(m) => set_if_changed(&mut self.settings.channel_mode, m),
            Message::Palette(e) => self.palette.update(e),
        }
    }
}
