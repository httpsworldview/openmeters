// OpenMeters - an audio analysis and visualization tool
// Copyright (C) 2026  Maika Namuo
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use super::palette::PaletteEvent;
use super::widgets::{labeled_pick_list, set_if_changed};
use crate::persistence::settings::{LoudnessSettings, MeterMode};
use crate::ui::theme;
use crate::visuals::registry::VisualKind;
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
