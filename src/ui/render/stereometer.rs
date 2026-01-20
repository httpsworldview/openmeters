use iced::Rectangle;
use iced::advanced::graphics::Viewport;

use crate::dsp::stereometer::BandCorrelation;
use crate::sdf_primitive;
use crate::ui::render::common::{
    ClipTransform, SdfVertex, dot_vertices, gradient_quad_vertices, line_vertices, quad_vertices,
};
use crate::ui::settings::{CorrelationMeterMode, CorrelationMeterSide, StereometerMode};

const CORR_W: f32 = 28.0;
const CORR_PAD: f32 = 4.0;
const CORR_VPAD: f32 = 16.0;
const CORR_EDGE: f32 = 6.0;
const BAND_GAP: f32 = 2.0;

#[derive(Debug, Clone)]
pub struct StereometerParams {
    pub key: u64,
    pub bounds: Rectangle,
    pub points: Vec<(f32, f32)>,
    pub palette: [[f32; 4]; 8],
    pub mode: StereometerMode,
    pub rotation: i8,
    pub flip: bool,
    pub correlation_meter: CorrelationMeterMode,
    pub correlation_meter_side: CorrelationMeterSide,
    pub corr_trail: Vec<f32>,
    pub band_trail: Vec<BandCorrelation>,
}

#[derive(Debug)]
pub struct StereometerPrimitive(StereometerParams);

impl From<StereometerParams> for StereometerPrimitive {
    fn from(p: StereometerParams) -> Self {
        Self(p)
    }
}

impl StereometerPrimitive {
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

        let (cx, cy) = (
            vec_bounds.x + vec_bounds.width * 0.5,
            vec_bounds.y + vec_bounds.height * 0.5,
        );
        let theta = (p.rotation as f32) * std::f32::consts::FRAC_PI_4;
        let (sin_t, cos_t) = theta.sin_cos();
        let radius = (vec_bounds.width.min(vec_bounds.height) * 0.5) - 2.0;
        let [cr, cg, cb, ca] = p.palette[0];

        let xform = |l: f32, r: f32| {
            let (l, r) = if p.flip { (r, l) } else { (l, r) };
            (
                cx + (l * cos_t + r * sin_t).clamp(-1., 1.) * radius,
                cy + (l * sin_t - r * cos_t).clamp(-1., 1.) * radius,
            )
        };

        let mut v = Vec::new();

        match p.mode {
            StereometerMode::DotCloud => {
                let nf = p.points.len() as f32;
                v.extend(p.points.iter().enumerate().flat_map(|(i, &(l, r))| {
                    let (px, py) = xform(l, r);
                    dot_vertices(
                        px,
                        py,
                        1.5,
                        0.75,
                        [cr, cg, cb, ca * (i + 1) as f32 / nf],
                        clip,
                    )
                }));
            }
            StereometerMode::Lissajous if p.points.len() >= 2 => {
                let nm1 = (p.points.len() - 1) as f32;
                v.extend(p.points.windows(2).enumerate().flat_map(|(i, w)| {
                    let (p0, p1) = (xform(w[0].0, w[0].1), xform(w[1].0, w[1].1));
                    let (t0, t1) = (i as f32 / nm1, (i + 1) as f32 / nm1);
                    line_vertices(
                        p0,
                        p1,
                        [cr, cg, cb, ca * t0],
                        [cr, cg, cb, ca * t1],
                        1.5,
                        1.0,
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
