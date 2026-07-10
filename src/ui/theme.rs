// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use iced::border::Border;
use iced::theme::palette::{self, Extended};
use iced::widget::{button, container, slider, text};
use iced::{Background, Color, Theme};

use crate::util::color::{lerp_color, with_alpha};

pub use crate::visuals::palettes::{BG_BASE, Palette, background};

pub const BODY_TEXT_SIZE: f32 = 12.0;
pub const CONTROL_GAP: f32 = 8.0;
pub const SECTION_GAP: f32 = 12.0;

const TEXT_PRIMARY: Color = Color::from_rgba(0.902, 0.910, 0.925, 1.0);
const TEXT_DARK: Color = Color::from_rgba(0.10, 0.10, 0.10, 1.0);

const ACCENT_PRIMARY: Color = Color::from_rgba(0.157, 0.157, 0.157, 1.0);
const ACCENT_SUCCESS: Color = Color::from_rgba(0.478, 0.557, 0.502, 1.0);
const ACCENT_DANGER: Color = Color::from_rgba(0.557, 0.478, 0.478, 1.0);

pub fn theme(custom_bg: Option<Color>) -> Theme {
    Theme::custom_with_fn("OpenMeters Monochrome", palette(custom_bg), |base| {
        let mut extended = Extended::generate(base);
        extended.background.weak = extended.background.neutral;
        extended
    })
}

fn readable_text(background: Color) -> Color {
    if palette::is_dark(background) {
        TEXT_PRIMARY
    } else {
        TEXT_DARK
    }
}

fn palette(custom_bg: Option<Color>) -> palette::Palette {
    let background = custom_bg.unwrap_or(BG_BASE);
    let text = readable_text(background);

    palette::Palette {
        background,
        text,
        primary: ACCENT_PRIMARY,
        success: ACCENT_SUCCESS,
        warning: ACCENT_SUCCESS,
        danger: ACCENT_DANGER,
    }
}

pub fn border_color(theme: &Theme, emphasized: bool) -> Color {
    let base = theme.extended_palette().background.base;
    let mix = if emphasized { 0.58 } else { 0.32 };
    with_alpha(lerp_color(base.color, base.text, mix), 1.0)
}

pub fn border(theme: &Theme, emphasized: bool) -> Border {
    Border {
        color: border_color(theme, emphasized),
        width: 1.0,
        ..Default::default()
    }
}

pub fn button_style(theme: &Theme, selected: bool, status: button::Status) -> button::Style {
    use button::Status::{Hovered, Pressed};
    let palette = theme.extended_palette();
    let base = if selected {
        palette.primary.base.color
    } else {
        palette.background.weak.color
    };
    let background = if status == Hovered {
        palette::deviate(base, 0.05)
    } else {
        base
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color: readable_text(background),
        border: border(theme, status == Pressed),
        ..Default::default()
    }
}

pub fn weak_container(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(palette.background.weak.color)),
        text_color: Some(palette.background.base.text),
        border: border(theme, false),
        ..Default::default()
    }
}

pub fn weak_text_style(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(theme.extended_palette().secondary.weak.text),
    }
}

pub fn resize_overlay(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(with_alpha(
            palette.background.base.color,
            0.7,
        ))),
        text_color: Some(palette.background.base.text),
        ..Default::default()
    }
}

pub fn slider_style(theme: &Theme, status: slider::Status) -> slider::Style {
    let palette = theme.extended_palette();

    let track = lerp_color(palette.background.base.color, Color::WHITE, 0.16);
    let filled = lerp_color(palette.primary.base.color, Color::WHITE, 0.10);

    slider::Style {
        rail: slider::Rail {
            backgrounds: (Background::Color(filled), Background::Color(track)),
            border: border(theme, false),
            width: 2.0,
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle { radius: 7.0 },
            background: Background::Color(filled),
            border_color: border_color(theme, status != slider::Status::Active),
            border_width: 1.0,
        },
    }
}
