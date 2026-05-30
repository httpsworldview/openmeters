// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use iced::Color;

pub const EPSILON: f32 = 1e-4;
pub const STOP_SPREAD_MIN: f32 = 0.2;
pub const STOP_SPREAD_MAX: f32 = 5.0;

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
        (b.r - a.r).mul_add(t, a.r),
        (b.g - a.g).mul_add(t, a.g),
        (b.b - a.b).mul_add(t, a.b),
        (b.a - a.a).mul_add(t, a.a),
    )
}

pub fn with_alpha(color: Color, alpha: f32) -> Color {
    Color {
        a: alpha.clamp(0.0, 1.0),
        ..color
    }
}

pub fn rgba_with_alpha(color: [f32; 4], alpha: f32) -> [f32; 4] {
    [color[0], color[1], color[2], alpha]
}

fn gradient_segment(count: usize, t: f32) -> Option<(usize, f32)> {
    (count >= 2).then(|| {
        let pos = t.clamp(0.0, 1.0) * (count - 1) as f32;
        let i = (pos as usize).min(count - 2);
        (i, pos - i as f32)
    })
}

pub fn sample_gradient(palette: &[Color], t: f32) -> Color {
    match gradient_segment(palette.len(), t) {
        Some((i, f)) => lerp_color(palette[i], palette[i + 1], f),
        None => palette.first().copied().unwrap_or(Color::BLACK),
    }
}

pub fn sample_rgba_gradient(palette: &[[f32; 4]], t: f32) -> [f32; 4] {
    match gradient_segment(palette.len(), t) {
        Some((i, f)) => {
            std::array::from_fn(|c| (palette[i + 1][c] - palette[i][c]).mul_add(f, palette[i][c]))
        }
        None => palette.first().copied().unwrap_or([0.0; 4]),
    }
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
    let mut out = vec![1.0; count];
    let Some(raw) = raw.filter(|raw| raw.len() == count) else {
        return out;
    };
    for (dst, &value) in out.iter_mut().zip(raw.iter()) {
        *dst = if value.is_finite() {
            value.clamp(STOP_SPREAD_MIN, STOP_SPREAD_MAX)
        } else {
            1.0
        };
    }
    out
}

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
        let (lo, linear) = gradient_segment(count, t).unwrap_or((0, 0.0));
        return (
            lo,
            lo + 1,
            interpolate_with_spreads(linear, spreads, lo, lo + 1),
        );
    }
    let hi = positions[..count]
        .partition_point(|&pos| pos < t)
        .clamp(1, count - 1);
    let lo = hi - 1;
    let span = (positions[hi] - positions[lo]).max(f32::EPSILON);
    let linear = ((t - positions[lo]) / span).clamp(0.0, 1.0);
    (lo, hi, interpolate_with_spreads(linear, spreads, lo, hi))
}

fn interpolate_with_spreads(linear: f32, spreads: &[f32], lo: usize, hi: usize) -> f32 {
    let sl = spreads.get(lo).copied().unwrap_or(1.0);
    let sr = spreads.get(hi).copied().unwrap_or(1.0);
    if (sl - 1.0).abs() < EPSILON && (sr - 1.0).abs() < EPSILON {
        linear
    } else {
        linear.powf(sl / sr).clamp(0.0, 1.0)
    }
}
