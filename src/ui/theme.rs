//! Monochrome Iced theme.
//!
//! GPU palette colors are defined in sRGB. The sRGB framebuffer format handles
//! gamma correction automatically, so colors are passed through without conversion.

use iced::border::Border;
use iced::theme::palette::{self, Extended};
use iced::widget::{button, container, slider, text};
use iced::{Background, Color, Theme};

// Core palette stops
// Slightly neutralized base to reduce blue cast in derived weak tones.
pub const BG_BASE: Color = Color::from_rgba(0.065, 0.065, 0.065, 1.0);

const TEXT_PRIMARY: Color = Color::from_rgba(0.902, 0.910, 0.925, 1.0);
const TEXT_DARK: Color = Color::from_rgba(0.10, 0.10, 0.10, 1.0);

const BORDER_SUBTLE: Color = Color::from_rgba(0.280, 0.288, 0.304, 1.0);
const BORDER_FOCUS: Color = Color::from_rgba(0.520, 0.536, 0.560, 1.0);

// Accent colors
const ACCENT_PRIMARY: Color = Color::from_rgba(0.157, 0.157, 0.157, 1.0);
const ACCENT_SUCCESS: Color = Color::from_rgba(0.478, 0.557, 0.502, 1.0);
const ACCENT_DANGER: Color = Color::from_rgba(0.557, 0.478, 0.478, 1.0);

// Unified GPU Palette system

/// A GPU visualization palette with colors and optional labels.
#[derive(Debug, Clone, Default)]
pub struct Palette {
    colors: Vec<Color>,
    defaults: &'static [Color],
    labels: &'static [&'static str],
}

impl Palette {
    pub const fn new(defaults: &'static [Color], labels: &'static [&'static str]) -> Self {
        Self {
            colors: Vec::new(),
            defaults,
            labels,
        }
    }

    #[inline]
    pub fn colors(&self) -> &[Color] {
        if self.colors.is_empty() {
            self.defaults
        } else {
            &self.colors
        }
    }

    #[inline]
    pub fn labels(&self) -> &'static [&'static str] {
        self.labels
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.defaults.len()
    }

    /// Sets colors, only stores if different from defaults.
    pub fn set(&mut self, colors: &[Color]) {
        self.colors.clear();
        if colors.len() == self.defaults.len() && !palettes_equal(colors, self.defaults) {
            self.colors.extend_from_slice(colors);
        }
    }

    pub fn reset(&mut self) {
        self.colors.clear();
    }

    #[inline]
    pub fn is_default(&self) -> bool {
        self.colors.is_empty() || palettes_equal(&self.colors, self.defaults)
    }
}

// Default palette definitions

/// Spectrogram heat map: quiet -> loud (5 stops)
pub mod spectrogram {
    use super::*;
    pub const COLORS: [Color; 5] = [
        Color::from_rgba(0.000, 0.000, 0.000, 0.0),
        Color::from_rgba(0.218, 0.106, 0.332, 1.0),
        Color::from_rgba(0.609, 0.000, 0.000, 1.0),
        Color::from_rgba(1.000, 0.737, 0.353, 1.0),
        Color::from_rgba(1.000, 1.000, 1.000, 1.0),
    ];
    pub const LABELS: &[&str] = &["Quietest", "->", "->", "->", "Loud"];
}

/// Spectrum analyzer gradient: quiet -> loud (6 stops)
pub mod spectrum {
    use super::*;
    pub const COLORS: [Color; 6] = [
        Color::from_rgba(0.000, 0.000, 0.000, 0.0),
        Color::from_rgba(0.218, 0.106, 0.332, 1.0),
        Color::from_rgba(0.609, 0.000, 0.000, 1.0),
        Color::from_rgba(0.906, 0.485, 0.000, 1.0),
        Color::from_rgba(1.000, 0.737, 0.353, 1.0),
        Color::from_rgba(1.000, 1.000, 0.000, 1.0),
    ];
    pub const LABELS: &[&str] = &["Floor", "Low", "Low-Mid", "Mid", "High", "Peak"];
}

