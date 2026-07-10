// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

macro_rules! form {
    ($($control:expr;)*) => {
        iced::widget::column![$($control),*].spacing($crate::ui::theme::CONTROL_GAP)
    };
}

macro_rules! slider {
    ($label:expr, $value:expr, $range:expr, $on_change:expr, $fmt:literal) => {
        slider!($label, $value, $range, $on_change, format!($fmt, $value))
    };
    ($label:expr, $value:expr, $range:expr, $on_change:expr, $display:expr) => {
        $crate::ui::widgets::slide($label, $value, $display, $range, $on_change)
    };
}

pub mod app;
pub mod config;
pub mod settings;
pub mod subscription;
pub mod theme;
pub mod visuals;
mod widgets;

pub(crate) fn scroll_delta_lines(delta: iced::advanced::mouse::ScrollDelta) -> f32 {
    match delta {
        iced::advanced::mouse::ScrollDelta::Lines { y, .. } => y,
        iced::advanced::mouse::ScrollDelta::Pixels { y, .. } => y / 50.0,
    }
}

pub(crate) use app::{UiConfig, run};
