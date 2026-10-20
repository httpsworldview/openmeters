// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use iced::Rectangle;
use std::ops::Deref;
use std::sync::{Arc, LazyLock};

use super::processor::{BAND_COUNT, FULL_BAND};
use crate::util::lerp;
use crate::visuals::options::{
    CorrelationMeterMode, CorrelationMeterSide, StereometerMode, StereometerScale,
};
use crate::visuals::palettes::stereometer::SIZE as PALETTE_SIZE;
use crate::visuals::render::common::{
    ClipTransform, RadialDotTemplate, SdfInstance, SdfPipeline, bounds_fingerprint,
    gradient_quad_instance, line_instance, quad_instance, sdf_primitive,
};

// For length >= f32::EPSILON: radius = min((0.66834 * length)^0.3, 1).
// Constants mirrored in render/shaders/sdf.wgsl.
const SCALED_MODE_SCALE: f32 = 0.886_133_7;
const SCALED_MODE_SATURATION_SQUARED: f32 = 2.238_747_4;
const LINEAR_GUIDE_LEVELS: [f32; 3] = [1.0 / 3.0, 2.0 / 3.0, 1.0];
// -48, -24, -12, and 0 dBFS.
const SCALED_GUIDE_LEVELS: [f32; 4] = [0.0039810717, 0.06309573, 0.25118864, 1.0];
const GRID_SEGMENTS: usize = 16;
const GRID_LINE_WIDTH: f32 = 1.0;
const RENDER_LABEL: &str = "Stereometer";
const GRID_CORNERS: [(f32, f32); 4] = [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)];
const GRID_AXES: [((f32, f32), (f32, f32)); 2] =
    [((1.0, 1.0), (-1.0, -1.0)), ((1.0, -1.0), (-1.0, 1.0))];

const CORR_W: f32 = 28.0;
const CORR_PAD: f32 = 4.0;
pub(super) const CORR_LABEL_GAP: f32 = 5.0;
pub(super) const CORR_LABEL_H: f32 = 12.0;
pub(super) const CORR_LABEL_W: f32 = 16.0;
pub(super) const CORR_TRAIL_LEN: usize = 32;
pub(super) const CORR_TRAIL_STEP: f32 = 1.0 / 60.0;
const CORR_VPAD_RATIO: f32 = 5.0 / 64.0;
const CORR_EDGE: f32 = 6.0;

fn corr_opacity(age: usize, trail_len: usize) -> f32 {
    (1.0 - (age + 1) as f32 / trail_len as f32).powf(2.4)
}

static CORR_OPACITIES: LazyLock<[f32; CORR_TRAIL_LEN - 1]> =
    LazyLock::new(|| std::array::from_fn(|age| corr_opacity(age, CORR_TRAIL_LEN)));

