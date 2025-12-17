use super::palette::{PaletteEditor, PaletteEvent};
use super::widgets::{SliderRange, labeled_pick_list, labeled_slider, set_f32, set_if_changed};
use super::{CHANNEL_OPTIONS, ModuleSettingsPane, SettingsMessage};
use crate::dsp::oscilloscope::TriggerMode;
use crate::ui::settings::{ChannelMode, OscilloscopeSettings, SettingsHandle};
use crate::ui::theme;
use crate::ui::visualization::visual_manager::{VisualId, VisualKind, VisualManagerHandle};
use iced::Element;
use iced::widget::column;

#[derive(Debug)]
pub struct OscilloscopeSettingsPane {
    visual_id: VisualId,
    settings: OscilloscopeSettings,
    palette: PaletteEditor,
}

#[derive(Debug, Clone, Copy)]
pub enum Message {
    SegmentDuration(f32),
    Persistence(f32),
    TriggerMode(TriggerMode),
    NumCycles(usize),
    ChannelMode(ChannelMode),
    Palette(PaletteEvent),
}

pub fn create(
    visual_id: VisualId,
    visual_manager: &VisualManagerHandle,
) -> OscilloscopeSettingsPane {
    let (settings, palette) = super::load_settings_and_palette(
        visual_manager,
        VisualKind::Oscilloscope,
        &theme::DEFAULT_OSCILLOSCOPE_PALETTE,
        &[],
    );
    OscilloscopeSettingsPane {
        visual_id,
        settings,
        palette,
    }
}

impl ModuleSettingsPane for OscilloscopeSettingsPane {
    fn visual_id(&self) -> VisualId {
        self.visual_id
    }

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
                SliderRange::new(1.0, 4.0, 1.0),
                |v| SettingsMessage::Oscilloscope(Message::NumCycles(v as usize)),
            ));
        }

        content
            .push(labeled_slider(
                dur_label,
                self.settings.segment_duration,
                format!("{:.1} ms", self.settings.segment_duration * 1000.0),
                SliderRange::new(0.005, 0.1, 0.001),
                |v| SettingsMessage::Oscilloscope(Message::SegmentDuration(v)),
            ))
            .push(labeled_slider(
                "Persistence",
                self.settings.persistence,
                format!("{:.2}", self.settings.persistence),
                SliderRange::new(0.0, 1.0, 0.01),
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
            Message::SegmentDuration(v) => {
                set_f32(&mut self.settings.segment_duration, v.clamp(0.005, 0.1))
            }
            Message::Persistence(v) => set_f32(&mut self.settings.persistence, v.clamp(0.0, 1.0)),
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
