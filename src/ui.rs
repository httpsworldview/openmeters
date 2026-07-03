// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

macro_rules! controls {
    ($spacing:literal; $($control:expr;)*) => {{
        controls!(@push iced::widget::Column::new().spacing($spacing); $($control;)*)
    }};
    ($base:expr; $($control:expr;)*) => {{
        controls!(@push $base; $($control;)*)
    }};
    (@push $base:expr; $($control:expr;)*) => {{
        let mut column = $base;
        $(column = column.push($control);)*
        column
    }};
}

macro_rules! slider {
    ($label:expr, $value:expr, $range:expr, $on_change:expr, $fmt:literal) => {
        $crate::ui::settings::widgets::slide(
            $label,
            $value,
            format!($fmt, $value),
            $range,
            $on_change,
        )
    };
    ($label:expr, $value:expr, $range:expr, $on_change:expr, $display:expr) => {
        $crate::ui::settings::widgets::slide($label, $value, $display, $range, $on_change)
    };
}

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

pub(crate) fn clipped_text<'a, M: 'a>(
    content: impl iced::widget::text::IntoFragment<'a>,
    size: f32,
) -> iced::widget::container::Container<'a, M> {
    iced::widget::container(
        iced::widget::text(content)
            .size(size)
            .wrapping(iced::widget::text::Wrapping::None),
    )
    .clip(true)
}

pub(crate) fn scroll_delta_lines(delta: iced::advanced::mouse::ScrollDelta) -> f32 {
    match delta {
        iced::advanced::mouse::ScrollDelta::Lines { y, .. } => y,
        iced::advanced::mouse::ScrollDelta::Pixels { y, .. } => y / 50.0,
    }
}

pub(crate) use app::{UiConfig, run};
