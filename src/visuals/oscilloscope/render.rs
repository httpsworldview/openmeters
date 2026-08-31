// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use iced::Rectangle;
use std::sync::Arc;

use super::processor::TRACE_COUNT;
use crate::util::color::rgba_with_alpha;
use crate::visuals::render::common::sdf_primitive;
use crate::visuals::render::common::{
    ChannelLayout, ClipTransform, GeometryScratch, bounds_fingerprint,
    decimate_finite_ordered_line_in_place, extend_filled_line,
};

const FILL_ALPHA: f32 = 0.15;

#[derive(Debug, Clone)]
pub struct OscilloscopeParams {
    pub geometry: crate::visuals::GeometryKey,
    pub bounds: Rectangle,
    pub channels: usize,
    pub samples_per_channel: usize,
    pub slots: [usize; TRACE_COUNT],
    pub samples: Arc<[f32]>,
    pub colors: [[f32; 4]; TRACE_COUNT],
    pub stacked: bool,
}

impl OscilloscopeParams {
    fn build_vertices(&self, scratch: &mut GeometryScratch) {
        const AMPLITUDE_SCALE: f32 = 0.9;
        const STROKE_WIDTH: f32 = 1.0;

        let samples_per_channel = self.samples_per_channel;
        let channels = self.channels;
        let bounds = self.bounds;
        let clip = ClipTransform::from_bounds(bounds);

        let layout = ChannelLayout::new(bounds, if self.stacked { 1 } else { channels }, AMPLITUDE_SCALE);
        let step = bounds.width.max(1.0) / (samples_per_channel - 1) as f32;
        let pixel_width = bounds.width.ceil().max(1.0) as usize;

        let vertices = &mut scratch.instances;
        let positions = &mut scratch.points;

        for i in 0..channels {
            let channel_idx = if self.stacked { channels - 1 - i } else { i };
            let start = channel_idx * samples_per_channel;
            let channel_samples = &self.samples[start..start + samples_per_channel];
            let color = self.colors[self.slots[channel_idx]];
            let center = layout.center_y(if self.stacked { 0 } else { channel_idx });

            positions.clear();
            positions.extend(
                channel_samples
                    .iter()
                    .enumerate()
                    .filter(|(_, sample)| sample.is_finite())
                    .map(|(i, &sample)| {
                        (
                            bounds.x + i as f32 * step,
                            center - sample.clamp(-1.0, 1.0) * layout.amplitude_scale,
                        )
                    }),
            );
            decimate_finite_ordered_line_in_place(positions, pixel_width * 2);

            extend_filled_line(
                vertices,
                positions,
                center,
                STROKE_WIDTH,
                color,
                rgba_with_alpha(color, color[3] * FILL_ALPHA),
                clip,
            );
        }
    }
}

sdf_primitive!(
    OscilloscopeParams,
    "Oscilloscope",
    |self| self.geometry.id,
    bounds_fingerprint(self.geometry.revision, self.bounds)
);
