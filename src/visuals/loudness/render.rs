// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use iced::Rectangle;
use iced::advanced::graphics::Viewport;

use crate::visuals::render::common::sdf_primitive;
use crate::visuals::render::common::{
    ClipTransform, GeometryFingerprint, GeometryScratch, bounds_fingerprint, line_instance,
    quad_instance,
};

pub(super) const DB_RANGE: (f32, f32) = (-60.0, 4.0);
pub(super) const GUIDE_LEVELS: [f32; 6] = [0.0, -6.0, -12.0, -18.0, -24.0, -36.0];

const FILL_COUNTS: [usize; 2] = [2, 1];
pub(super) const LEFT_PADDING: f32 = 28.0;
const RIGHT_PADDING: f32 = 64.0;
const GAP_FRACTION: f32 = 0.1;
const BAR_WIDTH_SCALE: f32 = 0.6;
const INNER_GAP_RATIO: f32 = 0.09;
const GUIDE_LENGTH: f32 = 4.0;
const GUIDE_THICKNESS: f32 = 1.0;
const GUIDE_PADDING: f32 = 3.0;
const THRESHOLD_THICKNESS: f32 = 1.5;
const PEAK_THICKNESS: f32 = 2.0;

#[derive(Debug, Clone, Copy)]
pub struct MeterFill {
    pub db: f32,
    pub segments: [(f32, [f32; 4]); 4],
    pub peak: Option<(f32, [f32; 4])>,
}

#[derive(Debug)]
pub struct LoudnessParams {
    pub key: u64,
    pub geometry_revision: u64,
    pub bounds: Rectangle,
    pub bg_color: [f32; 4],
    pub bars: [[MeterFill; 2]; 2],
    pub guide_color: [f32; 4],
}

pub(super) fn db_to_ratio(db: f32) -> f32 {
    let (min_db, max_db) = DB_RANGE;
    let range = max_db - min_db;
    if range <= f32::EPSILON { return 0.0; }
    let raw = ((db - min_db) / range).clamp(0.0, 1.0);
    raw.powf(0.9)
}

impl LoudnessParams {
    fn geometry_fingerprint(&self) -> GeometryFingerprint {
        bounds_fingerprint(self.geometry_revision, self.bounds)
    }

    pub fn meter_bounds(&self) -> Option<(f32, f32, f32)> {
        let bar_count = self.bars.len();
        let meter_width = (self.bounds.width - LEFT_PADDING - RIGHT_PADDING).max(0.0);
        if meter_width <= 0.0 { return None; }

        let gap = meter_width * GAP_FRACTION;
        let total_gap = gap * (bar_count - 1) as f32;
        let bar_slot = (meter_width - total_gap) / bar_count as f32;
        let bar_width = bar_slot * BAR_WIDTH_SCALE;
        let bar_offset = (bar_slot - bar_width) * 0.5;
        let stride = bar_width + gap;
        let meter_x = self.bounds.x + LEFT_PADDING + bar_offset;

        Some((meter_x, bar_width, stride))
    }
}

impl LoudnessPrimitive {
    fn build_vertices(&self, _viewport: &Viewport, scratch: &mut GeometryScratch) {
        let clip = ClipTransform::from_bounds(self.params.bounds);
        let Some((meter_x, bar_width, stride)) = self.params.meter_bounds() else {
            return;
        };

        let bounds = self.params.bounds;
        let y0 = bounds.y;
        let y1 = bounds.y + bounds.height;
        let height = y1 - y0;
        let y_of = |db| (y1 - height * db_to_ratio(db)).clamp(y0, y1);
        let bar_count = self.params.bars.len();
        let vertices = &mut scratch.instances;
        vertices.reserve(bar_count * 2 + FILL_COUNTS.iter().sum::<usize>() * 5 + GUIDE_LEVELS.len());

        for (i, (bar, &sub_bar_count)) in self.params.bars.iter().zip(&FILL_COUNTS).enumerate() {
            let sub_bar_count = sub_bar_count.min(bar.len());
            if sub_bar_count == 0 { continue; }
            let x0 = meter_x + i as f32 * stride;
            let x1 = x0 + bar_width;

            vertices.push(quad_instance(x0, y0, x1, y1, clip, self.params.bg_color));
            let inner_gap = if sub_bar_count <= 1 || bar_width <= 2.0 {
                0.0
            } else {
                (bar_width * INNER_GAP_RATIO)
                    .max(0.5)
                    .min(bar_width / (sub_bar_count - 1) as f32 * 0.5)
            };
            let total_inner = inner_gap * (sub_bar_count - 1) as f32;
            let seg_width = ((bar_width - total_inner) / sub_bar_count as f32).max(0.0);

            for (j, fill) in bar.iter().take(sub_bar_count).enumerate() {
                let sx0 = x0 + j as f32 * (seg_width + inner_gap);
                let sx1 = if j + 1 == sub_bar_count {
                    x1
                } else {
                    sx0 + seg_width
                };
                let value = fill.db.clamp(DB_RANGE.0, DB_RANGE.1);
                let mut lower = DB_RANGE.0;
                for &(ceiling, color) in &fill.segments {
                    let ceiling = ceiling.clamp(DB_RANGE.0, DB_RANGE.1);
                    let upper = value.min(ceiling);
                    if upper > lower {
                        vertices.push(quad_instance(
                            sx0,
                            y_of(upper),
                            sx1,
                            y_of(lower),
                            clip,
                            color,
                        ));
                    }
                    lower = lower.max(ceiling);
                    if value <= ceiling {
                        break;
                    }
                }

                if let Some((db, color)) = fill.peak {
                    let cy = y_of(db);
                    vertices.push(line_instance(
                        (sx0, cy),
                        (sx1, cy),
                        color,
                        color,
                        PEAK_THICKNESS,
                        clip,
                    ));
                }
            }
        }

        let guide_anchor = meter_x - GUIDE_PADDING;
        for db in GUIDE_LEVELS {
            let cy = y_of(db);
            vertices.push(line_instance(
                (guide_anchor - GUIDE_LENGTH, cy),
                (guide_anchor, cy),
                self.params.guide_color,
                self.params.guide_color,
                GUIDE_THICKNESS,
                clip,
            ));
        }

        let cy = y_of(0.0);
        for i in 0..bar_count {
            let x0 = meter_x + i as f32 * stride;
            let x1 = x0 + bar_width;
            vertices.push(line_instance(
                (x0, cy),
                (x1, cy),
                self.params.guide_color,
                self.params.guide_color,
                THRESHOLD_THICKNESS,
                clip,
            ));
        }

    }
}

sdf_primitive!(
    LoudnessPrimitive(LoudnessParams),
    Pipeline,
    u64,
    "Loudness",
    TriangleStrip,
    |self| self.params.key,
    self.params.geometry_fingerprint()
);
