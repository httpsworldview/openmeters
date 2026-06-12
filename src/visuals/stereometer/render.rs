// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use iced::Rectangle;
use iced::advanced::graphics::Viewport;

use super::processor::BandCorrelation;
use crate::visuals::render::common::sdf_primitive;
use crate::visuals::options::{
    CorrelationMeterMode, CorrelationMeterSide, StereometerMode, StereometerScale,
};
use crate::visuals::render::common::{
    ClipTransform, GeometryScratch, SdfVertex, dot_vertices, gradient_quad_vertices, line_vertices,
    quad_vertices,
};

pub fn scale_point(scale: StereometerScale, x: f32, y: f32, range: f32) -> (f32, f32) {
    match scale {
        StereometerScale::Linear => (x, y),
        StereometerScale::Exponential => {
            let len = x.hypot(y);
            if len < f32::EPSILON { return (0.0, 0.0); }
            let k = (len.max((-range).exp2()).log2() + range) / (range * len);
            (k * x, k * y)
        }
    }
}

const CORR_W: f32 = 28.0;
const CORR_PAD: f32 = 4.0;
const CORR_VPAD: f32 = 16.0;
const CORR_EDGE: f32 = 6.0;
const BAND_GAP: f32 = 2.0;
const GRID_RINGS: usize = 3;
const GRID_SEGMENTS: usize = 16;
const GRID_LINE_WIDTH: f32 = 1.0;
const GRID_CORNERS: [(f32, f32); 4] = [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)];
const GRID_AXES: [((f32, f32), (f32, f32)); 2] =
    [((1.0, 1.0), (-1.0, -1.0)), ((1.0, -1.0), (-1.0, 1.0))];

#[derive(Debug, Clone)]
pub struct StereometerParams {
    pub key: u64,
    pub bounds: Rectangle,
    pub points: Vec<(f32, f32)>,
    pub band_points: [Vec<(f32, f32)>; 3],
    pub palette: [[f32; 4]; 9],
    pub mode: StereometerMode,
    pub scale: StereometerScale,
    pub scale_range: f32,
    pub dot_radius: f32,
    pub rotation: i8,
    pub flip: bool,
    pub correlation_meter: CorrelationMeterMode,
    pub correlation_meter_side: CorrelationMeterSide,
    pub corr_trail: Vec<f32>,
    pub band_trail: Vec<BandCorrelation>,
}

struct VecTransform {
    cx: f32,
    cy: f32,
    sin_t: f32,
    cos_t: f32,
    rot_scale: f32,
    radius: f32,
    flip: bool,
    scale: StereometerScale,
    scale_range: f32,
}

impl VecTransform {
    fn new(p: &StereometerParams, bounds: Rectangle) -> Self {
        let (cx, cy) = (
            bounds.x + bounds.width * 0.5,
            bounds.y + bounds.height * 0.5,
        );
        let (sin_t, cos_t) = (f32::from(p.rotation) * std::f32::consts::FRAC_PI_4).sin_cos();
        let corner_k = match p.scale {
            StereometerScale::Linear => 1.0,
            StereometerScale::Exponential => {
                let r = p.scale_range.max(f32::EPSILON);
                (0.5 + r) / (r * std::f32::consts::SQRT_2)
            }
        };
        let axis_extent = cos_t.abs().max(sin_t.abs());
        let corner_extent = corner_k * (cos_t.abs() + sin_t.abs());
        let rot_scale = 1.0 / axis_extent.max(corner_extent).max(f32::EPSILON);
        let radius = (bounds.width.min(bounds.height) * 0.5) - 2.0;
        Self {
            cx,
            cy,
            sin_t,
            cos_t,
            rot_scale,
            radius,
            flip: p.flip,
            scale: p.scale,
            scale_range: p.scale_range,
        }
    }

    fn project(&self, l: f32, r: f32) -> (f32, f32) {
        let (l, r) = scale_point(self.scale, l, r, self.scale_range);
        self.apply_rotation(l, r)
    }

    fn apply_rotation(&self, l: f32, r: f32) -> (f32, f32) {
        let (l, r) = if self.flip { (r, l) } else { (l, r) };
        let l = l * self.rot_scale;
        let r = r * self.rot_scale;
        (
            self.cx + (l * self.cos_t + r * self.sin_t) * self.radius,
            self.cy + (l * self.sin_t - r * self.cos_t) * self.radius,
        )
    }
}

