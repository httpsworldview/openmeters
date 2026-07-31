// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use iced::Rectangle;
use iced::advanced::graphics::Viewport;
use std::sync::Arc;

use crate::visuals::options::SpectrumDisplayMode;
use crate::visuals::palettes::spectrum::SIZE as PALETTE_SIZE;
use crate::visuals::render::common::sdf_primitive;
use crate::util::color::{rgba_with_alpha, sample_rgba_gradient};
use crate::util::lerp;
use crate::visuals::render::common::{
    ClipTransform, GeometryFingerprint, GeometryScratch, SdfInstance, baseline_segment_instance,
    decimate_finite_ordered_line_in_place, dot_instance, extend_aa_line_list,
    gradient_quad_instance, line_instance, quad_instance,
};

const MIN_BAR_COUNT: usize = 4;
const LINE_THICKNESS: f32 = 1.0;
const SECONDARY_LINE_THICKNESS: f32 = 0.75;

fn pack_f32_pair(first: f32, second: f32) -> u64 {
    u64::from(first.to_bits()) << 32 | u64::from(second.to_bits())
}

#[derive(Debug, Clone, Copy)]
pub struct SpectrumPeakParams {
    pub marker: [f32; 2],
    pub marker_color: [f32; 4],
    pub leader_anchor: Option<[f32; 2]>,
    pub leader_color: [f32; 4],
}

#[derive(Debug, Clone)]
pub struct SpectrumParams {
    pub bounds: Rectangle,
    pub normalized_points: Arc<Vec<[f32; 2]>>,
    pub secondary_points: Arc<Vec<[f32; 2]>>,
    pub key: u64,
    pub geometry_revision: u64,
    pub line_color: [f32; 4],
    pub secondary_line_color: [f32; 4],
    pub highlight_threshold: f32,
    pub spectrum_palette: [[f32; 4]; PALETTE_SIZE],
    pub display_mode: SpectrumDisplayMode,
    pub bar_count: usize,
    pub bar_gap: f32,
    pub peak: Option<SpectrumPeakParams>,
}

impl SpectrumParams {
    pub(super) fn geometry_fingerprint(&self) -> GeometryFingerprint {
        let line = self.line_color;
        let secondary = self.secondary_line_color;
        [
            self.geometry_revision,
            pack_f32_pair(self.bounds.width, self.bounds.height),
            pack_f32_pair(line[0], line[1]),
            pack_f32_pair(line[2], line[3]),
            pack_f32_pair(secondary[0], secondary[1]),
            pack_f32_pair(secondary[2], secondary[3]),
        ]
    }
}

impl SpectrumPrimitive {
    fn build_vertices(&self, _viewport: &Viewport, scratch: &mut GeometryScratch) {
        let bounds = self.params.bounds;
        let clip = ClipTransform::from_bounds(bounds);

        let has_primary = self.params.normalized_points.len() >= 2;
        if !has_primary && self.params.secondary_points.len() < 2 {
            return;
        }

        if has_primary && self.params.display_mode == SpectrumDisplayMode::Bar {
            self.build_bar_vertices(&mut scratch.instances, clip, bounds);
        } else {
            self.build_line_vertices(scratch, clip, bounds);
        }
        let vertices = &mut scratch.instances;
        if let Some(peak) = self.params.peak {
            if let Some(anchor) = peak.leader_anchor {
                vertices.push(line_instance(
                    normalized_to_cartesian(bounds, anchor),
                    normalized_to_cartesian(bounds, peak.marker),
                    peak.leader_color,
                    peak.leader_color,
                    1.0,
                    clip,
                ));
            }
            let (x, y) = normalized_to_cartesian(bounds, peak.marker);
            vertices.push(dot_instance(x, y, 3.0, peak.marker_color, clip, false));
        }
    }

    fn build_line_vertices(&self, scratch: &mut GeometryScratch, clip: ClipTransform, bounds: Rectangle) {
        let pixel_budget = bounds.width.ceil().max(1.0) as usize * 2;
        let GeometryScratch { instances: vertices, points, points2, .. } = scratch;
        let normalized = self.params.normalized_points.as_ref();
        let has_primary = normalized.len() >= 2;
        let has_secondary = self.params.secondary_points.len() >= 2;
        let primary_segments = normalized.len().min(pixel_budget).saturating_sub(1);
        let secondary_segments = self.params.secondary_points.len().min(pixel_budget).saturating_sub(1);
        vertices.reserve(primary_segments * 2 + secondary_segments);
        let baseline = bounds.y + bounds.height;

        if has_primary {
            points.extend(normalized.iter().map(|&p| normalized_to_cartesian(bounds, p)));
            decimate_finite_ordered_line_in_place(points, pixel_budget);
            push_highlight_columns(
                vertices,
                clip,
                baseline,
                bounds.height,
                points,
                &self.params.spectrum_palette,
                self.params.highlight_threshold,
            );
        }

        if has_secondary {
            points2.extend(
                self.params
                    .secondary_points
                    .iter()
                    .map(|&p| normalized_to_cartesian(bounds, p)),
            );
            decimate_finite_ordered_line_in_place(points2, pixel_budget);
            extend_aa_line_list(
                vertices,
                points2,
                SECONDARY_LINE_THICKNESS,
                self.params.secondary_line_color,
                clip,
            );
        }

        if has_primary {
            extend_aa_line_list(
                vertices,
                points,
                LINE_THICKNESS,
                self.params.line_color,
                clip,
            );
        }
    }

