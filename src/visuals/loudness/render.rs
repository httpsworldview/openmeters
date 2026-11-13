// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use iced::{Rectangle, Size};

use crate::visuals::render::common::sdf_primitive;
use crate::visuals::render::common::{
    ClipTransform, GeometryScratch, bounds_fingerprint, line_instance, quad_instance,
};

pub(super) const DB_RANGE: (f32, f32) = (-60.0, 4.0);
pub(super) const GUIDE_LEVELS: [f32; 6] = [0.0, -6.0, -12.0, -18.0, -24.0, -36.0];

pub(super) const LEFT_PADDING: f32 = 28.0;
const VALUE_LABEL_SIZE: Size = Size::new(68.0, 20.0);
const VALUE_LINE_HEIGHTS: &[f32] = &[VALUE_LABEL_SIZE.height, 14.0];
const VALUE_LABEL_GAP: f32 = 4.0;
const MIN_METER_AREA_WIDTH: f32 = 28.0;
pub(crate) const MIN_LABELED_WIDTH: f32 =
    LEFT_PADDING + MIN_METER_AREA_WIDTH + VALUE_LABEL_GAP + VALUE_LABEL_SIZE.width;
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

pub type Wick = ([f32; 2], [f32; 4]);

#[derive(Debug)]
pub struct LoudnessParams {
    pub geometry: crate::visuals::GeometryKey,
    pub bounds: Rectangle,
    pub bg_color: [f32; 4],
    pub bars: [MeterFill; 3],
    // One per bar group.
    pub wicks: [Option<Wick>; 2],
    pub guide_color: [f32; 4],
}

fn db_y(bounds: Rectangle, db: f32) -> f32 {
    let (min_db, max_db) = DB_RANGE;
    let raw = ((db - min_db) / (max_db - min_db)).clamp(0.0, 1.0);
    bounds.y + bounds.height * (1.0 - raw.powf(0.9))
}

pub(super) fn label_top(bounds: Rectangle, db: f32, height: f32) -> f32 {
    let max_top = (bounds.y + bounds.height - height).max(bounds.y);
    (db_y(bounds, db) - height * 0.5).clamp(bounds.y, max_top)
}

pub(super) fn value_label_bounds(
    bounds: Rectangle,
    (meter_x, bar_width, stride): (f32, f32, f32),
    value: f32,
    range: bool,
) -> Option<(Rectangle, &'static [f32])> {
    let fit = |lines: usize| {
        let heights = &VALUE_LINE_HEIGHTS[..lines];
        let height = heights.iter().sum();
        let rect = Rectangle {
            x: meter_x + stride + bar_width + VALUE_LABEL_GAP,
            y: label_top(bounds, value, height),
            width: VALUE_LABEL_SIZE.width,
            height,
        };
        rect.is_within(&bounds).then_some((rect, heights))
    };
    fit(1 + usize::from(range)).or_else(|| fit(1))
}

impl LoudnessParams {
    fn bar_groups(&self) -> [&[MeterFill]; 2] {
        [&self.bars[..2], &self.bars[2..]]
    }

    pub fn meter_bounds(&self) -> Option<(f32, f32, f32)> {
        let bar_count = self.bar_groups().len();
        let left_padding = LEFT_PADDING.min(self.bounds.width * 0.5);
        let right_padding = if self.bounds.width >= MIN_LABELED_WIDTH {
            VALUE_LABEL_GAP + VALUE_LABEL_SIZE.width
        } else {
            0.0
        };
        let meter_width = (self.bounds.width - left_padding - right_padding).max(0.0);
        if meter_width <= 0.0 {
            return None;
        }

        let gap = meter_width * GAP_FRACTION;
        let total_gap = gap * (bar_count - 1) as f32;
        let bar_slot = (meter_width - total_gap) / bar_count as f32;
        let bar_width = bar_slot * BAR_WIDTH_SCALE;
        let bar_offset = (bar_slot - bar_width) * 0.5;
        let stride = bar_width + gap;
        let meter_x = self.bounds.x + left_padding + bar_offset;

        Some((meter_x, bar_width, stride))
    }

    fn build_vertices(&self, scratch: &mut GeometryScratch) {
        let clip = ClipTransform::from_bounds(self.bounds);
        let Some((meter_x, bar_width, stride)) = self.meter_bounds() else {
            return;
        };

        let bounds = self.bounds;
        let (y0, y1) = (bounds.y, bounds.y + bounds.height);
        let y_of = |db| db_y(bounds, db);
        let hline =
            |x0, x1, y, width, color| line_instance((x0, y), (x1, y), color, color, width, clip);
        let bars = self.bar_groups();
        let bar_count = bars.len();
        let vertices = &mut scratch.instances;
        // Wicks fill the middle half of the label gap, or of a tighter bar gap.
        let space = VALUE_LABEL_GAP.min(stride - bar_width);

        for (i, (bar, wick)) in bars.into_iter().zip(self.wicks).enumerate() {
            let sub_bar_count = bar.len();
            let x0 = meter_x + i as f32 * stride;
            let x1 = x0 + bar_width;

            vertices.push(quad_instance(x0, y0, x1, y1, clip, self.bg_color));
            let inner_gap = if sub_bar_count <= 1 || bar_width <= 2.0 {
                0.0
            } else {
                (bar_width * INNER_GAP_RATIO)
                    .max(0.5)
                    .min(bar_width / (sub_bar_count - 1) as f32 * 0.5)
            };
            let total_inner = inner_gap * (sub_bar_count - 1) as f32;
            let seg_width = ((bar_width - total_inner) / sub_bar_count as f32).max(0.0);

            for (j, fill) in bar.iter().enumerate() {
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
                        let (top, bottom) = (y_of(upper), y_of(lower));
                        vertices.push(quad_instance(sx0, top, sx1, bottom, clip, color));
                    }
                    lower = lower.max(ceiling);
                    if value <= ceiling {
                        break;
                    }
                }

                if let Some((db, color)) = fill.peak {
                    vertices.push(hline(sx0, sx1, y_of(db), PEAK_THICKNESS, color));
                }
            }

            if let Some(([low, high], color)) = wick {
                let (top, bottom) = (y_of(high), y_of(low));
                // A zero range still shows.
                let pad = ((space * 0.5 - (bottom - top)) * 0.5).max(0.0);
                let (x0, x1) = (x1 + space * 0.25, x1 + space * 0.75);
                vertices.push(quad_instance(x0, top - pad, x1, bottom + pad, clip, color));
            }
        }

        let guide = self.guide_color;
        let guide_x1 = meter_x - GUIDE_PADDING;
        for db in GUIDE_LEVELS {
            let x0 = guide_x1 - GUIDE_LENGTH;
            vertices.push(hline(x0, guide_x1, y_of(db), GUIDE_THICKNESS, guide));
        }

        let cy = y_of(0.0);
        for i in 0..bar_count {
            let x0 = meter_x + i as f32 * stride;
            vertices.push(hline(x0, x0 + bar_width, cy, THRESHOLD_THICKNESS, guide));
        }
    }
}

sdf_primitive!(
    LoudnessParams,
    "Loudness",
    |self| self.geometry.id,
    Some(bounds_fingerprint(self.geometry.revision, self.bounds))
);