impl StereometerPrimitive {
    fn add_grid_vertices(&self, vertices: &mut Vec<SdfVertex>, t: &VecTransform, clip: ClipTransform) {
        let grid_color = self.params.palette[8];
        if grid_color[3] < f32::EPSILON {
            return;
        }
        let mut draw_line = |(ax, ay): (f32, f32), (bx, by): (f32, f32)| {
            for seg in 0..GRID_SEGMENTS {
                let t0 = seg as f32 / GRID_SEGMENTS as f32;
                let t1 = (seg + 1) as f32 / GRID_SEGMENTS as f32;
                let p0 = t.project(ax + (bx - ax) * t0, ay + (by - ay) * t0);
                let p1 = t.project(ax + (bx - ax) * t1, ay + (by - ay) * t1);
                vertices.extend(line_vertices(
                    p0,
                    p1,
                    grid_color,
                    grid_color,
                    GRID_LINE_WIDTH,
                    clip,
                ));
            }
        };

        for ring in 1..=GRID_RINGS {
            let frac = ring as f32 / GRID_RINGS as f32;
            let radius = match t.scale {
                StereometerScale::Linear => frac,
                StereometerScale::Exponential => (t.scale_range * (frac - 1.0)).exp2(),
            };
            for (edge, &(x, y)) in GRID_CORNERS.iter().enumerate() {
                let (nx, ny) = GRID_CORNERS[(edge + 1) % GRID_CORNERS.len()];
                draw_line((x * radius, y * radius), (nx * radius, ny * radius));
            }
        }
        for (start, end) in GRID_AXES {
            draw_line(start, end);
        }
    }

    fn meter_bounds(p: &StereometerParams) -> (Rectangle, Option<Rectangle>) {
        let has_corr = p.correlation_meter != CorrelationMeterMode::Off;
        let left = p.correlation_meter_side == CorrelationMeterSide::Left;
        let margin = if has_corr {
            CORR_W + CORR_PAD + CORR_EDGE
        } else {
            0.0
        };
        let vec_bounds = Rectangle {
            x: p.bounds.x + if left { margin } else { 0.0 },
            width: (p.bounds.width - margin).max(0.0),
            ..p.bounds
        };
        let corr_bounds = has_corr.then(|| {
            let x = if left {
                p.bounds.x + CORR_EDGE
            } else {
                (p.bounds.x + p.bounds.width - CORR_W - CORR_EDGE).max(p.bounds.x)
            };
            Rectangle {
                x,
                y: p.bounds.y + CORR_VPAD,
                width: CORR_W,
                height: (p.bounds.height - 2.0 * CORR_VPAD).max(0.0),
            }
        });
        (vec_bounds, corr_bounds)
    }

    fn add_trace_vertices(
        out: &mut Vec<SdfVertex>,
        p: &StereometerParams,
        t: &VecTransform,
        clip: ClipTransform,
    ) {
        let [cr, cg, cb, ca] = p.palette[0];
        let dot_r = p.dot_radius;

        match p.mode {
            StereometerMode::DotCloud => {
                let count = p.points.len() as f32;
                out.extend(p.points.iter().enumerate().flat_map(|(i, &(l, r))| {
                    let (px, py) = t.apply_rotation(l, r);
                    let alpha = ca * (i + 1) as f32 / count;
                    dot_vertices(px, py, dot_r, [cr, cg, cb, alpha], clip, false)
                }));
            }
            StereometerMode::Lissajous if p.points.len() >= 2 => {
                let last = (p.points.len() - 1) as f32;
                out.extend(p.points.windows(2).enumerate().flat_map(|(i, w)| {
                    let p0 = t.apply_rotation(w[0].0, w[0].1);
                    let p1 = t.apply_rotation(w[1].0, w[1].1);
                    let (t0, t1) = (i as f32 / last, (i + 1) as f32 / last);
                    line_vertices(p0, p1, [cr, cg, cb, ca * t0], [cr, cg, cb, ca * t1], 1.5, clip)
                }));
            }
            StereometerMode::DotCloudBands => {
                for (band, pts) in p.band_points.iter().enumerate() {
                    let count = pts.len() as f32;
                    let [cr, cg, cb, ca] = p.palette[5 + band];
                    out.extend(pts.iter().enumerate().flat_map(|(i, &(l, r))| {
                        let (px, py) = t.apply_rotation(l, r);
                        let factor = ca * (i + 1) as f32 / count;
                        dot_vertices(px, py, dot_r, [cr * factor, cg * factor, cb * factor, 0.0], clip, true)
                    }));
                }
            }
            StereometerMode::Lissajous => {}
        }
    }

