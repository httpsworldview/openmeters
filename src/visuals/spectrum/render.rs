// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use iced::Rectangle;
use std::sync::Arc;

use crate::util::color::{ramp_segment, rgba_with_alpha};
use crate::util::lerp;
use crate::visuals::options::SpectrumDisplayMode;
use crate::visuals::palettes::spectrum::DEFAULT_POSITIONS;
use crate::visuals::render::common::{
    ClipTransform, GeometryFingerprint, GeometryScratch, QuarterTurn, SdfInstance, SdfPipeline,
    baseline_segment_instance, decimate_line, dot_instance, extend_aa_line_list,
    gradient_quad_instance, line_instance, pack_f32_pair, quad_instance, replace_quad_instance,
    sdf_primitive, turn_instances,
};

pub(super) const MIN_TRACE_POINTS: usize = 2;
pub(super) const RAMP_SIZE: usize = DEFAULT_POSITIONS.len();
const MIN_BAR_COUNT: usize = 4;
const LINE_THICKNESS: f32 = 1.0;
const SECONDARY_LINE_THICKNESS: f32 = 0.75;

#[derive(Debug, Clone, Copy)]
pub struct SpectrumPeakParams {
    pub marker: [f32; 2],
    pub marker_color: [f32; 4],
    pub leader_anchor: Option<[f32; 2]>,
    pub leader_color: [f32; 4],
}

#[derive(Debug, Clone)]
pub struct SpectrumCutoutParams {
    pub bounds: Rectangle,
    pub rectangles: Arc<Vec<Rectangle>>,
    pub geometry: crate::visuals::GeometryKey,
    pub revision: u64,
    pub background: [f32; 4],
}

impl SpectrumCutoutParams {
    fn geometry_fingerprint(&self) -> GeometryFingerprint {
        [
            self.revision,
            pack_f32_pair(self.bounds.width, self.bounds.height),
            pack_f32_pair(self.background[0], self.background[1]),
            pack_f32_pair(self.background[2], self.background[3]),
            0,
            0,
        ]
    }

    fn build_vertices(&self, scratch: &mut GeometryScratch) {
        let clip = ClipTransform::from_bounds(self.bounds);
        scratch.instances.extend(
            self.rectangles
                .iter()
                .filter_map(|rectangle| self.bounds.intersection(rectangle))
                .map(|rectangle| replace_quad_instance(rectangle, clip, self.background)),
        );
    }
}

#[derive(Debug, Clone)]
pub struct SpectrumParams {
    pub bounds: Rectangle,
    pub normalized_points: Arc<Vec<[f32; 2]>>,
    pub secondary_points: Arc<Vec<[f32; 2]>>,
    pub geometry: crate::visuals::GeometryKey,
    pub line_color: [f32; 4],
    pub secondary_line_color: [f32; 4],
    pub ramp: [[f32; 4]; RAMP_SIZE],
    pub stop_positions: [f32; RAMP_SIZE],
    pub stop_spreads: [f32; RAMP_SIZE],
    pub display_mode: SpectrumDisplayMode,
    pub turn: QuarterTurn,
    pub bar_count: usize,
    pub bar_gap: f32,
    pub peak: Option<SpectrumPeakParams>,
}

impl SpectrumParams {
    pub(super) fn geometry_fingerprint(&self) -> GeometryFingerprint {
        [
            self.geometry.revision,
            pack_f32_pair(self.bounds.width, self.bounds.height),
            pack_f32_pair(self.line_color[0], self.line_color[1]),
            pack_f32_pair(self.line_color[2], self.line_color[3]),
            pack_f32_pair(self.secondary_line_color[0], self.secondary_line_color[1]),
            pack_f32_pair(self.secondary_line_color[2], self.secondary_line_color[3]),
        ]
    }

    fn build_vertices(&self, scratch: &mut GeometryScratch) {
        let bounds = self.turn.frame(self.bounds);
        let clip = ClipTransform::from_bounds(bounds);

        let has_primary = self.normalized_points.len() >= MIN_TRACE_POINTS;
        if has_primary && self.display_mode == SpectrumDisplayMode::Bar {
            self.build_bar_vertices(&mut scratch.instances, clip, bounds);
        } else {
            self.build_line_vertices(scratch, clip, bounds);
        }
        let vertices = &mut scratch.instances;
        if let Some(peak) = self.peak {
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
            vertices.push(dot_instance(x, y, 3.0, peak.marker_color, clip));
        }
        turn_instances(vertices, self.turn);
    }