    fn build_bar_vertices(&self, verts: &mut Vec<SdfInstance>, clip: ClipTransform, bounds: Rectangle) {
        let p = &self.params;
        let bar_count = p.bar_count.max(MIN_BAR_COUNT);
        let gap = p.bar_gap.clamp(0.0, 0.8);
        let unit = bounds.width / bar_count as f32;
        let (bar_w, offset) = (unit * (1.0 - gap), unit * gap * 0.5);
        let baseline = bounds.y + bounds.height;
        let y_at = |amp: f32| bounds.y + bounds.height * (1.0 - amp);
        let secondary = (p.secondary_points.len() >= 2).then_some(p.secondary_points.as_ref());

        verts.reserve(bar_count * if secondary.is_some() { 2 } else { 1 });
        for i in 0..bar_count {
            let (t0, t1) = (
                i as f32 / bar_count as f32,
                (i + 1) as f32 / bar_count as f32,
            );
            let amp = sample_max(&p.normalized_points, t0, t1);
            let x0 = bounds.x + i as f32 * unit + offset;
            let x1 = x0 + bar_w;
            if amp >= 1e-4 {
                let y = y_at(amp);
                let color = palette_color(&p.spectrum_palette, amp, p.highlight_threshold);
                verts.push(gradient_quad_instance(
                    x0,
                    y,
                    x1,
                    baseline,
                    clip,
                    rgba_with_alpha(color, color[3] * 0.82),
                    rgba_with_alpha(color, color[3] * 0.22),
                ));
            }

            if let Some(secondary) = secondary {
                let sec_y = y_at(sample_lerp(secondary, (t0 + t1) * 0.5));
                let h = SECONDARY_LINE_THICKNESS.max(1.0) * 0.5;
                verts.push(quad_instance(
                    x0,
                    sec_y - h,
                    x1,
                    sec_y + h,
                    clip,
                    p.secondary_line_color,
                ));
            }
        }
    }
}

fn normalized_to_cartesian(b: Rectangle, [x, y]: [f32; 2]) -> (f32, f32) {
    (b.x + b.width * x, b.y + b.height * (1.0 - y))
}

fn push_highlight_columns(
    vertices: &mut Vec<SdfInstance>,
    clip: ClipTransform,
    baseline: f32,
    height: f32,
    positions: &[(f32, f32)],
    palette: &[[f32; 4]],
    threshold: f32,
) {
    for seg in positions.windows(2) {
        let c0 = palette_color(palette, (baseline - seg[0].1) / height, threshold);
        let c1 = palette_color(palette, (baseline - seg[1].1) / height, threshold);
        if c0[3] > 0.0 || c1[3] > 0.0 {
            vertices.push(baseline_segment_instance(seg[0], seg[1], baseline, clip, [c0, c1]));
        }
    }
}

fn palette_color(palette: &[[f32; 4]], amp: f32, threshold: f32) -> [f32; 4] {
    let intensity = (amp - threshold) / (1.0 - threshold).max(1e-6);
    sample_rgba_gradient(palette, intensity)
}

fn sample_max(pts: &[[f32; 2]], t0: f32, t1: f32) -> f32 {
    let (lo, hi) = (t0.min(t1).clamp(0.0, 1.0), t0.max(t1).clamp(0.0, 1.0));
    let start = pts.partition_point(|p| p[0] < lo);
    let hi_index = pts.partition_point(|p| p[0] < hi);
    let mut end = hi_index;
    while end < pts.len() && pts[end][0] <= hi {
        end += 1;
    }
    pts[start..end]
        .iter()
        .map(|p| p[1])
        .fold(sample_lerp_at(pts, lo, start).max(sample_lerp_at(pts, hi, hi_index)), f32::max)
}

fn sample_lerp(pts: &[[f32; 2]], t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let index = pts.partition_point(|p| p[0] < t);
    sample_lerp_at(pts, t, index)
}

fn sample_lerp_at(pts: &[[f32; 2]], t: f32, index: usize) -> f32 {
    if index == 0 { return pts[0][1]; }
    if index >= pts.len() { return pts[pts.len() - 1][1]; }
    let a = pts[index - 1];
    let b = pts[index];
    lerp(a[1], b[1], (t - a[0]) / (b[0] - a[0]).max(1e-6))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_max_matches_reference_with_duplicate_positions() {
        let points = [[0.0, 0.1], [0.25, 0.8], [0.25, 0.4], [0.6, 0.3], [1.0, 0.9]];
        let reference = |lo: f32, hi: f32| {
            let start = points.partition_point(|p| p[0] < lo);
            let end = points.partition_point(|p| p[0] <= hi);
            points[start..end]
                .iter()
                .map(|p| p[1])
                .fold(sample_lerp(&points, lo).max(sample_lerp(&points, hi)), f32::max)
        };
        for (lo, hi) in [(0.1, 0.2), (0.25, 0.25), (0.2, 0.6), (0.6, 0.9)] {
            assert_eq!(sample_max(&points, lo, hi), reference(lo, hi));
        }
    }
}

sdf_primitive!(
    SpectrumPrimitive(SpectrumParams),
    Pipeline,
    u64,
    "Spectrum",
    TriangleStrip,
    |self| self.params.key,
    self.params.geometry_fingerprint()
);
