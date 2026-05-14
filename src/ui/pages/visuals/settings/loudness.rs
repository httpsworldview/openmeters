// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::palette::PaletteEvent;
use super::widgets::{pick, set_if_changed};
use crate::persistence::settings::{LoudnessSettings, MeterMode};
use crate::visuals::registry::VisualKind;
use iced::Element;

settings_pane!(
    LoudnessSettingsPane,
    LoudnessSettings,
    VisualKind::Loudness,
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
        controls!(16.0;
            pick("Left meter mode", MeterMode::ALL, self.settings.left_mode, Message::LeftMode);
            pick("Right meter mode", MeterMode::ALL, self.settings.right_mode, Message::RightMode);
            super::palette_section(&self.palette, Message::Palette);
        )
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
