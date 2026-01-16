use super::palette::PaletteEvent;
use super::widgets::{
    SliderRange, labeled_pick_list, labeled_slider, set_if_changed, update_f32_range,
};
use super::{CHANNEL_OPTIONS, SettingsMessage};
use crate::dsp::oscilloscope::TriggerMode;
use crate::ui::settings::{ChannelMode, OscilloscopeSettings, SettingsHandle};
use crate::ui::theme;
use crate::ui::visualization::visual_manager::{VisualKind, VisualManagerHandle};
use iced::Element;
use iced::widget::column;

settings_pane!(
    OscilloscopeSettingsPane,
    OscilloscopeSettings,
    VisualKind::Oscilloscope,
    theme::DEFAULT_OSCILLOSCOPE_PALETTE
);

const SEGMENT_DURATION_RANGE: SliderRange = SliderRange::new(0.005, 0.1, 0.001);
const PERSISTENCE_RANGE: SliderRange = SliderRange::new(0.0, 1.0, 0.01);
const NUM_CYCLES_RANGE: SliderRange = SliderRange::new(1.0, 4.0, 1.0);

#[derive(Debug, Clone, Copy)]
pub enum Message {
    SegmentDuration(f32),
    Persistence(f32),
    TriggerMode(TriggerMode),
    NumCycles(usize),
    ChannelMode(ChannelMode),
    Palette(PaletteEvent),
}

impl OscilloscopeSettingsPane {
    fn view(&self) -> Element<'_, SettingsMessage> {
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
                &["Free-run", "Stable"],
                Some(if is_stable { "Stable" } else { "Free-run" }),
                |l| SettingsMessage::Oscilloscope(Message::TriggerMode(if l == "Stable" {
                    TriggerMode::Stable { num_cycles: 1 }
                } else {
                    TriggerMode::FreeRun
                }))
            ),
            labeled_pick_list(
                "Channels",
                &CHANNEL_OPTIONS,
                Some(self.settings.channel_mode),
                |m| SettingsMessage::Oscilloscope(Message::ChannelMode(m))
            ),
        ]
        .spacing(16);

        if let TriggerMode::Stable { num_cycles } = mode {
            content = content.push(labeled_slider(
                "Cycles",
                num_cycles as f32,
                num_cycles.to_string(),
                NUM_CYCLES_RANGE,
                |v| SettingsMessage::Oscilloscope(Message::NumCycles(v as usize)),
            ));
        }

        content
            .push(labeled_slider(
                dur_label,
                self.settings.segment_duration,
                format!("{:.1} ms", self.settings.segment_duration * 1000.0),
                SEGMENT_DURATION_RANGE,
                |v| SettingsMessage::Oscilloscope(Message::SegmentDuration(v)),
            ))
            .push(labeled_slider(
                "Persistence",
                self.settings.persistence,
                format!("{:.2}", self.settings.persistence),
                PERSISTENCE_RANGE,
                |v| SettingsMessage::Oscilloscope(Message::Persistence(v)),
            ))
            .push(super::palette_section(
                &self.palette,
                Message::Palette,
                SettingsMessage::Oscilloscope,
            ))
            .into()
    }

    fn handle(
        &mut self,
        message: &SettingsMessage,
        visual_manager: &VisualManagerHandle,
        settings_handle: &SettingsHandle,
    ) {
        let SettingsMessage::Oscilloscope(msg) = message else {
            return;
        };
        let changed = match *msg {
            Message::SegmentDuration(v) => update_f32_range(
                &mut self.settings.segment_duration,
                v,
                SEGMENT_DURATION_RANGE,
            ),
            Message::Persistence(v) => {
                update_f32_range(&mut self.settings.persistence, v, PERSISTENCE_RANGE)
            }
            Message::TriggerMode(m) => set_if_changed(&mut self.settings.trigger_mode, m),
            Message::NumCycles(c) => match self.settings.trigger_mode {
                TriggerMode::Stable { .. } => set_if_changed(
                    &mut self.settings.trigger_mode,
                    TriggerMode::Stable {
                        num_cycles: c.clamp(1, 4),
                    },
                ),
                TriggerMode::FreeRun => false,
            },
            Message::ChannelMode(m) => set_if_changed(&mut self.settings.channel_mode, m),
            Message::Palette(e) => self.palette.update(e),
        };
        if changed {
            persist_palette!(
                visual_manager,
                settings_handle,
                VisualKind::Oscilloscope,
                self,
                theme::DEFAULT_OSCILLOSCOPE_PALETTE
            );
        }
    }
}
