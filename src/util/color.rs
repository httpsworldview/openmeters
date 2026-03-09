// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

// Pure color math and gradient utilities.

use iced::Color;

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
    const EPSILON: f32 = 1e-4;
    (a.r - b.r).abs() <= EPSILON
        && (a.g - b.g).abs() <= EPSILON
        && (a.b - b.b).abs() <= EPSILON
        && (a.a - b.a).abs() <= EPSILON
}

#[inline]
pub fn palettes_equal(a: &[Color], b: &[Color]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| colors_equal(*x, *y))
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

pub fn with_alpha(color: Color, alpha: f32) -> Color {
    Color {
        a: alpha.clamp(0.0, 1.0),
        ..color
    }
}

#[inline]
pub fn color_to_rgba(color: Color) -> [f32; 4] {
    [color.r, color.g, color.b, color.a]
}

// Samples a gradient at position `t` (0.0 to 1.0) using Oklch interpolation.
#[inline]
pub fn sample_gradient(palette: &[Color], t: f32) -> Color {
    let n = palette.len();
    match n {
        0 => Color::BLACK,
        1 => palette[0],
        _ => {
            let pos = t.clamp(0.0, 1.0) * (n - 1) as f32;
            let i = (pos as usize).min(n - 2);
            mix_colors(palette[i], palette[i + 1], pos - i as f32)
        }
    }
}

// Samples a gradient with non-uniform stop positions using sRGB linear
// interpolation (matches the GPU palette LUT).
pub fn sample_gradient_positioned(
    colors: &[Color],
    positions: &[f32],
    spreads: &[f32],
    t: f32,
) -> Color {
    let n = colors.len();
    if n == 0 {
        return Color::BLACK;
    }
    if n == 1 {
        return colors[0];
    }
    let (lo, hi, f) = find_segment(positions, spreads, t, n);
    Color {
        r: colors[lo].r + (colors[hi].r - colors[lo].r) * f,
        g: colors[lo].g + (colors[hi].g - colors[lo].g) * f,
        b: colors[lo].b + (colors[hi].b - colors[lo].b) * f,
        a: colors[lo].a + (colors[hi].a - colors[lo].a) * f,
    }
}

pub fn uniform_positions(count: usize) -> Vec<f32> {
    if count <= 1 {
        return vec![0.0; count];
    }
    (0..count).map(|i| i as f32 / (count - 1) as f32).collect()
}

pub fn default_spreads(count: usize) -> Vec<f32> {
    vec![1.0; count]
}

pub fn sanitize_stop_positions(raw: Option<&[f32]>, count: usize) -> Vec<f32> {
    if count < 2 {
        return vec![0.0; count];
    }
    let mut out = uniform_positions(count);
    let Some(raw) = raw else {
        return out;
    };

    let end = count - 1;
    let eps = 1e-4_f32;
    let internals = count - 2;

    if raw.len() == count {
        out[1..end].copy_from_slice(&raw[1..end]);
    } else if raw.len() == internals {
        for (i, &value) in raw.iter().enumerate() {
            out[i + 1] = value;
        }
    } else {
        return out;
    }

    out[0] = 0.0;
    out[end] = 1.0;

    for i in 1..end {
        let fallback = i as f32 / end as f32;
        let value = if out[i].is_finite() { out[i] } else { fallback };
        let min = (out[i - 1] + eps).min(1.0);
        let max = (1.0 - eps * (end - i) as f32).max(min);
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

// Finds the gradient segment for a given `t` and returns (lo, hi, interpolation_factor)
// with the spread curve applied. Shared between CPU preview and GPU LUT builder.
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

// --- private helpers ---

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

#[inline]
fn srgb_to_oklch(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let r_lin = srgb_to_linear(r);
    let g_lin = srgb_to_linear(g);
    let b_lin = srgb_to_linear(b);

    let l = 0.412_221_46 * r_lin + 0.536_332_55 * g_lin + 0.051_445_995 * b_lin;
    let m = 0.211_903_5 * r_lin + 0.680_699_5 * g_lin + 0.107_396_96 * b_lin;
    let s = 0.088_302_46 * r_lin + 0.281_718_84 * g_lin + 0.629_978_7 * b_lin;

    let l_cbrt = l.max(0.0).cbrt();
    let m_cbrt = m.max(0.0).cbrt();
    let s_cbrt = s.max(0.0).cbrt();

    let ok_l = 0.210_454_26 * l_cbrt + 0.793_617_8 * m_cbrt - 0.004_072_047 * s_cbrt;
    let ok_a = 1.977_998_5 * l_cbrt - 2.428_592_2 * m_cbrt + 0.450_593_7 * s_cbrt;
    let ok_b = 0.025_904_037 * l_cbrt + 0.782_771_77 * m_cbrt - 0.808_675_77 * s_cbrt;

    let chroma = (ok_a * ok_a + ok_b * ok_b).sqrt();
    let hue = ok_b.atan2(ok_a);
    (ok_l, chroma, hue)
}

#[inline]
fn oklch_to_srgb(l: f32, c: f32, h: f32) -> (f32, f32, f32) {
    let ok_a = c * h.cos();
    let ok_b = c * h.sin();

    let l_cubed = l + 0.396_337_78 * ok_a + 0.215_803_76 * ok_b;
    let m_cubed = l - 0.105_561_346 * ok_a - 0.063_854_17 * ok_b;
    let s_cubed = l - 0.089_484_18 * ok_a - 1.291_485_5 * ok_b;

    let l_lin = l_cubed * l_cubed * l_cubed;
    let m_lin = m_cubed * m_cubed * m_cubed;
    let s_lin = s_cubed * s_cubed * s_cubed;

    let r_lin = 4.076_741_7 * l_lin - 3.307_711_6 * m_lin + 0.230_969_94 * s_lin;
    let g_lin = -1.268_438 * l_lin + 2.609_757_4 * m_lin - 0.341_319_4 * s_lin;
    let b_lin = -0.004_196_086 * l_lin - 0.703_418_6 * m_lin + 1.707_614_7 * s_lin;

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

#[inline]
fn interpolate_with_spreads(linear: f32, spreads: &[f32], lo: usize, hi: usize) -> f32 {
    let sl = spreads.get(lo).copied().unwrap_or(1.0);
    let sr = spreads.get(hi).copied().unwrap_or(1.0);
    if (sl - 1.0).abs() < 1e-4 && (sr - 1.0).abs() < 1e-4 {
        linear
    } else {
        linear.powf(sl / sr).clamp(0.0, 1.0)
    }
}