fn scaled_point(x: f32, y: f32) -> (f32, f32) {
    let squared = x * x + y * y;
    if squared < f32::EPSILON * f32::EPSILON {
        return (0.0, 0.0);
    }
    let scale = if squared < SCALED_MODE_SATURATION_SQUARED {
        SCALED_MODE_SCALE * squared.powf(-0.35)
    } else if squared.is_finite() {
        squared.sqrt().recip()
    } else {
        let len = x.hypot(y);
        return (x / len, y / len);
    };
    (x * scale, y * scale)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FixedTrail([f32; CORR_TRAIL_LEN], usize);

impl FixedTrail {
    pub(super) fn advance(&mut self, value: f32, steps: usize) {
        let steps = steps.min(CORR_TRAIL_LEN);
        self.1 = (self.1 + steps).clamp(1, CORR_TRAIL_LEN);
        self.0[..self.1].rotate_right(steps);
        self.0[..steps.max(1)].fill(value);
    }
}

impl Deref for FixedTrail {
    type Target = [f32];
    fn deref(&self) -> &Self::Target {
        &self.0[..self.1]
    }
}

#[derive(Debug, Clone)]
pub struct StereometerParams {
    pub geometry: crate::visuals::GeometryKey,
    pub grid: crate::visuals::GeometryKey,
    pub bounds: Rectangle,
    pub points: [Arc<[(f32, f32)]>; BAND_COUNT + 1],
    pub band_colors: [Arc<[[f32; 4]]>; BAND_COUNT],
    pub palette: [[f32; 4]; PALETTE_SIZE],
    pub mode: StereometerMode,
    pub scale: StereometerScale,
    pub dot_radius: f32,
    pub rotation: i8,
    pub flip: bool,
    pub unipolar: bool,
    pub correlation_meter: CorrelationMeterMode,
    pub correlation_meter_side: CorrelationMeterSide,
    pub trails: [FixedTrail; BAND_COUNT + 1],
}

#[derive(Debug, Clone, Copy)]
struct Projection {
    cx: f32,
    cy: f32,
    sin_t: f32,
    cos_t: f32,
    fit: f32,
    radius: f32,
    flip: bool,
    unipolar: bool,
    scale: StereometerScale,
}

impl Projection {
    fn from_params(p: &StereometerParams, bounds: Rectangle) -> Self {
        let scale = if p.mode == StereometerMode::Lissajous {
            StereometerScale::Linear
        } else {
            p.scale
        };
        Self::new(scale, p.rotation, p.flip, p.unipolar, bounds)
    }

    fn new(
        scale: StereometerScale,
        rotation: i8,
        flip: bool,
        unipolar: bool,
        bounds: Rectangle,
    ) -> Self {
        let half_w = bounds.width * 0.5;
        let half_h = bounds.height * 0.5;
        let cx = bounds.x + half_w;
        let (cy, extent) = if unipolar {
            (bounds.y + bounds.height, half_w.min(bounds.height))
        } else {
            (bounds.y + half_h, half_w.min(half_h))
        };
        let (sin_t, cos_t) = (f32::from(rotation) * std::f32::consts::FRAC_PI_4).sin_cos();
        let fit = match scale {
            StereometerScale::Linear => 1.0 / (cos_t.abs() + sin_t.abs()).max(f32::EPSILON),
            StereometerScale::Scaled => 1.0,
        };
        Self {
            cx,
            cy,
            sin_t,
            cos_t,
            fit,
            radius: (extent - 2.0).max(0.0),
            flip,
            unipolar,
            scale,
        }
    }

    fn project(self, l: f32, r: f32) -> (f32, f32) {
        self.to_screen(self.visible(self.unit(l, r)))
    }

    fn visible(self, (x, y): (f32, f32)) -> (f32, f32) {
        if self.unipolar && y > 0.0 {
            (-x, -y)
        } else {
            (x, y)
        }
    }

    fn segment(self, a: (f32, f32), b: (f32, f32)) -> Option<((f32, f32), (f32, f32))> {
        let (a, b) = (self.unit(a.0, a.1), self.unit(b.0, b.1));
        let (a, b) = if self.unipolar {
            clip_segment_to_visible_unipolar_half(a, b)?
        } else {
            (a, b)
        };
        Some((self.to_screen(a), self.to_screen(b)))
    }

    fn rotated(self, l: f32, r: f32) -> (f32, f32) {
        let (l, r) = if self.flip { (r, l) } else { (l, r) };
        (
            l * self.cos_t + r * self.sin_t,
            l * self.sin_t - r * self.cos_t,
        )
    }

    fn unit(self, l: f32, r: f32) -> (f32, f32) {
        let (x, y) = self.rotated(l, r);
        match self.scale {
            StereometerScale::Linear => (x * self.fit, y * self.fit),
            StereometerScale::Scaled => scaled_point(x, y),
        }
    }

    fn to_screen(self, (x, y): (f32, f32)) -> (f32, f32) {
        (self.cx + x * self.radius, self.cy + y * self.radius)
    }
}

fn clip_segment_to_visible_unipolar_half(
    mut a: (f32, f32),
    mut b: (f32, f32),
) -> Option<((f32, f32), (f32, f32))> {
    let a_outside = a.1 > 0.0;
    let b_outside = b.1 > 0.0;

    if a_outside && b_outside {
        return None;
    }
    if a_outside || b_outside {
        let boundary_fraction = a.1 / (a.1 - b.1);
        let boundary = (lerp(a.0, b.0, boundary_fraction), 0.0);
        if a_outside {
            a = boundary;
        } else {
            b = boundary;
        }
    }
    Some((a, b))
}

fn projected_line(
    out: &mut Vec<SdfInstance>,
    projection: Projection,
    a: (f32, f32),
    b: (f32, f32),
    color: [f32; 4],
    clip: ClipTransform,
) {
    for seg in 0..GRID_SEGMENTS {
        let t0 = seg as f32 / GRID_SEGMENTS as f32;
        let t1 = (seg + 1) as f32 / GRID_SEGMENTS as f32;
        if let Some((p0, p1)) = projection.segment(
            (lerp(a.0, b.0, t0), lerp(a.1, b.1, t0)),
            (lerp(a.0, b.0, t1), lerp(a.1, b.1, t1)),
        ) {
            out.push(line_instance(p0, p1, color, color, GRID_LINE_WIDTH, clip));
        }
    }
}

impl StereometerParams {
    fn add_grid_vertices(
        &self,
        vertices: &mut Vec<SdfInstance>,
        projection: Projection,
        clip: ClipTransform,
    ) {
        let color = self.palette[8];
        if color[3] < f32::EPSILON {
            return;
        }

        let levels: &[f32] = match projection.scale {
            StereometerScale::Linear => &LINEAR_GUIDE_LEVELS,
            StereometerScale::Scaled => &SCALED_GUIDE_LEVELS,
        };
        let mut add_line = |start, end| {
            projected_line(vertices, projection, start, end, color, clip);
        };
        for &radius in levels {
            for (edge, &(x, y)) in GRID_CORNERS.iter().enumerate() {
                let (nx, ny) = GRID_CORNERS[(edge + 1) % GRID_CORNERS.len()];
                add_line((x * radius, y * radius), (nx * radius, ny * radius));
            }
        }

        let axes = if self.mode == StereometerMode::Lissajous {
            &GRID_AXES[..1]
        } else {
            &GRID_AXES[..]
        };
        axes.iter().copied().for_each(|(a, b)| add_line(a, b));
    }

    pub(super) fn correlation_y(bounds: Rectangle, value: f32) -> f32 {
        bounds.y + (1.0 - value.clamp(-1.0, 1.0)) * bounds.height * 0.5
    }

    pub(super) fn meter_layout(&self) -> (Rectangle, Option<Rectangle>) {
        let has_meter = self.correlation_meter != CorrelationMeterMode::Off;
        let left = self.correlation_meter_side == CorrelationMeterSide::Left;
        let scale = match self.correlation_meter {
            CorrelationMeterMode::SingleBand => 0.5,
            _ => 1.0,
        };
        let available_height = self.bounds.height.max(0.0);
        let width = (available_height * 5.0 / 32.0).min(CORR_W) * scale;
        let margin = if has_meter {
            CORR_EDGE + width + CORR_LABEL_GAP + CORR_LABEL_W + CORR_PAD
        } else {
            0.0
        };
        let vector = Rectangle {
            x: self.bounds.x + if left { margin } else { 0.0 },
            width: (self.bounds.width - margin).max(0.0),
            ..self.bounds
        };
        let meter = has_meter.then(|| {
            let x = if left {
                self.bounds.x + CORR_EDGE
            } else {
                (self.bounds.x + self.bounds.width - width - CORR_EDGE).max(self.bounds.x)
            };
            let vpad = (available_height * CORR_VPAD_RATIO)
                .max(CORR_LABEL_H * 0.5)
                .min(available_height * 0.5);
            Rectangle {
                x,
                y: self.bounds.y + vpad,
                width,
                height: (available_height - 2.0 * vpad).max(0.0),
            }
        });
        (vector, meter)
    }

    fn add_trace_vertices(
        &self,
        out: &mut Vec<SdfInstance>,
        projection: Projection,
        clip: ClipTransform,
    ) {
        let [cr, cg, cb, ca] = self.palette[0];
        let dot_r = self.dot_radius;
        let radial_scale = match projection.scale {
            StereometerScale::Linear => projection.fit,
            StereometerScale::Scaled => -1.0,
        };
        let center_radius = [projection.cx, projection.cy, projection.radius];
        let dots = RadialDotTemplate::new(
            center_radius,
            radial_scale,
            dot_r,
            clip,
            self.mode == StereometerMode::DotCloudBands,
        );
        let dot = |l, r, color| dots.instance(projection.visible(projection.rotated(l, r)), color);

        let points = &self.points[FULL_BAND];
        match self.mode {
            StereometerMode::DotCloud => {
                let count = points.len() as f32;
                out.extend(points.iter().enumerate().map(|(i, &(l, r))| {
                    let alpha = ca * (i + 1) as f32 / count;
                    dot(l, r, [cr, cg, cb, alpha])
                }));
            }
            StereometerMode::Lissajous => {
                if points.len() >= 2 {
                    let last = (points.len() - 1) as f32;
                    out.extend(points.windows(2).enumerate().map(|(i, w)| {
                        let p0 = projection.project(w[0].0, w[0].1);
                        let p1 = projection.project(w[1].0, w[1].1);
                        let (t0, t1) = (i as f32 / last, (i + 1) as f32 / last);
                        line_instance(
                            p0,
                            p1,
                            [cr, cg, cb, ca * t0],
                            [cr, cg, cb, ca * t1],
                            1.5,
                            clip,
                        )
                    }));
                }
            }
            StereometerMode::DotCloudBands => {
                for (pts, colors) in self.points[1..].iter().zip(&self.band_colors) {
                    out.extend(
                        pts.iter()
                            .zip(colors.iter())
                            .map(|(&(l, r), &color)| dot(l, r, color)),
                    );
                }
            }
        }
    }

    fn add_correlation_vertices(
        &self,
        out: &mut Vec<SdfInstance>,
        alpha: &mut Vec<f32>,
        bounds: Rectangle,
        clip: ClipTransform,
    ) {
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return;
        }

        let multi_band = self.correlation_meter == CorrelationMeterMode::MultiBand;
        let bars = if multi_band { BAND_COUNT } else { 1 };
        let bar_width = bounds.width / bars as f32;
        let val_y = |value| Self::correlation_y(bounds, value);
        let center = val_y(0.0);
        let marker_h = (self.bounds.height / 128.0).clamp(1.0, 3.0) * 0.5;
        let y_min = bounds.y as i32;
        let height = (bounds.height as i32 + 1).max(0) as usize;
        let y_max = y_min + height as i32 - 1;

        let (trails, palette) = (&self.trails, &self.palette);

        let quad =
            |y0, y1, color| quad_instance(bounds.x, y0, bounds.x + bounds.width, y1, clip, color);
        out.push(quad(bounds.y, bounds.y + bounds.height, palette[1]));
        for y in [val_y(1.0), center, val_y(-1.0)] {
            out.push(quad(y - 0.5, y + 0.5, palette[2]));
        }

        let mut draw_trail =
            |x: f32, width: f32, trail: &[f32], positive: [f32; 4], negative: [f32; 4]| {
                let inset = (width * 0.5).min(0.25);
                let (x0, x1) = (x + inset, x + width - inset);
                let color = |is_negative| if is_negative { negative } else { positive };
                if trail.len() > 1 {
                    alpha.clear();
                    alpha.resize(height, 0.0);
                    for (age, pair) in trail.windows(2).enumerate() {
                        let opacity = if trail.len() == CORR_TRAIL_LEN {
                            CORR_OPACITIES[age]
                        } else {
                            corr_opacity(age, trail.len())
                        };
                        let (y0, y1) = (val_y(pair[0]), val_y(pair[1]));
                        let (top, bottom) = (y0.min(y1) as i32, (y0.max(y1) + 2.0) as i32);
                        for y in top.max(y_min)..=bottom.min(y_max) {
                            let index = (y - y_min) as usize;
                            alpha[index] = alpha[index].max(opacity);
                        }
                    }
                    for (index, opacity) in alpha.windows(2).enumerate() {
                        if opacity[0] > 0.0 || opacity[1] > 0.0 {
                            let y0 = (y_min + index as i32) as f32;
                            let y1 = y0 + 1.0;
                            let (mut top, mut bottom) = (color(y0 > center), color(y1 > center));
                            top[3] *= opacity[0];
                            bottom[3] *= opacity[1];
                            out.push(gradient_quad_instance(x0, y0, x1, y1, clip, top, bottom));
                        }
                    }
                }
                if let Some(&current) = trail.first() {
                    let y = val_y(current);
                    let (top, bottom) = (y - marker_h, y + marker_h);
                    let color = color(current < 0.0);
                    out.push(quad_instance(x0, top, x1, bottom, clip, color));
                }
            };

        if multi_band {
            let mut color = palette[2];
            color[3] *= 0.25;
            draw_trail(bounds.x, bounds.width, &trails[FULL_BAND], color, color);
        }
        for band in 0..bars {
            let x = bounds.x + band as f32 * bar_width;
            let (trail, positive, negative) = if multi_band {
                (&trails[band + 1][..], palette[5 + band], palette[5 + band])
            } else {
                (&trails[FULL_BAND][..], palette[3], palette[4])
            };
            draw_trail(x, bar_width, trail, positive, negative);
        }
    }
}

