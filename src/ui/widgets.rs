// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo
pub mod pane_grid;
pub mod scroll_glow;

pub fn scroll_delta_lines(delta: iced::advanced::mouse::ScrollDelta) -> f32 {
    match delta {
        iced::advanced::mouse::ScrollDelta::Lines { y, .. } => y,
        iced::advanced::mouse::ScrollDelta::Pixels { y, .. } => y / 50.0,
    }
}