/// Waveform display: low, mid, high frequency bands (3 stops)
pub mod waveform {
    use super::*;
    pub const COLORS: [Color; 3] = [
        Color::from_rgba(0.800, 0.200, 0.100, 1.0),
        Color::from_rgba(1.000, 0.600, 0.100, 1.0),
        Color::from_rgba(0.400, 0.300, 0.900, 1.0),
    ];
    pub const LABELS: &[&str] = &["Low", "Mid", "High"];
}

/// Oscilloscope trace color (1 stop)
pub mod oscilloscope {
    use super::*;
    pub const COLORS: [Color; 1] = [Color::from_rgba(1.000, 1.000, 1.000, 1.0)];
    pub const LABELS: &[&str] = &["Trace"];
}

/// Stereometer (8 stops)
pub mod stereometer {
    use super::*;
    pub const COLORS: [Color; 8] = [
        Color::from_rgba(1.000, 1.000, 1.000, 1.0),
        Color::from_rgba(0.10, 0.10, 0.10, 1.0),
        Color::from_rgba(0.50, 0.50, 0.50, 1.0),
        Color::from_rgba(0.45, 0.65, 0.50, 1.0),
        Color::from_rgba(0.70, 0.35, 0.35, 1.0),
        Color::from_rgba(0.55, 0.45, 0.70, 1.0),
        Color::from_rgba(0.50, 0.60, 0.55, 1.0),
        Color::from_rgba(0.65, 0.55, 0.45, 1.0),
    ];
    pub const LABELS: &[&str] = &[
        "Trace",
        "Corr BG",
        "Corr Center",
        "Corr +",
        "Corr -",
        "Low",
        "Mid",
        "High",
    ];
}

/// Loudness meter: background, left_ch_1, left_ch_2, right_fill, guide_line (5 stops)
pub mod loudness {
    use super::*;
    pub const COLORS: [Color; 5] = [
        Color::from_rgba(0.161, 0.161, 0.161, 1.0),
        Color::from_rgba(0.626, 0.665, 0.680, 1.0),
        Color::from_rgba(0.584, 0.618, 0.650, 1.0),
        Color::from_rgba(0.701, 0.767, 0.735, 1.0),
        Color::from_rgba(0.735, 0.748, 0.774, 0.88),
    ];
    pub const LABELS: &[&str] = &["Background", "Left 1", "Left 2", "Right", "Guide"];
}

/// App background color (1 stop)
pub mod background {
    use super::*;
    pub const COLORS: [Color; 1] = [BG_BASE];
    pub const LABELS: &[&str] = &["Background"];
}

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

/// Standard sharp border for buttons and containers.
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

pub fn accent_primary() -> Color {
    ACCENT_PRIMARY
}

pub fn mix_colors(a: Color, b: Color, factor: f32) -> Color {
    let t = factor.clamp(0.0, 1.0);
    Color::from_rgba(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
        a.a + (b.a - a.a) * t,
    )
}

pub fn with_alpha(color: Color, alpha: f32) -> Color {
    Color {
        a: alpha.clamp(0.0, 1.0),
        ..color
    }
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

/// Converts a color to `[f32; 4]` RGBA array for GPU pipelines.
#[inline]
pub fn color_to_rgba(color: Color) -> [f32; 4] {
    [color.r, color.g, color.b, color.a]
}

/// Compares two colors for approximate equality.
#[inline]
pub fn colors_equal(a: Color, b: Color) -> bool {
    const EPSILON: f32 = 1e-4;
    (a.r - b.r).abs() <= EPSILON
        && (a.g - b.g).abs() <= EPSILON
        && (a.b - b.b).abs() <= EPSILON
        && (a.a - b.a).abs() <= EPSILON
}

/// Compares two color slices for equality.
#[inline]
pub fn palettes_equal(a: &[Color], b: &[Color]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| colors_equal(*x, *y))
}

/// Samples a gradient at position `t` (0.0 to 1.0) using linear interpolation.
#[inline]
pub fn sample_gradient(palette: &[Color], t: f32) -> Color {
    let n = palette.len();
    match n {
        0 => ACCENT_PRIMARY,
        1 => palette[0],
        _ => {
            let pos = t.clamp(0.0, 1.0) * (n - 1) as f32;
            let i = (pos as usize).min(n - 2);
            mix_colors(palette[i], palette[i + 1], pos - i as f32)
        }
    }
}
