use super::SettingsMessage;
use super::palette::PaletteEvent;
use super::widgets::{labeled_pick_list, set_if_changed};
use crate::ui::settings::{LoudnessSettings, MeterMode, SettingsHandle};
use crate::ui::theme;
use crate::ui::visualization::visual_manager::{VisualKind, VisualManagerHandle};
use iced::{Element, widget::column};

settings_pane!(
    LoudnessSettingsPane,
    LoudnessSettings,
    VisualKind::Loudness,
    theme::loudness
);

#[derive(Debug, Clone)]
pub enum Message {
    LeftMode(MeterMode),
    RightMode(MeterMode),
    Palette(PaletteEvent),
}

impl LoudnessSettingsPane {
    fn view(&self) -> Element<'_, SettingsMessage> {
        column![
            labeled_pick_list(
                "Left meter mode",
                MeterMode::ALL,
                Some(self.settings.left_mode),
                |m| SettingsMessage::Loudness(Message::LeftMode(m))
            ),
            labeled_pick_list(
                "Right meter mode",
                MeterMode::ALL,
                Some(self.settings.right_mode),
                |m| SettingsMessage::Loudness(Message::RightMode(m))
            ),
            super::palette_section(&self.palette, Message::Palette, SettingsMessage::Loudness)
        ]
        .spacing(16)
        .into()
    }

    fn handle(
        &mut self,
        message: &SettingsMessage,
        visual_manager: &VisualManagerHandle,
        settings_handle: &SettingsHandle,
    ) {
        let SettingsMessage::Loudness(msg) = message else {
            return;
        };
        let changed = match msg {
            Message::LeftMode(mode) => set_if_changed(&mut self.settings.left_mode, *mode),
            Message::RightMode(mode) => set_if_changed(&mut self.settings.right_mode, *mode),
            Message::Palette(event) => self.palette.update(*event),
        };
        if changed {
            persist_palette!(
                visual_manager,
                settings_handle,
                VisualKind::Loudness,
                self,
                &theme::loudness::COLORS
            );
        }
    }
}
