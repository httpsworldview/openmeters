// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

pub mod app;
pub mod config;
pub mod settings;
pub mod subscription;
pub mod theme;
pub mod visuals;
pub mod widgets {
    pub mod palette_editor;
    pub mod pane_grid;
    pub mod scroll_glow;
}

pub(crate) fn scroll_delta_lines(delta: iced::advanced::mouse::ScrollDelta) -> f32 {
    match delta {
        iced::advanced::mouse::ScrollDelta::Lines { y, .. } => y,
        iced::advanced::mouse::ScrollDelta::Pixels { y, .. } => y / 50.0,
    }
}

pub(crate) use app::{UiConfig, run};
