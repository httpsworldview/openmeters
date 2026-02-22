use iced::Rectangle;
use iced::advanced::graphics::Viewport;

use crate::dsp::stereometer::BandCorrelation;
use crate::sdf_primitive;
use crate::ui::render::common::{
    ClipTransform, SdfVertex, gradient_quad_vertices, line_vertices, quad_vertices,
};
use crate::ui::settings::{
    CorrelationMeterMode, CorrelationMeterSide, StereometerMode, StereometerScale,
};

pub fn scale_point(scale: StereometerScale, x: f32, y: f32, range: f32) -> (f32, f32) {
    match scale {
        StereometerScale::Linear => (x, y),
        StereometerScale::Exponential => {
            let len = x.hypot(y);
            if len < f32::EPSILON {
                return (0.0, 0.0);
            }
            let k = (len.max((-range).exp2()).log2() + range) / (-range * len);
            (k * x, k * y)
        }
    }
}

const CORR_W: f32 = 28.0;
const CORR_PAD: f32 = 4.0;
const CORR_VPAD: f32 = 16.0;
const CORR_EDGE: f32 = 6.0;
const BAND_GAP: f32 = 2.0;

#[inline]
fn dot_vertices(
    cx: f32,
    cy: f32,
    radius: f32,
    color: [f32; 4],
    clip: ClipTransform,
) -> [SdfVertex; 6] {
    let outer = radius + 1.0;
    let v = |px, py, ox, oy| SdfVertex {
        position: clip.to_clip(px, py),
        color,
        params: [ox, oy, radius, 0.0],
    };
    [
        v(cx - outer, cy - outer, -outer, -outer),
        v(cx - outer, cy + outer, -outer, outer),
        v(cx + outer, cy - outer, outer, -outer),
        v(cx + outer, cy - outer, outer, -outer),
        v(cx - outer, cy + outer, -outer, outer),
        v(cx + outer, cy + outer, outer, outer),
    ]
}

#[derive(Debug, Clone)]
pub struct StereometerParams {
    pub key: u64,
    pub bounds: Rectangle,
    pub points: Vec<(f32, f32)>,
    pub palette: [[f32; 4]; 9],
    pub mode: StereometerMode,
    pub scale: StereometerScale,
    pub scale_range: f32,
    pub rotation: i8,
    pub flip: bool,
    pub correlation_meter: CorrelationMeterMode,
    pub correlation_meter_side: CorrelationMeterSide,
    pub corr_trail: Vec<f32>,
    pub band_trail: Vec<BandCorrelation>,
}

#[derive(Debug)]
pub struct StereometerPrimitive(StereometerParams);

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
        let theta = (p.rotation as f32) * std::f32::consts::FRAC_PI_4;
        let (sin_t, cos_t) = theta.sin_cos();
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
        let (l, r) = self.apply_scale(l, r);
        self.apply_rotation(l, r)
    }

    // Data points arrive pre-scaled from the visualization layer.
    fn apply_rotation(&self, l: f32, r: f32) -> (f32, f32) {
        let (l, r) = if self.flip { (r, l) } else { (l, r) };
        let l = l * self.rot_scale;
        let r = r * self.rot_scale;
        (
            self.cx + (l * self.cos_t + r * self.sin_t) * self.radius,
            self.cy + (l * self.sin_t - r * self.cos_t) * self.radius,
        )
    }

    fn apply_scale(&self, x: f32, y: f32) -> (f32, f32) {
        scale_point(self.scale, x, y, self.scale_range)
    }
}

impl From<StereometerParams> for StereometerPrimitive {
    fn from(p: StereometerParams) -> Self {
        Self(p)
    }
}

impl StereometerPrimitive {
    pub fn new(params: StereometerParams) -> Self {
        Self(params)
    }

    fn grid_vertices(&self, t: &VecTransform, clip: ClipTransform) -> Vec<SdfVertex> {
        let grid_color = self.0.palette[8];
        if grid_color[3] < f32::EPSILON {
            return Vec::new();
        }

        const RINGS: usize = 3;
        const SEGMENTS: usize = 16;
        const WIDTH: f32 = 1.0;
        const CORNERS: [(f32, f32); 4] = [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)];

        let mut v = Vec::new();

        for ring in 1..=RINGS {
            let r = ring as f32 / RINGS as f32;
            for edge in 0..4 {
                let (ax, ay) = (CORNERS[edge].0 * r, CORNERS[edge].1 * r);
                let next = CORNERS[(edge + 1) % 4];
                let (bx, by) = (next.0 * r, next.1 * r);
                for seg in 0..SEGMENTS {
                    let t0 = seg as f32 / SEGMENTS as f32;
                    let t1 = (seg + 1) as f32 / SEGMENTS as f32;
                    let p0 = t.project(ax + (bx - ax) * t0, ay + (by - ay) * t0);
                    let p1 = t.project(ax + (bx - ax) * t1, ay + (by - ay) * t1);
                    v.extend(line_vertices(p0, p1, grid_color, grid_color, WIDTH, clip));
                }
            }
        }

