// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::set;
use crate::persistence::settings::LoudnessSettings;
use crate::ui::widgets::pick;
use crate::visuals::options::MeterMode;

settings_pane!(LoudnessSettings);

settings_messages!(pane, settings, value {
    LeftMode(MeterMode) => set(&mut settings.left_mode, value);
    RightMode(MeterMode) => set(&mut settings.right_mode, value);
});

settings_view! {
    pane as settings {}
    "Meters" => form!(
        pick("Left meter mode", MeterMode::ALL, settings.left_mode, LeftMode);
        pick("Right meter mode", MeterMode::ALL, settings.right_mode, RightMode);
    );
}
