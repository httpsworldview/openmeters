// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{finite_or, lerp};
use iced::Color;

pub const EPSILON: f32 = 1e-4;
pub const STOP_SPREAD_MIN: f32 = 0.2;
pub const STOP_SPREAD_MAX: f32 = 5.0;

pub fn parse_hex_color(value: &str) -> Result<Color, &'static str> {
    const ERROR: &str = "expected 3, 4, 6 or 8 hex digits with optional #";
    let hex = value.strip_prefix('#').unwrap_or(value);
    // Iced slices at byte offsets and accepts signs, so only pass it hex digits.
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ERROR);
    }
    hex.parse().map_err(|_| ERROR)
}

/// Canonical uppercase `#RRGGBB`, or `#RRGGBBAA` when translucent.
pub fn to_hex(c: Color) -> String {
    let [r, g, b, a] = c.into_rgba8();
    if a == 255 {
        format!("#{r:02X}{g:02X}{b:02X}")
    } else {
        format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
    }
}

/// Hue and saturation in `0..=1`; `previous` supplies them where they are undefined.
pub fn to_hsv(c: Color, previous: [f32; 3]) -> [f32; 3] {
    let v = c.r.max(c.g).max(c.b);
    let chroma = v - c.r.min(c.g).min(c.b);
    if v <= 0.0 {
        return [previous[0], previous[1], 0.0];
    }
    if chroma <= 0.0 {
        return [previous[0], 0.0, v];
    }
    let sector = if v == c.r {
        (c.g - c.b) / chroma
    } else if v == c.g {
        2.0 + (c.b - c.r) / chroma
    } else {
        4.0 + (c.r - c.g) / chroma
    };
    [(sector / 6.0).rem_euclid(1.0), chroma / v, v]
}

pub fn from_hsv([h, s, v]: [f32; 3], alpha: f32) -> Color {
    let channel = |n: f32| {
        let k = (n + h * 6.0) % 6.0;
        v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    Color::from_rgba(channel(5.0), channel(3.0), channel(1.0), alpha)
}

pub fn colors_equal(a: Color, b: Color) -> bool {
    (a.r - b.r).abs() <= EPSILON
        && (a.g - b.g).abs() <= EPSILON
        && (a.b - b.b).abs() <= EPSILON
        && (a.a - b.a).abs() <= EPSILON
}

pub fn palettes_equal(a: &[Color], b: &[Color]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| colors_equal(*x, *y))
}

pub fn color_to_rgba(color: Color) -> [f32; 4] {
    iced_wgpu::graphics::color::pack(color).components()
}

pub fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color::from_rgba(
        lerp(a.r, b.r, t),
        lerp(a.g, b.g, t),
        lerp(a.b, b.b, t),
        lerp(a.a, b.a, t),
    )
}

pub fn with_alpha(color: Color, alpha: f32) -> Color {
    Color {
        a: alpha.clamp(0.0, 1.0),
        ..color
    }
}

pub fn rgba_with_alpha(color: [f32; 4], alpha: f32) -> [f32; 4] {
    [color[0], color[1], color[2], alpha.clamp(0.0, 1.0)]
}

pub fn sample_rgba_gradient(palette: &[[f32; 4]], t: f32) -> [f32; 4] {
    if palette.len() < 2 {
        return palette.first().copied().unwrap_or([0.0; 4]);
    }
    let pos = t.clamp(0.0, 1.0) * (palette.len() - 1) as f32;
    let i = (pos as usize).min(palette.len() - 2);
    let [a, b] = [palette[i], palette[i + 1]];
    std::array::from_fn(|ch| lerp(a[ch], b[ch], pos - i as f32))
}

