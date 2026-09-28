// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

macro_rules! form {
    ($($control:expr;)*) => {
        iced::widget::column![$($control),*].spacing($crate::ui::theme::CONTROL_GAP)
    };
}

macro_rules! slider {
    ($label:expr, $value:expr, $range:expr, $on_change:expr, $fmt:literal) => {{
        let (label, value) = ($label, $value);
        $crate::ui::widgets::slide(label, value, format!($fmt, value), $range, $on_change)
    }};
    ($label:expr, $value:expr, $range:expr, $on_change:expr, $display:expr) => {
        $crate::ui::widgets::slide($label, $value, $display, $range, $on_change)
    };
}

pub mod app;
pub mod config;
pub mod settings;
pub mod theme;
pub mod visuals;
mod widgets;

pub(crate) use app::{UiConfig, run};
