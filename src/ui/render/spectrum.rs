use iced::Rectangle;
use iced::advanced::graphics::Viewport;
use std::sync::Arc;

use crate::sdf_primitive;
use crate::ui::render::common::{ClipTransform, SdfVertex, quad_vertices};
use crate::ui::render::geometry::{DEFAULT_FEATHER, build_aa_line_list};

#[derive(Debug, Clone)]
pub struct SpectrumParams {
    pub bounds: Rectangle,
    pub normalized_points: Arc<[[f32; 2]]>,
    pub secondary_points: Arc<[[f32; 2]]>,
    pub key: u64,
    pub line_color: [f32; 4],
    pub line_width: f32,
    pub secondary_line_color: [f32; 4],
    pub secondary_line_width: f32,
    pub highlight_threshold: f32,
    pub spectrum_palette: Vec<[f32; 4]>,
    // true = bar mode, false = line mode
    pub display_mode: bool,
    pub show_secondary_line: bool,
    pub bar_count: usize,
    pub bar_gap: f32,
}

#[derive(Debug)]
pub struct SpectrumPrimitive {
    params: SpectrumParams,
}

impl SpectrumPrimitive {
    pub fn new(params: SpectrumParams) -> Self {
        Self { params }
    }

    fn build_vertices(&self, viewport: &Viewport) -> Vec<SdfVertex> {
        let bounds = self.params.bounds;
        let clip = ClipTransform::from_viewport(viewport);

        if self.params.normalized_points.len() < 2 {
            return Vec::new();
        }

        if self.params.display_mode {
            self.build_bar_vertices(&clip, bounds)
        } else {
            self.build_line_vertices(&clip, bounds)
        }
    }

    fn build_line_vertices(&self, clip: &ClipTransform, bounds: Rectangle) -> Vec<SdfVertex> {
        let positions = to_cartesian_positions(bounds, self.params.normalized_points.as_ref());
        if positions.len() < 2 {
            return Vec::new();
        }

        let mut vertices = Vec::new();
        let baseline = bounds.y + bounds.height;

        push_highlight_columns(
            &mut vertices,
            clip,
            baseline,
            &positions,
            self.params.normalized_points.as_ref(),
            &self.params.spectrum_palette,
            self.params.highlight_threshold,
        );

        vertices.extend(build_aa_line_list(
            &positions,
            self.params.line_width,
            DEFAULT_FEATHER,
            self.params.line_color,
            clip,
        ));

        if self.params.show_secondary_line && self.params.secondary_points.len() >= 2 {
            let overlay_positions =
                to_cartesian_positions(bounds, self.params.secondary_points.as_ref());
            vertices.extend(build_aa_line_list(
                &overlay_positions,
                self.params.secondary_line_width,
                DEFAULT_FEATHER,
                self.params.secondary_line_color,
                clip,
            ));
        }

        vertices
    }

    fn build_bar_vertices(&self, clip: &ClipTransform, bounds: Rectangle) -> Vec<SdfVertex> {
        let p = &self.params;
        let bar_count = p.bar_count.max(4);
        let gap = p.bar_gap.clamp(0.0, 0.8);
        let unit = bounds.width / bar_count as f32;
        let (bar_w, offset) = (unit * (1.0 - gap), unit * gap * 0.5);
        let baseline = bounds.y + bounds.height;
        let y_at = |amp: f32| bounds.y + bounds.height * (1.0 - amp);

        let mut verts = Vec::with_capacity(bar_count * 12);
        for i in 0..bar_count {
            let (t0, t1) = (
                i as f32 / bar_count as f32,
                (i + 1) as f32 / bar_count as f32,
            );
            let amp = sample_max(&p.normalized_points, t0, t1);
            if amp < 1e-4 {
                continue;
            }
            let x0 = bounds.x + i as f32 * unit + offset;
            let color = palette_color(&p.spectrum_palette, amp, p.highlight_threshold);
            verts.extend_from_slice(&quad_vertices(
                x0,
                y_at(amp),
                x0 + bar_w,
                baseline,
                *clip,
                color,
            ));

            if p.show_secondary_line && p.secondary_points.len() >= 2 {
                let sec_y = y_at(sample_lerp(&p.secondary_points, (t0 + t1) * 0.5));
                let h = p.secondary_line_width.max(1.0) * 0.5;
                verts.extend_from_slice(&quad_vertices(
                    x0,
                    sec_y - h,
                    x0 + bar_w,
                    sec_y + h,
                    *clip,
                    p.secondary_line_color,
                ));
            }
        }
        verts
    }
}

fn to_cartesian_positions(bounds: Rectangle, pts: &[[f32; 2]]) -> Vec<(f32, f32)> {
    pts.iter()
        .map(|p| {
            (
                bounds.x + bounds.width * p[0],
                bounds.y + bounds.height * (1.0 - p[1]),
            )
        })
        .collect()
}

fn push_highlight_columns(
    vertices: &mut Vec<SdfVertex>,
    clip: &ClipTransform,
    baseline: f32,
    positions: &[(f32, f32)],
    normalized_points: &[[f32; 2]],
    palette: &[[f32; 4]],
    threshold: f32,
) {
    if palette.is_empty() {
        return;
    }
    for (seg, pts) in positions.windows(2).zip(normalized_points.windows(2)) {
        let amp = pts[0][1].max(pts[1][1]);
        let color = palette_color(palette, amp, threshold);
        if color[3] <= 0.0 {
            continue;
        }
        vertices.extend_from_slice(&quad_vertices(
            seg[0].0,
            seg[0].1.min(seg[1].1),
            seg[1].0,
            baseline,
            *clip,
            color,
        ));
    }
}

fn lerp_palette(palette: &[[f32; 4]], t: f32) -> [f32; 4] {
    let n = palette.len();
    if n < 2 {
        return palette.first().copied().unwrap_or([0.0; 4]);
    }
    let pos = t.clamp(0.0, 1.0) * (n - 1) as f32;
    let i = (pos as usize).min(n - 2);
    let f = pos - i as f32;
    std::array::from_fn(|c| palette[i][c] + (palette[i + 1][c] - palette[i][c]) * f)
}

#[inline]
fn palette_color(palette: &[[f32; 4]], amp: f32, threshold: f32) -> [f32; 4] {
    let intensity = (amp - threshold) / (1.0 - threshold).max(1e-6);
    lerp_palette(palette, intensity.clamp(0.0, 1.0))
}

fn sample_max(pts: &[[f32; 2]], t0: f32, t1: f32) -> f32 {
    let n = pts.len().saturating_sub(1).max(1);
    let i0 = (t0.clamp(0.0, 1.0) * n as f32) as usize;
    let i1 = ((t1.clamp(0.0, 1.0) * n as f32) as usize + 1).min(pts.len() - 1);
    pts.get(i0..=i1)
        .map(|s| s.iter().map(|p| p[1]).fold(0.0, f32::max))
        .unwrap_or(0.0)
}

fn sample_lerp(pts: &[[f32; 2]], t: f32) -> f32 {
    let n = pts.len().saturating_sub(1).max(1);
    let pos = t.clamp(0.0, 1.0) * n as f32;
    let i = (pos as usize).min(n.saturating_sub(1));
    let f = pos - i as f32;
    pts.get(i)
        .map(|a| a[1] * (1.0 - f) + pts.get(i + 1).map_or(a[1], |b| b[1]) * f)
        .unwrap_or(0.0)
}

sdf_primitive!(
    SpectrumPrimitive,
    Pipeline,
    u64,
    "Spectrum",
    TriangleList,
    |self| self.params.key
);