        const AXES: [((f32, f32), (f32, f32)); 2] =
            [((1.0, 1.0), (-1.0, -1.0)), ((1.0, -1.0), (-1.0, 1.0))];
        for (start, end) in AXES {
            for seg in 0..SEGMENTS {
                let t0 = seg as f32 / SEGMENTS as f32;
                let t1 = (seg + 1) as f32 / SEGMENTS as f32;
                let p0 = t.project(
                    start.0 + (end.0 - start.0) * t0,
                    start.1 + (end.1 - start.1) * t0,
                );
                let p1 = t.project(
                    start.0 + (end.0 - start.0) * t1,
                    start.1 + (end.1 - start.1) * t1,
                );
                v.extend(line_vertices(p0, p1, grid_color, grid_color, WIDTH, clip));
            }
        }

        v
    }

    fn build_vertices(&self, viewport: &Viewport) -> Vec<SdfVertex> {
        let clip = ClipTransform::from_viewport(viewport);
        let p = &self.0;

        let is_single = p.correlation_meter == CorrelationMeterMode::SingleBand;
        let has_corr = p.correlation_meter != CorrelationMeterMode::Off;
        let margin = if has_corr {
            CORR_W + CORR_PAD + CORR_EDGE
        } else {
            0.0
        };

        let (vec_bounds, corr_bounds) = {
            let left = p.correlation_meter_side == CorrelationMeterSide::Left;
            let vb = Rectangle {
                x: p.bounds.x + if left { margin } else { 0.0 },
                width: (p.bounds.width - margin).max(0.0),
                ..p.bounds
            };
            let cb = if has_corr {
                let cx = if left {
                    p.bounds.x + CORR_EDGE
                } else {
                    (p.bounds.x + p.bounds.width - CORR_W - CORR_EDGE).max(p.bounds.x)
                };
                Rectangle {
                    x: cx,
                    y: p.bounds.y + CORR_VPAD,
                    width: CORR_W,
                    height: (p.bounds.height - 2.0 * CORR_VPAD).max(0.0),
                }
            } else {
                Rectangle::default()
            };
            (vb, cb)
        };

        let t = VecTransform::new(p, vec_bounds);
        let [cr, cg, cb, ca] = p.palette[0];

        let mut v = self.grid_vertices(&t, clip);

        match p.mode {
            StereometerMode::DotCloud => {
                let nf = p.points.len() as f32;
                v.extend(p.points.iter().enumerate().flat_map(|(i, &(l, r))| {
                    let (px, py) = t.apply_rotation(l, r);
                    dot_vertices(px, py, 1.5, [cr, cg, cb, ca * (i + 1) as f32 / nf], clip)
                }));
            }
            StereometerMode::Lissajous if p.points.len() >= 2 => {
                let nm1 = (p.points.len() - 1) as f32;
                v.extend(p.points.windows(2).enumerate().flat_map(|(i, w)| {
                    let (p0, p1) = (
                        t.apply_rotation(w[0].0, w[0].1),
                        t.apply_rotation(w[1].0, w[1].1),
                    );
                    let (t0, t1) = (i as f32 / nm1, (i + 1) as f32 / nm1);
                    line_vertices(
                        p0,
                        p1,
                        [cr, cg, cb, ca * t0],
                        [cr, cg, cb, ca * t1],
                        1.5,
                        clip,
                    )
                }));
            }
            _ => {}
        }

        if !has_corr {
            return v;
        }

        let (bars, gap) = if is_single { (1, 0.0) } else { (3, BAND_GAP) };
        let bar_w = (CORR_W - gap * (bars - 1) as f32) / bars as f32;
        let corr_cy = corr_bounds.y + corr_bounds.height * 0.5;
        let half_h = corr_bounds.height * 0.5;
        let val_y = |val: f32| corr_cy - val.clamp(-1.0, 1.0) * half_h;

        let y_min = corr_bounds.y as i32;
        let height = (corr_bounds.height as i32 + 1).max(0) as usize;
        let y_max = y_min + height as i32 - 1;
        let mut alpha = vec![0.0f32; height];

        let trail_len = if is_single {
            p.corr_trail.len()
        } else {
            p.band_trail.len()
        };
        let val = |band: usize, i: usize| {
            if is_single {
                p.corr_trail.get(i).copied().unwrap_or(0.0)
            } else {
                p.band_trail.get(i).map(|b| b[band]).unwrap_or(0.0)
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
            let bx = corr_bounds.x + band as f32 * (bar_w + gap);
            let (x0, x1) = (bx + 1.0, bx + bar_w - 1.0);

            v.extend(quad_vertices(
                bx,
                corr_bounds.y,
                bx + bar_w,
                corr_bounds.y + corr_bounds.height,
                clip,
                p.palette[1],
            ));
            v.extend(quad_vertices(
                bx,
                corr_cy - 0.5,
                bx + bar_w,
                corr_cy + 0.5,
                clip,
                p.palette[2],
            ));

            if trail_len > 1 {
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
                        v.extend(gradient_quad_vertices(x0, y, x1, y + 1.0, clip, c0, c1));
                    }
                }
            }

            if trail_len > 0 {
                let current = val(band, 0);
                let y = val_y(current);
                v.extend(quad_vertices(
                    x0,
                    y - 1.0,
                    x1,
                    y + 1.0,
                    clip,
                    color_for(band, current),
                ));
            }
        }
        v
    }
}

sdf_primitive!(
    StereometerPrimitive,
    Pipeline,
    u64,
    "Stereometer",
    TriangleList,
    |self| self.0.key
);
