use super::palette::PaletteEvent;
use super::widgets::{labeled_pick_list, set_if_changed};
use crate::ui::settings::{LoudnessSettings, MeterMode};
use crate::ui::theme;
use crate::ui::visualization::visual_manager::VisualKind;
use iced::{Element, widget::column};

settings_pane!(
    LoudnessSettingsPane,
    LoudnessSettings,
    VisualKind::Loudness,
    theme::loudness,
    Loudness
);

#[derive(Debug, Clone)]
pub enum Message {
    LeftMode(MeterMode),
    RightMode(MeterMode),
    Palette(PaletteEvent),
}

impl LoudnessSettingsPane {
    fn view(&self) -> Element<'_, Message> {
        column![
            labeled_pick_list(
                "Left meter mode",
                MeterMode::ALL,
                Some(self.settings.left_mode),
                Message::LeftMode
            ),
            labeled_pick_list(
                "Right meter mode",
                MeterMode::ALL,
                Some(self.settings.right_mode),
                Message::RightMode
            ),
            super::palette_section(&self.palette, Message::Palette)
        ]
        .spacing(16)
        .into()
    }

    fn handle(&mut self, msg: &Message) -> bool {
        match msg {
            Message::LeftMode(mode) => set_if_changed(&mut self.settings.left_mode, *mode),
            Message::RightMode(mode) => set_if_changed(&mut self.settings.right_mode, *mode),
            Message::Palette(event) => self.palette.update(*event),
        }
    }
}
