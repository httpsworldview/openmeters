// Monochrome Iced theme.
//
// GPU palette colors are defined in sRGB. The sRGB framebuffer format handles
// gamma correction automatically, so colors are passed through without conversion.

use iced::border::Border;
use iced::theme::palette::{self, Extended};
use iced::widget::{button, container, scrollable, slider, text};
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

// A GPU visualization palette with colors and optional labels.
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

    // Sets colors, only stores if different from defaults.
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

// Spectrogram heat map: quiet -> loud (5 stops)
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

// Spectrum analyzer gradient: quiet -> loud (6 stops)
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

// dark red (low) -> orange -> green -> cyan -> blue (high)
pub mod waveform {
    use super::*;
    pub const COLORS: [Color; 6] = [
        Color::from_rgba(0.545, 0.000, 0.000, 1.0),
        Color::from_rgba(1.000, 0.259, 0.000, 1.0),
        Color::from_rgba(1.000, 0.412, 0.000, 1.0),
        Color::from_rgba(0.298, 1.000, 0.180, 1.0),
        Color::from_rgba(0.196, 0.804, 1.000, 1.0),
        Color::from_rgba(0.000, 0.000, 1.000, 1.0),
    ];
    pub const LABELS: &[&str] = &["Sub-bass", "->", "->", "->", "->", "Brilliance"];
}

// Oscilloscope trace color (1 stop)
pub mod oscilloscope {
    use super::*;
    pub const COLORS: [Color; 1] = [Color::from_rgba(1.000, 1.000, 1.000, 1.0)];
    pub const LABELS: &[&str] = &["Trace"];
}

// Stereometer (9 stops)
pub mod stereometer {
    use super::*;
    pub const COLORS: [Color; 9] = [
        Color::from_rgba(1.000, 1.000, 1.000, 1.0),
        Color::from_rgba(0.10, 0.10, 0.10, 1.0),
        Color::from_rgba(0.50, 0.50, 0.50, 1.0),
        Color::from_rgba(0.45, 0.65, 0.50, 1.0),
        Color::from_rgba(0.70, 0.35, 0.35, 1.0),
        Color::from_rgba(0.55, 0.45, 0.70, 1.0),
        Color::from_rgba(0.50, 0.60, 0.55, 1.0),
        Color::from_rgba(0.65, 0.55, 0.45, 1.0),
        Color::from_rgba(0.50, 0.50, 0.50, 0.25),
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
        "Grid",
    ];
}

// Loudness meter: background, left_ch_1, left_ch_2, right_fill, guide_line (5 stops)
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

// App background color (1 stop)
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

// Interpolates colors in Oklch space along the hue circle for perceptually
// smooth transitions (e.g., orange -> green passes through yellow).
pub fn mix_colors(a: Color, b: Color, factor: f32) -> Color {
    let t = factor.clamp(0.0, 1.0);
    let (l1, c1, h1) = srgb_to_oklch(a.r, a.g, a.b);
    let (l2, c2, h2) = srgb_to_oklch(b.r, b.g, b.b);

    let l = l1 + (l2 - l1) * t;
    let c = c1 + (c2 - c1) * t;
    let h = interpolate_hue(h1, c1, h2, c2, t);

    let (r, g, b_out) = oklch_to_srgb(l, c, h);
    Color::from_rgba(r, g, b_out, a.a + (b.a - a.a) * t)
}

// Interpolates hue along the shorter arc, handling achromatic colors.
fn interpolate_hue(h1: f32, c1: f32, h2: f32, c2: f32, t: f32) -> f32 {
    const EPSILON: f32 = 1e-6;
    if c1 < EPSILON {
        return h2;
    }
    if c2 < EPSILON {
        return h1;
    }
    let mut delta = h2 - h1;
    if delta > std::f32::consts::PI {
        delta -= std::f32::consts::TAU;
    } else if delta < -std::f32::consts::PI {
        delta += std::f32::consts::TAU;
    }
    h1 + delta * t
}

// Oklch color space conversions (perceptually uniform with hue interpolation)