/// The ramp stops around `t` and the blend between them.
pub fn ramp_segment(positions: &[f32], spreads: &[f32], t: f32) -> (usize, usize, f32) {
    let count = positions.len();
    let t = t.clamp(0.0, 1.0);
    let hi = positions
        .partition_point(|&pos| pos < t)
        .clamp(1, count - 1);
    let lo = hi - 1;
    let linear =
        ((t - positions[lo]) / (positions[hi] - positions[lo]).max(f32::EPSILON)).clamp(0.0, 1.0);
    let (sl, sr) = (spreads[lo], spreads[hi]);
    let f = if (sl - 1.0).abs() < EPSILON && (sr - 1.0).abs() < EPSILON {
        linear
    } else {
        linear.powf(sl / sr).clamp(0.0, 1.0)
    };
    (lo, hi, f)
}

pub fn sanitize_stop_positions(raw: Option<&[f32]>, defaults: &[f32]) -> Vec<f32> {
    let count = defaults.len();
    if count < 2 {
        return vec![0.0; count];
    }
    let mut out = defaults.to_vec();
    let end = count - 1;
    let internals = count - 2;

    if let Some(raw) = raw.filter(|r| r.len() == count || r.len() == internals) {
        let start = usize::from(raw.len() == count);
        out[1..end].copy_from_slice(&raw[start..start + internals]);
    }

    out[0] = 0.0;
    out[end] = 1.0;

    for i in 1..end {
        let value = finite_or(out[i], defaults[i]);
        let min = (out[i - 1] + EPSILON).min(1.0);
        let max = (1.0 - EPSILON * (end - i) as f32).max(min);
        out[i] = value.clamp(min, max);
    }

    out
}

pub fn sanitize_stop_spreads(raw: Option<&[f32]>, count: usize) -> Vec<f32> {
    match raw.filter(|raw| raw.len() == count) {
        Some(raw) => raw
            .iter()
            .map(|&value| finite_or(value, 1.0).clamp(STOP_SPREAD_MIN, STOP_SPREAD_MAX))
            .collect(),
        None => vec![1.0; count],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_input_preserves_supported_syntax_and_alpha() {
        for (input, rgba) in [
            ("12abEF", [0x12, 0xAB, 0xEF, 0xFF]),
            ("#aBc8", [0xAA, 0xBB, 0xCC, 0x88]),
        ] {
            assert_eq!(parse_hex_color(input).unwrap().into_rgba8(), rgba);
        }
        for input in ["##fff", " fff", "+f+f+f", "#a\u{00e9}"] {
            assert!(parse_hex_color(input).is_err(), "{input:?}");
        }
    }

    #[test]
    fn hsv_round_trips_and_keeps_undefined_hue() {
        let previous = [0.25, 0.75, 0.5];
        for (rgb, hsv) in [
            ([1.0, 0.0, 0.0], [0.0, 1.0, 1.0]),
            ([0.0, 0.0, 1.0], [4.0 / 6.0, 1.0, 1.0]),
            ([0.0, 0.5, 0.5], [0.5, 1.0, 0.5]),
            ([0.5, 0.25, 0.5], [5.0 / 6.0, 0.5, 0.5]),
            ([0.5, 0.5, 0.5], [0.25, 0.0, 0.5]),
            ([0.0, 0.0, 0.0], [0.25, 0.75, 0.0]),
        ] {
            let color = Color::from_rgba(rgb[0], rgb[1], rgb[2], 0.5);
            let actual = to_hsv(color, previous);
            assert!(
                actual.iter().zip(hsv).all(|(a, e)| (a - e).abs() < EPSILON),
                "{rgb:?}"
            );
            assert!(colors_equal(from_hsv(actual, 0.5), color), "{rgb:?}");
        }
    }

    // Shaders require raw sRGB packing (iced's web-colors feature).
    #[test]
    fn packed_colors_keep_raw_srgb_components() {
        let color = Color::from_rgb8(0x80, 0x40, 0xC0);
        assert_eq!(
            color_to_rgba(color),
            [128.0 / 255.0, 64.0 / 255.0, 192.0 / 255.0, 1.0]
        );
    }
}
