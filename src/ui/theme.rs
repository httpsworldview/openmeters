// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

// Monochrome Iced theme.
//
// GPU palette colors are defined in sRGB. The sRGB framebuffer format handles
// gamma correction automatically, so colors are passed through without conversion.

use iced::border::Border;
use iced::theme::palette::{self, Extended};
use iced::widget::{button, container, scrollable, slider, text};
use iced::{Background, Color, Theme};

// Re-exports: color math (canonical home: util/color)
pub use crate::util::color::{
    colors_equal, default_spreads, f32_to_u8, mix_colors, sample_gradient_positioned,
    sanitize_stop_positions, sanitize_stop_spreads, uniform_positions, with_alpha,
};

// Re-exports: palette data (canonical home: visuals/palettes)
pub use crate::visuals::palettes::{
    BG_BASE, Palette, background, loudness, oscilloscope, spectrogram, spectrum, stereometer,
    waveform,
};

const TEXT_PRIMARY: Color = Color::from_rgba(0.902, 0.910, 0.925, 1.0);
const TEXT_DARK: Color = Color::from_rgba(0.10, 0.10, 0.10, 1.0);

const BORDER_SUBTLE: Color = Color::from_rgba(0.280, 0.288, 0.304, 1.0);
const BORDER_FOCUS: Color = Color::from_rgba(0.520, 0.536, 0.560, 1.0);

// Accent colors
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
        warning: ACCENT_SUCCESS,
        danger: ACCENT_DANGER,
    }
}

// styling helpers

// Standard sharp border for buttons and containers.
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
    let palette = theme.extended_palette();
    let mut style = button::Style {
        background: Some(Background::Color(base)),
        text_color: palette.background.base.text,
        border: sharp_border(),
        ..Default::default()
    };

    match status {
        button::Status::Hovered => {
            let hover = palette::deviate(base, 0.05);
            style.background = Some(Background::Color(hover));
        }
        button::Status::Pressed => {
            style.border = focus_border();
        }
        _ => {}
    }

    style
}

pub fn tab_button_style(theme: &Theme, active: bool, status: button::Status) -> button::Style {
    let palette = theme.extended_palette();
    let mut base = if active {
        palette.primary.base.color
    } else {
        mix_colors(palette.background.base.color, Color::WHITE, 0.2)
    };
    base.a = 1.0;
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
    let palette = theme.extended_palette();
    let mut bg = palette.background.base.color;
    bg.a = 1.0;
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

    let track = mix_colors(palette.background.base.color, Color::WHITE, 0.16);
    let filled = mix_colors(palette.primary.base.color, Color::WHITE, 0.10);

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

// Transparent scrollable with no visible rails or scrollers.
pub fn transparent_scrollable(_theme: &Theme, _status: scrollable::Status) -> scrollable::Style {
    let transparent_scroller = scrollable::Scroller {
        background: Background::Color(Color::TRANSPARENT),
        border: Border::default(),
    };
    let transparent_rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: transparent_scroller,
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: transparent_rail,
        horizontal_rail: transparent_rail,
        gap: None,
        auto_scroll: scrollable::AutoScroll {
            background: Background::Color(Color::TRANSPARENT),
            border: Border::default(),
            shadow: iced::Shadow::default(),
            icon: Color::TRANSPARENT,
        },
    }
}