    fn build_line_vertices(
        &self,
        scratch: &mut GeometryScratch,
        clip: ClipTransform,
        bounds: Rectangle,
    ) {
        let pixel_budget = bounds.width.ceil().max(1.0) as usize * 2;
        let GeometryScratch {
            instances: vertices,
            points,
            points2,
            ..
        } = scratch;
        let segments = |points: &[[f32; 2]]| points.len().min(pixel_budget).saturating_sub(1);
        vertices.reserve(segments(&self.normalized_points) * 2 + segments(&self.secondary_points));
        let baseline = bounds.y + bounds.height;
        let project = |out: &mut Vec<(f32, f32)>, normalized: &[[f32; 2]]| {
            out.extend(
                normalized
                    .iter()
                    .map(|&p| normalized_to_cartesian(bounds, p)),
            );
            decimate_line(out, pixel_budget);
        };
        project(points, &self.normalized_points);
        project(points2, &self.secondary_points);

        for seg in points.windows(2) {
            let c0 = self.ramp_color((baseline - seg[0].1) / bounds.height);
            let c1 = self.ramp_color((baseline - seg[1].1) / bounds.height);
            if c0[3] > 0.0 || c1[3] > 0.0 {
                vertices.push(baseline_segment_instance(
                    seg[0],
                    seg[1],
                    baseline,
                    clip,
                    [c0, c1],
                    false,
                ));
            }
        }
        extend_aa_line_list(
            vertices,
            points2,
            SECONDARY_LINE_THICKNESS,
            self.secondary_line_color,
            clip,
        );
        extend_aa_line_list(vertices, points, LINE_THICKNESS, self.line_color, clip);
    }

    fn build_bar_vertices(
        &self,
        verts: &mut Vec<SdfInstance>,
        clip: ClipTransform,
        bounds: Rectangle,
    ) {
        let pixel_budget = (bounds.width.ceil().max(1.0) as usize).saturating_mul(2);
        let bar_count = self
            .bar_count
            .clamp(MIN_BAR_COUNT, pixel_budget.max(MIN_BAR_COUNT));
        let gap = self.bar_gap.clamp(0.0, 0.8);
        let unit = bounds.width / bar_count as f32;
        let (bar_w, offset) = (unit * (1.0 - gap), unit * gap * 0.5);
        let baseline = bounds.y + bounds.height;
        let y_at = |amp: f32| bounds.y + bounds.height * (1.0 - amp);
        let secondary = (self.secondary_points.len() >= MIN_TRACE_POINTS)
            .then_some(self.secondary_points.as_ref());

        verts.reserve(bar_count * if secondary.is_some() { 2 } else { 1 });
        for i in 0..bar_count {
            let [t0, t1] = [i, i + 1].map(|edge| edge as f32 / bar_count as f32);
            let amp = sample_max(&self.normalized_points, t0, t1);
            let x0 = bounds.x + i as f32 * unit + offset;
            let x1 = x0 + bar_w;
            if amp >= 1e-4 {
                let y = y_at(amp);
                let color = self.ramp_color(amp);
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
                let sec_y = y_at(sample_max(secondary, t0, t1));
                let h = SECONDARY_LINE_THICKNESS.max(1.0) * 0.5;
                verts.push(quad_instance(
                    x0,
                    sec_y - h,
                    x1,
                    sec_y + h,
                    clip,
                    self.secondary_line_color,
                ));
            }
        }
    }

    fn ramp_color(&self, amp: f32) -> [f32; 4] {
        let (lo, hi, f) = ramp_segment(&self.stop_positions, &self.stop_spreads, amp);
        let [a, b] = [self.ramp[lo], self.ramp[hi]];
        std::array::from_fn(|ch| lerp(a[ch], b[ch], f))
    }
}

fn normalized_to_cartesian(b: Rectangle, [x, y]: [f32; 2]) -> (f32, f32) {
    (b.x + b.width * x, b.y + b.height * (1.0 - y))
}

fn sample_max(pts: &[[f32; 2]], t0: f32, t1: f32) -> f32 {
    let (lo, hi) = (t0.min(t1).clamp(0.0, 1.0), t0.max(t1).clamp(0.0, 1.0));
    let start = pts.partition_point(|p| p[0] < lo);
    let hi_index = pts.partition_point(|p| p[0] < hi);
    let end = pts.partition_point(|p| p[0] <= hi);
    pts[start..end].iter().map(|p| p[1]).fold(
        sample_lerp_at(pts, lo, start).max(sample_lerp_at(pts, hi, hi_index)),
        f32::max,
    )
}

fn sample_lerp_at(pts: &[[f32; 2]], t: f32, index: usize) -> f32 {
    if index == 0 || index >= pts.len() {
        return pts[index.min(pts.len() - 1)][1];
    }
    let [a, b] = [pts[index - 1], pts[index]];
    lerp(a[1], b[1], (t - a[0]) / (b[0] - a[0]).max(1e-6))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_max_handles_interpolation_and_duplicate_positions() {
        let points = [[0.0, 0.1], [0.25, 0.8], [0.25, 0.4], [0.6, 0.3], [1.0, 0.9]];

        for (lo, hi, expected) in [
            (0.1, 0.2, 0.66),
            (0.25, 0.25, 0.8),
            (0.2, 0.6, 0.8),
            (0.6, 0.9, 0.75),
        ] {
            let actual = sample_max(&points, lo, hi);
            assert!((actual - expected).abs() < 1.0e-6, "{lo}..{hi}: {actual}");
        }
    }
}

sdf_primitive!(
    SpectrumParams,
    SdfPipeline,
    "Spectrum",
    |self| self.geometry.id,
    Some(self.geometry_fingerprint())
);
sdf_primitive!(
    SpectrumCutoutParams,
    SdfPipeline<true>,
    "Spectrum label cutouts",
    |self| self.geometry.id,
    Some(self.geometry_fingerprint())
);
