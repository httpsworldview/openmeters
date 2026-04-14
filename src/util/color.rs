// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use iced::Color;

pub const EPSILON: f32 = 1e-4;

#[inline]
pub fn f32_to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

pub const fn hex(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color::from_rgba(
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    )
}

#[inline]
pub fn colors_equal(a: Color, b: Color) -> bool {
    (a.r - b.r).abs() <= EPSILON
        && (a.g - b.g).abs() <= EPSILON
        && (a.b - b.b).abs() <= EPSILON
        && (a.a - b.a).abs() <= EPSILON
}

#[inline]
pub fn palettes_equal(a: &[Color], b: &[Color]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| colors_equal(*x, *y))
}

// Packs `iced::Color` to `[f32; 4]` for GPU upload. Delegates to iced's `pack`
// so the `web-colors` feature flag controls linearization.
#[inline]
pub fn color_to_rgba(color: Color) -> [f32; 4] {
    iced_wgpu::graphics::color::pack(color).components()
}

// sRGB-space lerp. Only correct for inputs close in value (theme tweens, track
// fills); distant-hue interpolation belongs on the GPU palette pipeline.
#[inline]
pub fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
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

#[inline]
pub fn rgba_with_alpha(color: [f32; 4], alpha: f32) -> [f32; 4] {
    [color[0], color[1], color[2], alpha]
}

#[inline]
pub fn sample_gradient(palette: &[Color], t: f32) -> Color {
    let n = palette.len();
    match n {
        0 => Color::BLACK,
        1 => palette[0],
        _ => {
            let pos = t.clamp(0.0, 1.0) * (n - 1) as f32;
            let i = (pos as usize).min(n - 2);
            lerp_color(palette[i], palette[i + 1], pos - i as f32)
        }
    }
}

#[inline]
pub fn sample_rgba_gradient(palette: &[[f32; 4]], t: f32) -> [f32; 4] {
    let n = palette.len();
    if n < 2 {
        return palette.first().copied().unwrap_or([0.0; 4]);
    }
    let pos = t.clamp(0.0, 1.0) * (n - 1) as f32;
    let i = (pos as usize).min(n - 2);
    let f = pos - i as f32;
    std::array::from_fn(|c| palette[i][c] + (palette[i + 1][c] - palette[i][c]) * f)
}

pub fn default_spreads(count: usize) -> Vec<f32> {
    vec![1.0; count]
}

pub fn sanitize_stop_positions(raw: Option<&[f32]>, defaults: &[f32]) -> Vec<f32> {
    let count = defaults.len();
    if count < 2 {
        return vec![0.0; count];
    }
    let mut out = defaults.to_vec();
    let end = count - 1;
    let internals = count - 2;

    // Accept either the full row (endpoints included, discarded here) or just
    // the interior stops. Anything else falls back to defaults.
    if let Some(raw) = raw.filter(|r| r.len() == count || r.len() == internals) {
        let start = if raw.len() == count { 1 } else { 0 };
        out[1..end].copy_from_slice(&raw[start..start + internals]);
    }

    out[0] = 0.0;
    out[end] = 1.0;

    for i in 1..end {
        let value = if out[i].is_finite() {
            out[i]
        } else {
            defaults[i]
        };
        let min = (out[i - 1] + EPSILON).min(1.0);
        let max = (1.0 - EPSILON * (end - i) as f32).max(min);
        out[i] = value.clamp(min, max);
    }

    out
}

pub fn sanitize_stop_spreads(raw: Option<&[f32]>, count: usize) -> Vec<f32> {
    let mut out = default_spreads(count);
    let Some(raw) = raw else {
        return out;
    };
    if raw.len() != count {
        return out;
    }
    for (dst, &value) in out.iter_mut().zip(raw.iter()) {
        *dst = if value.is_finite() {
            value.clamp(0.2, 5.0)
        } else {
            1.0
        };
    }
    out
}

// Returns (lo, hi, spread-adjusted factor) for the gradient segment containing `t`.
pub fn find_segment(
    positions: &[f32],
    spreads: &[f32],
    t: f32,
    count: usize,
) -> (usize, usize, f32) {
    let t = t.clamp(0.0, 1.0);
    if count < 2 {
        return (0, 0, 0.0);
    }
    if positions.len() < count {
        let pos = t * (count - 1) as f32;
        let lo = (pos.floor() as usize).min(count - 2);
        let hi = lo + 1;
        let linear = (pos - lo as f32).clamp(0.0, 1.0);
        return (lo, hi, interpolate_with_spreads(linear, spreads, lo, hi));
    }
    for i in 0..count - 1 {
        if t <= positions[i + 1] || i == count - 2 {
            let span = (positions[i + 1] - positions[i]).max(f32::EPSILON);
            let linear = ((t - positions[i]) / span).clamp(0.0, 1.0);
            return (
                i,
                i + 1,
                interpolate_with_spreads(linear, spreads, i, i + 1),
            );
        }
    }
    (count - 2, count - 1, 1.0)
}

#[inline]
fn interpolate_with_spreads(linear: f32, spreads: &[f32], lo: usize, hi: usize) -> f32 {
    let sl = spreads.get(lo).copied().unwrap_or(1.0);
    let sr = spreads.get(hi).copied().unwrap_or(1.0);
    if (sl - 1.0).abs() < EPSILON && (sr - 1.0).abs() < EPSILON {
        linear
    } else {
        linear.powf(sl / sr).clamp(0.0, 1.0)
    }
}
