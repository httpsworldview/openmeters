// Geometry utilities for rendering.

use crate::ui::render::common::{ClipTransform, SdfVertex};

// Default feather distance for antialiased lines.
pub const DEFAULT_FEATHER: f32 = 1.0;

// Joins triangle strips with degenerate triangles for batched draw calls.
pub fn append_strip(dest: &mut Vec<SdfVertex>, strip: Vec<SdfVertex>) {
    if strip.is_empty() {
        return;
    }
    if let Some(&last) = dest.last() {
        dest.extend([last, last, strip[0], strip[0]]);
    }
    dest.extend(strip);
}

// Builds an antialiased polyline for `TriangleList` topology.
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
    let (half, outer) = (stroke.max(0.1) * 0.5, stroke.max(0.1) * 0.5 + feather);
    let mut verts = Vec::with_capacity((pts.len() - 1) * 6);
    for seg in pts.windows(2) {
        let ((x0, y0), (x1, y1)) = (seg[0], seg[1]);
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-4 {
            continue;
        }
        let inv = len.recip();
        let (ox, oy) = (-dy * inv * outer, dx * inv * outer);
        let mk = |px, py, d| SdfVertex::antialiased(clip.to_clip(px, py), color, d, half, feather);
        verts.extend([
            mk(x0 - ox, y0 - oy, -outer),
            mk(x0 + ox, y0 + oy, outer),
            mk(x1 + ox, y1 + oy, outer),
            mk(x0 - ox, y0 - oy, -outer),
            mk(x1 + ox, y1 + oy, outer),
            mk(x1 - ox, y1 - oy, -outer),
        ]);
    }
    verts
}