// Convert sRGB (0-1) to Oklch (lightness, chroma, hue).
#[inline]
fn srgb_to_oklch(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    // sRGB -> linear RGB
    let r_lin = srgb_to_linear(r);
    let g_lin = srgb_to_linear(g);
    let b_lin = srgb_to_linear(b);

    // Linear RGB -> LMS
    let l = 0.412_221_46 * r_lin + 0.536_332_55 * g_lin + 0.051_445_995 * b_lin;
    let m = 0.211_903_5 * r_lin + 0.680_699_5 * g_lin + 0.107_396_96 * b_lin;
    let s = 0.088_302_46 * r_lin + 0.281_718_84 * g_lin + 0.629_978_7 * b_lin;

    // LMS -> Oklab
    let l_cbrt = l.max(0.0).cbrt();
    let m_cbrt = m.max(0.0).cbrt();
    let s_cbrt = s.max(0.0).cbrt();

    let ok_l = 0.210_454_26 * l_cbrt + 0.793_617_8 * m_cbrt - 0.004_072_047 * s_cbrt;
    let ok_a = 1.977_998_5 * l_cbrt - 2.428_592_2 * m_cbrt + 0.450_593_7 * s_cbrt;
    let ok_b = 0.025_904_037 * l_cbrt + 0.782_771_77 * m_cbrt - 0.808_675_77 * s_cbrt;

    // Oklab -> Oklch
    let chroma = (ok_a * ok_a + ok_b * ok_b).sqrt();
    let hue = ok_b.atan2(ok_a);
    (ok_l, chroma, hue)
}

// Convert Oklch (lightness, chroma, hue) to sRGB (0-1), clamped to valid range.
#[inline]
fn oklch_to_srgb(l: f32, c: f32, h: f32) -> (f32, f32, f32) {
    // Oklch -> Oklab
    let ok_a = c * h.cos();
    let ok_b = c * h.sin();

    // Oklab -> LMS
    let l_cubed = l + 0.396_337_78 * ok_a + 0.215_803_76 * ok_b;
    let m_cubed = l - 0.105_561_346 * ok_a - 0.063_854_17 * ok_b;
    let s_cubed = l - 0.089_484_18 * ok_a - 1.291_485_5 * ok_b;

    let l_lin = l_cubed * l_cubed * l_cubed;
    let m_lin = m_cubed * m_cubed * m_cubed;
    let s_lin = s_cubed * s_cubed * s_cubed;

    // LMS -> linear RGB
    let r_lin = 4.076_741_7 * l_lin - 3.307_711_6 * m_lin + 0.230_969_94 * s_lin;
    let g_lin = -1.268_438 * l_lin + 2.609_757_4 * m_lin - 0.341_319_4 * s_lin;
    let b_lin = -0.004_196_086 * l_lin - 0.703_418_6 * m_lin + 1.707_614_7 * s_lin;

    // Linear RGB -> sRGB
    (
        linear_to_srgb(r_lin).clamp(0.0, 1.0),
        linear_to_srgb(g_lin).clamp(0.0, 1.0),
        linear_to_srgb(b_lin).clamp(0.0, 1.0),
    )
}

#[inline]
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
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

// Converts a color to `[f32; 4]` RGBA array for GPU pipelines.
#[inline]
pub fn color_to_rgba(color: Color) -> [f32; 4] {
    [color.r, color.g, color.b, color.a]
}

// Converts a normalized channel in [0.0, 1.0] to u8.
#[inline]
pub fn f32_to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

// Compares two colors for approximate equality.
#[inline]
pub fn colors_equal(a: Color, b: Color) -> bool {
    const EPSILON: f32 = 1e-4;
    (a.r - b.r).abs() <= EPSILON
        && (a.g - b.g).abs() <= EPSILON
        && (a.b - b.b).abs() <= EPSILON
        && (a.a - b.a).abs() <= EPSILON
}

// Compares two color slices for equality.
#[inline]
pub fn palettes_equal(a: &[Color], b: &[Color]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| colors_equal(*x, *y))
}

// Samples a gradient at position `t` (0.0 to 1.0) using Oklch interpolation.
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