    fn add_correlation_vertices(
        out: &mut Vec<SdfVertex>,
        alpha: &mut Vec<f32>,
        p: &StereometerParams,
        bounds: Rectangle,
        clip: ClipTransform,
    ) {
        let is_single = p.correlation_meter == CorrelationMeterMode::SingleBand;
        let (bars, gap) = if is_single { (1, 0.0) } else { (3, BAND_GAP) };
        let bar_w = (CORR_W - gap * (bars - 1) as f32) / bars as f32;
        let corr_cy = bounds.y + bounds.height * 0.5;
        let half_h = bounds.height * 0.5;
        let val_y = |val: f32| corr_cy - val.clamp(-1.0, 1.0) * half_h;

        let y_min = bounds.y as i32;
        let height = (bounds.height as i32 + 1).max(0) as usize;
        let y_max = y_min + height as i32 - 1;

        let trail_len = if is_single {
            p.corr_trail.len()
        } else {
            p.band_trail.len()
        };
        let val = |band: usize, i: usize| {
            if is_single {
                p.corr_trail[i]
            } else {
                match band {
                    0 => p.band_trail[i].low,
                    1 => p.band_trail[i].mid,
                    _ => p.band_trail[i].high,
                }
            }
        };
        let color_for = |band: usize, value: f32| {
            if is_single {
                p.palette[if value < 0.0 { 4 } else { 3 }]
            } else {
                p.palette[5 + band]
            }
        };

        for band in 0..bars {
            let bx = bounds.x + band as f32 * (bar_w + gap);
            let (x0, x1) = (bx + 1.0, bx + bar_w - 1.0);

            out.extend(quad_vertices(
                bx,
                bounds.y,
                bx + bar_w,
                bounds.y + bounds.height,
                clip,
                p.palette[1],
            ));
            out.extend(quad_vertices(
                bx,
                corr_cy - 0.5,
                bx + bar_w,
                corr_cy + 0.5,
                clip,
                p.palette[2],
            ));

            if trail_len > 1 {
                alpha.resize(height, 0.0);
                alpha.fill(0.0);
                for j in 0..trail_len - 1 {
                    let a = (1.0 - (j + 1) as f32 / trail_len as f32).powf(2.4);
                    if a <= 0.0 {
                        continue;
                    }
                    let (y0, y1) = (val_y(val(band, j)), val_y(val(band, j + 1)));
                    let (top, bot) = (y0.min(y1) as i32, (y0.max(y1) + 2.0) as i32);
                    for sy in top.max(y_min)..=bot.min(y_max) {
                        let idx = (sy - y_min) as usize;
                        alpha[idx] = alpha[idx].max(a);
                    }
                }
                let base = color_for(band, val(band, 0));
                for (k, w) in alpha.windows(2).enumerate() {
                    if w[0] > 0.0 || w[1] > 0.0 {
                        let (mut c0, mut c1) = (base, base);
                        c0[3] *= w[0];
                        c1[3] *= w[1];
                        let y = (y_min + k as i32) as f32;
                        out.extend(gradient_quad_vertices(x0, y, x1, y + 1.0, clip, c0, c1));
                    }
                }
            }

            if trail_len > 0 {
                let current = val(band, 0);
                let y = val_y(current);
                out.extend(quad_vertices(
                    x0,
                    y - 1.0,
                    x1,
                    y + 1.0,
                    clip,
                    color_for(band, current),
                ));
            }
        }
    }

    fn build_vertices(&self, viewport: &Viewport, scratch: &mut GeometryScratch) {
        let clip = ClipTransform::from_viewport(viewport);
        let p = &self.params;
        let (vec_bounds, corr_bounds) = Self::meter_bounds(p);
        let transform = VecTransform::new(p, vec_bounds);
        let vertices = &mut scratch.vertices;
        self.add_grid_vertices(vertices, &transform, clip);
        Self::add_trace_vertices(vertices, p, &transform, clip);
        if let Some(bounds) = corr_bounds {
            Self::add_correlation_vertices(vertices, &mut scratch.scalars, p, bounds, clip);
        }
    }
}

sdf_primitive!(
    StereometerPrimitive(StereometerParams),
    Pipeline,
    u64,
    "Stereometer",
    TriangleList,
    |self| self.params.key
);
