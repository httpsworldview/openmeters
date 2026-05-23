// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use iced::Rectangle;
use iced::advanced::graphics::Viewport;

use crate::visuals::render::common::sdf_primitive;
use crate::visuals::render::common::{ClipTransform, SdfVertex, line_vertices, quad_vertices};

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

pub type MeterBar = Vec<MeterFill>;

#[derive(Debug, Clone)]
pub struct LoudnessParams {
    pub key: u64,
    pub bounds: Rectangle,
    pub min_db: f32,
    pub max_db: f32,
    pub bg_color: [f32; 4],
    pub bars: Vec<MeterBar>,
    pub guides: Vec<f32>,
    pub guide_color: [f32; 4],
    pub threshold_db: Option<f32>,
    pub left_padding: f32,
    pub right_padding: f32,
}

impl LoudnessParams {
    pub fn db_to_ratio(&self, db: f32) -> f32 {
        let range = self.max_db - self.min_db;
        if range <= f32::EPSILON {
            return 0.0;
        }
        let raw = ((db - self.min_db) / range).clamp(0.0, 1.0);
        raw.powf(0.9)
    }

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

fn sub_bar_gap(bar_width: f32, fill_count: usize) -> f32 {
    if fill_count <= 1 || bar_width <= 2.0 {
        return 0.0;
    }

    let desired = (bar_width * INNER_GAP_RATIO).max(0.5);
    let max_gap = bar_width / (fill_count - 1) as f32 * 0.5;
    desired.min(max_gap)
}

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
        let y_of = |db| (y1 - height * self.params.db_to_ratio(db)).clamp(y0, y1);
        let bar_count = self.params.bars.len();
        let fill_count: usize = self.params.bars.iter().map(|bar| bar.len()).sum();
        let mut vertices =
            Vec::with_capacity(bar_count * 12 + fill_count * 30 + self.params.guides.len() * 6);

        for (i, bar) in self.params.bars.iter().enumerate() {
            let x0 = meter_x + i as f32 * stride;
            let x1 = x0 + bar_width;

            vertices.extend(quad_vertices(x0, y0, x1, y1, clip, self.params.bg_color));
            let fill_count = bar.len();
            if fill_count > 0 {
                let inner_gap = sub_bar_gap(bar_width, fill_count);
                let total_inner = inner_gap * (fill_count - 1) as f32;
                let seg_width = ((bar_width - total_inner) / fill_count as f32).max(0.0);

                for (j, fill) in bar.iter().enumerate() {
                    let sx0 = x0 + j as f32 * (seg_width + inner_gap);
                    let sx1 = if j + 1 == fill_count {
                        x1
                    } else {
                        sx0 + seg_width
                    };
                    let value = fill.db.clamp(self.params.min_db, self.params.max_db);
                    let mut lower = self.params.min_db;
                    for &(ceiling, color) in &fill.segments {
                        let ceiling = ceiling.clamp(self.params.min_db, self.params.max_db);
                        let upper = value.min(ceiling);
                        if upper > lower {
                            vertices.extend(quad_vertices(
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
                        vertices.extend(line_vertices(
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
        }

        let guide_anchor = meter_x - GUIDE_PADDING;
        for &db in &self.params.guides {
            let cy = y_of(db);
            vertices.extend(line_vertices(
                (guide_anchor - GUIDE_LENGTH, cy),
                (guide_anchor, cy),
                self.params.guide_color,
                self.params.guide_color,
                GUIDE_THICKNESS,
                clip,
            ));
        }

        if let Some(db) = self.params.threshold_db {
            let cy = y_of(db);
            for i in 0..bar_count {
                let x0 = meter_x + i as f32 * stride;
                let x1 = x0 + bar_width;
                vertices.extend(line_vertices(
                    (x0, cy),
                    (x1, cy),
                    self.params.guide_color,
                    self.params.guide_color,
                    THRESHOLD_THICKNESS,
                    clip,
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
