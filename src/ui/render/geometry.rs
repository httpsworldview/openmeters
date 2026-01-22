//! Geometry utilities for rendering.

use crate::ui::render::common::{ClipTransform, SdfVertex};

const MIN_SEG: f32 = 0.1;
const MAX_MITER: f32 = 4.0;

/// Default feather distance for antialiased lines.
pub const DEFAULT_FEATHER: f32 = 1.0;

/// Joins triangle strips with degenerate triangles for batched draw calls.
pub fn append_strip(dest: &mut Vec<SdfVertex>, strip: Vec<SdfVertex>) {
    if strip.is_empty() {
        return;
    }
    if let Some(&last) = dest.last() {
        dest.extend([last, last, strip[0], strip[0]]);
    }
    dest.extend(strip);
}

pub fn compute_normals(pts: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if pts.is_empty() {
        return Vec::new();
    }
    if pts.len() == 1 {
        return vec![(0.0, 1.0)];
    }

    let seg: Vec<_> = pts
        .windows(2)
        .map(|w| {
            let (dx, dy) = (w[1].0 - w[0].0, w[1].1 - w[0].1);
            let len_sq = dx * dx + dy * dy;
            (len_sq >= MIN_SEG * MIN_SEG).then(|| {
                let inv = len_sq.sqrt().recip();
                (-dy * inv, dx * inv)
            })
        })
        .collect();

    (0..pts.len())
        .map(|i| {
            let prev = (i > 0).then(|| seg[i - 1]).flatten();
            let next = seg.get(i).copied().flatten();
            match (prev, next) {
                (Some((px, py)), Some((nx, ny))) => {
                    let (sx, sy) = (px + nx, py + ny);
                    let len_sq = sx * sx + sy * sy;
                    if len_sq <= f32::EPSILON * f32::EPSILON {
                        return (nx, ny);
                    }
                    let inv = len_sq.sqrt().recip();
                    if inv * 2.0 > MAX_MITER {
                        (nx, ny)
                    } else {
                        (sx * inv, sy * inv)
                    }
                }
                (Some(n), None) | (None, Some(n)) => n,
                (None, None) => seg[i..]
                    .iter()
                    .find_map(|&x| x)
                    .or_else(|| seg[..i].iter().rev().find_map(|&x| x))
                    .unwrap_or((0.0, 1.0)),
            }
        })
        .collect()
}

fn build_strip_core(
    pts: &[(f32, f32)],
    stroke: f32,
    feather: f32,
    clip: &ClipTransform,
    color_fn: impl Fn(usize) -> [f32; 4],
) -> Vec<SdfVertex> {
    if pts.len() < 2 {
        return Vec::new();
    }
    let normals = compute_normals(pts);
    let (half, outer) = (stroke.max(0.1) * 0.5, stroke.max(0.1) * 0.5 + feather);
    pts.iter()
        .zip(&normals)
        .enumerate()
        .flat_map(|(i, ((x, y), (nx, ny)))| {
            let (ox, oy) = (nx * outer, ny * outer);
            let c = color_fn(i);
            [
                SdfVertex::antialiased(clip.to_clip(x - ox, y - oy), c, -outer, half, feather),
                SdfVertex::antialiased(clip.to_clip(x + ox, y + oy), c, outer, half, feather),
            ]
        })
        .collect()
}

/// Builds an antialiased polyline for `TriangleStrip` topology.
pub fn build_aa_line_strip(
    pts: &[(f32, f32)],
    stroke: f32,
    feather: f32,
    color: [f32; 4],
    clip: &ClipTransform,
) -> Vec<SdfVertex> {
    build_strip_core(pts, stroke, feather, clip, |_| color)
}

/// Builds an antialiased polyline for `TriangleList` topology.
pub fn build_aa_line_list(
    pts: &[(f32, f32)],
    stroke: f32,
    feather: f32,
    color: [f32; 4],
    clip: &ClipTransform,
) -> Vec<SdfVertex> {
    if pts.len() < 2 {
        return Vec::new();
    }
    let normals = compute_normals(pts);
    let (half, outer) = (stroke.max(0.1) * 0.5, stroke.max(0.1) * 0.5 + feather);
    let mut verts = Vec::with_capacity((pts.len() - 1) * 6);
    let mut prev: Option<(SdfVertex, SdfVertex)> = None;
    for ((x, y), (nx, ny)) in pts.iter().zip(&normals) {
        let (ox, oy) = (nx * outer, ny * outer);
        let l = SdfVertex::antialiased(clip.to_clip(x - ox, y - oy), color, -outer, half, feather);
        let r = SdfVertex::antialiased(clip.to_clip(x + ox, y + oy), color, outer, half, feather);
        if let Some((l0, r0)) = prev {
            verts.extend([l0, r0, r, l0, r, l]);
        }
        prev = Some((l, r));
    }
    verts
}
