// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

// Theme colors are sRGB CSS floats. Under iced `web-colors` no gamma
// conversion happens anywhere: sRGB flows raw to a non-sRGB framebuffer.

use iced::border::Border;
use iced::theme::palette::{self, Extended};
use iced::widget::{button, container, slider, text};
use iced::{Background, Color, Theme};

use crate::util::color::{lerp_color, with_alpha};

pub use crate::visuals::palettes::{
    BG_BASE, Palette, background, loudness, oscilloscope, spectrogram, spectrum, stereometer,
    waveform,
};

const TEXT_PRIMARY: Color = Color::from_rgba(0.902, 0.910, 0.925, 1.0);
const TEXT_DARK: Color = Color::from_rgba(0.10, 0.10, 0.10, 1.0);

pub const BORDER_SUBTLE: Color = Color::from_rgba(0.280, 0.288, 0.304, 1.0);
const BORDER_FOCUS: Color = Color::from_rgba(0.520, 0.536, 0.560, 1.0);

const ACCENT_PRIMARY: Color = Color::from_rgba(0.157, 0.157, 0.157, 1.0);
const ACCENT_SUCCESS: Color = Color::from_rgba(0.478, 0.557, 0.502, 1.0);
const ACCENT_DANGER: Color = Color::from_rgba(0.557, 0.478, 0.478, 1.0);

pub fn theme(custom_bg: Option<Color>) -> Theme {
    Theme::custom_with_fn(
        "OpenMeters Monochrome".to_string(),
        palette(custom_bg),
        Extended::generate,
    )
}

fn palette(custom_bg: Option<Color>) -> palette::Palette {
    let background = custom_bg.unwrap_or(BG_BASE);
    let text = if palette::is_dark(background) {
        TEXT_PRIMARY
    } else {
        TEXT_DARK
    };

    palette::Palette {
        background,
        text,
        primary: ACCENT_PRIMARY,
        success: ACCENT_SUCCESS,
        warning: ACCENT_SUCCESS, // monochrome: no distinct warning color
        danger: ACCENT_DANGER,
    }
}

pub fn sharp_border() -> Border {
    Border {
        color: BORDER_SUBTLE,
        width: 1.0,
        radius: 0.0.into(),
    }
}

pub fn focus_border() -> Border {
    Border {
        color: BORDER_FOCUS,
        width: 1.0,
        radius: 0.0.into(),
    }
}

pub fn button_style(theme: &Theme, base: Color, status: button::Status) -> button::Style {
    use button::Status::{Hovered, Pressed};
    let bg = if status == Hovered {
        palette::deviate(base, 0.05)
    } else {
        base
    };
    let border = if status == Pressed {
        focus_border()
    } else {
        sharp_border()
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: theme.extended_palette().background.base.text,
        border,
        ..Default::default()
    }
}

pub fn tab_button_style(theme: &Theme, active: bool, status: button::Status) -> button::Style {
    let pal = theme.extended_palette();
    let base = [pal.background.weak.color, pal.primary.base.color][active as usize];
    button_style(theme, base, status)
}

pub fn weak_container(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(palette.background.weak.color)),
        text_color: Some(palette.background.base.text),
        border: sharp_border(),
        ..Default::default()
    }
}

pub fn weak_text_style(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(theme.extended_palette().secondary.weak.text),
    }
}

pub fn opaque_container(theme: &Theme) -> container::Style {
    let bg = with_alpha(theme.extended_palette().background.base.color, 1.0);
    container::Style {
        background: Some(Background::Color(bg)),
        ..Default::default()
    }
}

pub fn resize_handle_container(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(with_alpha(
            palette.secondary.weak.color,
            0.1,
        ))),
        ..Default::default()
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

pub fn accent_primary() -> Color {
    ACCENT_PRIMARY
}

pub fn slider_style(theme: &Theme, status: slider::Status) -> slider::Style {
    let palette = theme.extended_palette();

    let track = lerp_color(palette.background.base.color, Color::WHITE, 0.16);
    let filled = lerp_color(palette.primary.base.color, Color::WHITE, 0.10);

    let (handle_color, border_color, border_width) = match status {
        slider::Status::Hovered | slider::Status::Dragged => (filled, BORDER_FOCUS, 1.0),
        _ => (filled, BORDER_SUBTLE, 1.0),
    };

    slider::Style {
        rail: slider::Rail {
            backgrounds: (Background::Color(filled), Background::Color(track)),
            border: sharp_border(),
            width: 2.0,
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle { radius: 7.0 },
            background: Background::Color(handle_color),
            border_color,
            border_width,
        },
    }
}
