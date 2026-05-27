// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::visuals::render::common::{
    ClipTransform, SdfVertex, gradient_quad_vertices, line_vertices, sdf_primitive,
};
use iced::Rectangle;
use iced::advanced::graphics::Viewport;

use super::processor::NUM_PITCH_CLASSES;

const BAR_GAP_FRACTION: f32 = 0.12;
const PEAK_MARKER_THICKNESS: f32 = 1.5;
pub const VERTICAL_PADDING: f32 = 6.0;
pub const LABEL_AREA_HEIGHT: f32 = 16.0;

#[derive(Debug, Clone)]
pub struct ChromaParams {
    pub bounds: Rectangle,
    pub bins: [f32; NUM_PITCH_CLASSES],
    pub peak_bins: Option<[f32; NUM_PITCH_CLASSES]>,
    pub note_colors: [[f32; 4]; NUM_PITCH_CLASSES],
    pub peak_color: [f32; 4],
    pub note_names: [&'static str; NUM_PITCH_CLASSES],
    pub key: u64,
}

#[derive(Debug)]
pub struct ChromaPrimitive {
    params: ChromaParams,
}

impl ChromaPrimitive {
    pub fn new(params: ChromaParams) -> Self {
        Self { params }
    }

    fn build_vertices(&self, viewport: &Viewport) -> Vec<SdfVertex> {
        let clip = ClipTransform::from_viewport(viewport);
        let bounds = self.params.bounds;

        let usable_h = (bounds.height - VERTICAL_PADDING * 2.0 - LABEL_AREA_HEIGHT).max(0.0);
        let usable_w = bounds.width;

        if usable_h <= 0.0 || usable_w <= 0.0 {
            return Vec::new();
        }

        let bar_slot = usable_w / NUM_PITCH_CLASSES as f32;
        let gap = bar_slot * BAR_GAP_FRACTION;
        let bar_w = (bar_slot - gap).max(1.0);

        let top_y = bounds.y + VERTICAL_PADDING;
        let bottom_y = top_y + usable_h;

        let mut verts = Vec::with_capacity(NUM_PITCH_CLASSES * 18);

        for i in 0..NUM_PITCH_CLASSES {
            let x0 = bounds.x + i as f32 * bar_slot + gap * 0.5;
            let x1 = x0 + bar_w;
            let color = self.params.note_colors[i];
            let level = self.params.bins[i].clamp(0.0, 1.0);

            let bar_top = bottom_y - usable_h * level;

            let transparent = [color[0], color[1], color[2], 0.0];
            verts.extend(gradient_quad_vertices(
                x0, bar_top, x1, bottom_y, clip, color, transparent,
            ));

            if let Some(peaks) = self.params.peak_bins {
                let peak = peaks[i].clamp(0.0, 1.0);
                if peak > 0.01 {
                    let peak_y = bottom_y - usable_h * peak;
                    verts.extend(line_vertices(
                        (x0, peak_y),
                        (x1, peak_y),
                        self.params.peak_color,
                        self.params.peak_color,
                        PEAK_MARKER_THICKNESS,
                        clip,
                    ));
                }
            }
        }

        verts
    }
}

sdf_primitive!(
    ChromaPrimitive,
    Pipeline,
    u64,
    "Chroma",
    TriangleList,
    |self| self.params.key
);
