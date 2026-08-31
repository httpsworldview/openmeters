// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{finite_or, lerp};
use iced::Color;

pub const EPSILON: f32 = 1e-4;
pub const STOP_SPREAD_MIN: f32 = 0.2;
pub const STOP_SPREAD_MAX: f32 = 5.0;

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
    let mut out = vec![1.0; count];
    let Some(raw) = raw.filter(|raw| raw.len() == count) else {
        return out;
    };
    for (dst, &value) in out.iter_mut().zip(raw.iter()) {
        *dst = finite_or(value, 1.0).clamp(STOP_SPREAD_MIN, STOP_SPREAD_MAX);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // packed colors must stay raw sRGB. Without it iced linearizes on
    // pack and fucks up every rendered color.
    #[test]
    fn packed_colors_keep_raw_srgb_components() {
        let color = Color::from_rgb8(0x80, 0x40, 0xC0);
        assert_eq!(
            color_to_rgba(color),
            [128.0 / 255.0, 64.0 / 255.0, 192.0 / 255.0, 1.0]
        );
    }
}
