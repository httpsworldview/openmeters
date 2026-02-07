// Rendering code for the loudness meters.

use iced::Rectangle;
use iced::advanced::graphics::Viewport;

use crate::sdf_primitive;
use crate::ui::render::common::{ClipTransform, SdfVertex, quad_vertices};

const GAP_FRACTION: f32 = 0.1;
const BAR_WIDTH_SCALE: f32 = 0.6;
const INNER_GAP_RATIO: f32 = 0.09;
const GUIDE_LENGTH: f32 = 4.0;
const GUIDE_THICKNESS: f32 = 1.0;
const GUIDE_PADDING: f32 = 3.0;
const THRESHOLD_THICKNESS: f32 = 1.5;

// A single meter bar with background and fill segments.
#[derive(Debug, Clone)]
pub struct MeterBar {
    pub bg_color: [f32; 4],
    pub fills: Vec<(f32, [f32; 4])>,
}

// Parameters for rendering the loudness meter.
#[derive(Debug, Clone)]
pub struct LoudnessParams {
    pub key: u64,
    pub bounds: Rectangle,
    pub min_db: f32,
    pub max_db: f32,
    pub bars: Vec<MeterBar>,
    pub guides: Vec<f32>,
    pub guide_color: [f32; 4],
    pub threshold_db: Option<f32>,
    pub left_padding: f32,
    pub right_padding: f32,
}

impl LoudnessParams {
    // Convert dB value to 0..1 ratio with visual scaling.
    pub fn db_to_ratio(&self, db: f32) -> f32 {
        let range = self.max_db - self.min_db;
        if range <= f32::EPSILON {
            return 0.0;
        }
        let raw = ((db - self.min_db) / range).clamp(0.0, 1.0);
        raw.powf(0.9)
    }

    // Get horizontal bounds of the meter area.
    pub fn meter_bounds(&self) -> Option<(f32, f32, f32)> {
        let bar_count = self.bars.len();
        if bar_count == 0 {
            return None;
        }

        let meter_width = (self.bounds.width - self.left_padding - self.right_padding).max(0.0);
        if meter_width <= 0.0 {
            return None;
        }

        let gap = meter_width * GAP_FRACTION;
        let total_gap = gap * (bar_count - 1) as f32;
        let bar_slot = (meter_width - total_gap) / bar_count as f32;
        let bar_width = bar_slot * BAR_WIDTH_SCALE;
        let bar_offset = (bar_slot - bar_width) * 0.5;
        let stride = bar_width + gap;
        let meter_x = self.bounds.x + self.left_padding + bar_offset;

        Some((meter_x, bar_width, stride))
    }
}

// Custom primitive that draws a loudness meter.
#[derive(Debug)]
pub struct LoudnessPrimitive {
    pub params: LoudnessParams,
}

impl LoudnessPrimitive {
    pub fn new(params: LoudnessParams) -> Self {
        Self { params }
    }

    fn build_vertices(&self, viewport: &Viewport) -> Vec<SdfVertex> {
        let clip = ClipTransform::from_viewport(viewport);
        let Some((meter_x, bar_width, stride)) = self.params.meter_bounds() else {
            return Vec::new();
        };

        let bounds = self.params.bounds;
        let y0 = bounds.y;
        let y1 = bounds.y + bounds.height;
        let height = y1 - y0;
        let bar_count = self.params.bars.len();

        let mut vertices = Vec::with_capacity(bar_count * 18 + self.params.guides.len() * 6 + 12);

        for (i, bar) in self.params.bars.iter().enumerate() {
            let x0 = meter_x + i as f32 * stride;
            let x1 = x0 + bar_width;

            vertices.extend(quad_vertices(x0, y0, x1, y1, clip, bar.bg_color));
            let fill_count = bar.fills.len();
            if fill_count > 0 {
                let inner_gap = if fill_count > 1 && bar_width > 2.0 {
                    (bar_width * INNER_GAP_RATIO).min(bar_width * 0.4).max(0.5)
                } else {
                    0.0
                };
                let total_inner = inner_gap * (fill_count - 1) as f32;
                let seg_width = (bar_width - total_inner) / fill_count as f32;

                for (j, &(db, color)) in bar.fills.iter().enumerate() {
                    let ratio = self.params.db_to_ratio(db);
                    let fill_y = y1 - height * ratio;
                    let sx0 = x0 + j as f32 * (seg_width + inner_gap);
                    let sx1 = if j + 1 == fill_count {
                        x1
                    } else {
                        sx0 + seg_width
                    };
                    vertices.extend(quad_vertices(sx0, fill_y, sx1, y1, clip, color));
                }
            }
        }

        let guide_anchor = meter_x - GUIDE_PADDING;
        for &db in &self.params.guides {
            let ratio = self.params.db_to_ratio(db);
            let cy = y1 - height * ratio;
            let half = GUIDE_THICKNESS * 0.5;
            vertices.extend(quad_vertices(
                guide_anchor - GUIDE_LENGTH,
                (cy - half).max(y0),
                guide_anchor,
                (cy + half).min(y1),
                clip,
                self.params.guide_color,
            ));
        }

        if let Some(db) = self.params.threshold_db {
            let ratio = self.params.db_to_ratio(db);
            let cy = y1 - height * ratio;
            let half = THRESHOLD_THICKNESS * 0.5;
            for i in 0..bar_count {
                let x0 = meter_x + i as f32 * stride;
                let x1 = x0 + bar_width;
                vertices.extend(quad_vertices(
                    x0,
                    (cy - half).max(y0),
                    x1,
                    (cy + half).min(y1),
                    clip,
                    self.params.guide_color,
                ));
            }
        }

        vertices
    }
}

sdf_primitive!(
    LoudnessPrimitive,
    Pipeline,
    u64,
    "Loudness",
    TriangleList,
    |self| self.params.key
);