sdf_primitive!(
    StereometerParams, SdfPipeline,
    RENDER_LABEL,
    layers |self, scratch| {
        (self.grid.id, Some(bounds_fingerprint(self.grid.revision, self.bounds))) => {
            self.add_grid_vertices(
                &mut scratch.instances,
                Projection::from_params(self, self.meter_layout().0),
                ClipTransform::from_bounds(self.bounds),
            );
        },
        (self.geometry.id, Some(bounds_fingerprint(self.geometry.revision, self.bounds))) => {
            let clip = ClipTransform::from_bounds(self.bounds);
            let (vector, correlation) = self.meter_layout();
            let projection = Projection::from_params(self, vector);
            self.add_trace_vertices(&mut scratch.instances, projection, clip);
            if let Some(meter) = correlation {
                self.add_correlation_vertices(
                    &mut scratch.instances, &mut scratch.scalars, meter, clip,
                );
            }
        },
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use StereometerScale::{Linear, Scaled};

    const EPS: f32 = 1e-4;
    const BOUNDS: Rectangle = Rectangle {
        x: 0.0,
        y: 0.0,
        width: 200.0,
        height: 100.0,
    };
    const FULL_SCALE: [(f32, f32); 6] = [
        (-1.0, -1.0),
        (-1.0, 1.0),
        (1.0, -1.0),
        (1.0, 1.0),
        (1.0, 0.0),
        (0.0, 1.0),
    ];

    fn assert_close((ax, ay): (f32, f32), (bx, by): (f32, f32)) {
        assert!(
            (ax - bx).abs() <= EPS && (ay - by).abs() <= EPS,
            "({ax}, {ay}) != ({bx}, {by})"
        );
    }

    fn assert_inside((x, y): (f32, f32)) {
        assert!(
            (-EPS..=BOUNDS.width + EPS).contains(&x) && (-EPS..=BOUNDS.height + EPS).contains(&y),
            "({x}, {y}) outside {BOUNDS:?}"
        );
    }

    #[test]
    fn fixed_trail_is_newest_first_and_bounded() {
        let mut trail = FixedTrail::default();
        for value in 0..CORR_TRAIL_LEN + 2 {
            trail.advance(value as f32, 1);
        }
        assert_eq!(trail.len(), CORR_TRAIL_LEN);
        assert_eq!((trail[0], trail[CORR_TRAIL_LEN - 1]), (33.0, 2.0));
        trail.advance(40.0, 0);
        trail.advance(41.0, 2);
        assert_eq!(trail[..4], [41.0, 41.0, 40.0, 32.0]);
    }

    #[test]
    fn projection_centers_fits_and_flips() {
        for scale in [Linear, Scaled] {
            for rotation in -4_i8..=4 {
                for unipolar in [false, true] {
                    let normal = Projection::new(scale, rotation, false, unipolar, BOUNDS);
                    let flipped = Projection::new(scale, rotation, true, unipolar, BOUNDS);

                    for p in [normal, flipped] {
                        assert_close(p.project(0.0, 0.0), (p.cx, p.cy));
                        for (l, r) in FULL_SCALE {
                            assert_inside(p.project(l, r));
                        }
                    }

                    for (l, r) in [(-0.75, 0.25), (0.2, -0.9), (1.0, 0.0)] {
                        assert_close(flipped.project(l, r), normal.project(r, l));
                    }
                }
            }
        }
    }

    #[test]
    fn scaled_projection_matches_radial_definition() {
        let reference = |x: f32, y: f32| {
            let len = x.hypot(y);
            if len < f32::EPSILON {
                return (0.0, 0.0);
            }
            let radius = (len * 0.66834).powf(0.3).min(1.0);
            (x * radius / len, y * radius / len)
        };
        for x in -32..=32 {
            for y in -32..=32 {
                let point = (x as f32 / 16.0, y as f32 / 16.0);
                assert_close(scaled_point(point.0, point.1), reference(point.0, point.1));
            }
        }
    }

    #[test]
    fn unipolar_clip_rejects_hidden_segments_and_trims_crossings() {
        assert!(clip_segment_to_visible_unipolar_half((-1.0, 1.0), (1.0, 1.0)).is_none());

        for (input, expected) in [
            (((-1.0, -1.0), (1.0, 1.0)), ((-1.0, -1.0), (0.0, 0.0))),
            (((-1.0, 1.0), (1.0, -1.0)), ((0.0, 0.0), (1.0, -1.0))),
        ] {
            let got = clip_segment_to_visible_unipolar_half(input.0, input.1).unwrap();
            assert_close(got.0, expected.0);
            assert_close(got.1, expected.1);
        }
    }
}
