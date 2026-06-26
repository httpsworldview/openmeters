// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::widgets::{card, palette_card, pick, set_if_changed};
use crate::persistence::settings::LoudnessSettings;
use crate::visuals::options::MeterMode;
use iced::Element;

settings_pane!(LoudnessSettings);

settings_messages!(pane, value {
    LeftMode(MeterMode) => set_if_changed(&mut pane.settings.left_mode, value);
    RightMode(MeterMode) => set_if_changed(&mut pane.settings.right_mode, value);
});

impl Pane {
    pub(super) fn view(&self) -> Element<'_, Message> {
        controls!(12.0;
            card("Meters", controls!(8.0;
                pick("Left meter mode", MeterMode::ALL, self.settings.left_mode, Message::LeftMode);
                pick("Right meter mode", MeterMode::ALL, self.settings.right_mode, Message::RightMode);
            ));
            palette_card(&self.palette, Message::Palette);
        )
        .into()
    }
}
