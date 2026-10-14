// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use iced::Rectangle;
use std::ops::Range;
use std::sync::Arc;

use super::processor::TRACE_COUNT;
use crate::util::color::rgba_with_alpha;
use crate::visuals::render::common::{
    ClipTransform, GeometryScratch, amplitude_y, bounds_fingerprint, decimate_line,
    extend_filled_line,
};
use crate::visuals::render::common::{SdfPipeline, sdf_primitive};

const FILL_ALPHA: f32 = 0.3;

#[derive(Debug, Clone)]
pub struct OscilloscopeParams {
    pub geometry: crate::visuals::GeometryKey,
    pub bounds: Rectangle,
    pub channels: Range<usize>,
    pub samples_per_channel: usize,
    pub slots: [usize; TRACE_COUNT],
    pub samples: Arc<[f32]>,
    pub colors: [[f32; 4]; TRACE_COUNT],
}

impl OscilloscopeParams {
    fn build_vertices(&self, scratch: &mut GeometryScratch) {
        const STROKE_WIDTH: f32 = 1.0;
        const EDGE_INSET: f32 = STROKE_WIDTH * 0.5 + 0.5;

        let samples_per_channel = self.samples_per_channel;
        let bounds = self.bounds;
        let clip = ClipTransform::from_bounds(bounds);
        let trace = bounds.shrink(iced::padding::vertical(EDGE_INSET.min(bounds.height * 0.5)));
        let baseline = amplitude_y(trace, 0.0);
        let step = bounds.width.max(1.0) / (samples_per_channel - 1) as f32;
        let pixel_width = bounds.width.ceil().max(1.0) as usize;

        let vertices = &mut scratch.instances;
        let positions = &mut scratch.points;

        for channel_idx in self.channels.clone().rev() {
            let start = channel_idx * samples_per_channel;
            let channel_samples = &self.samples[start..start + samples_per_channel];
            let color = self.colors[self.slots[channel_idx]];

            positions.clear();
            positions.extend(
                channel_samples
                    .iter()
                    .enumerate()
                    .filter(|(_, sample)| sample.is_finite())
                    .map(|(i, &sample)| {
                        (
                            bounds.x + i as f32 * step,
                            amplitude_y(trace, sample.clamp(-1.0, 1.0)),
                        )
                    }),
            );
            decimate_line(positions, pixel_width * 2);

            extend_filled_line(
                vertices,
                positions,
                baseline,
                STROKE_WIDTH,
                color,
                [
                    rgba_with_alpha(color, color[3] * FILL_ALPHA),
                    rgba_with_alpha(color, 0.0),
                ],
                clip,
            );
        }
    }
}

sdf_primitive!(
    OscilloscopeParams,
    SdfPipeline,
    "Oscilloscope",
    |self| self.geometry.id,
    Some(bounds_fingerprint(self.geometry.revision, self.bounds))
);

#[cfg(test)]
mod tests {
    use super::*;
    use iced::{Point, Size};

    #[test]
    fn trace_spans_lane_edge_to_edge() {
        let bounds = Rectangle::new(Point::new(10.25, 20.5), Size::new(300.5, 50.5));
        let params = OscilloscopeParams {
            geometry: crate::visuals::GeometryKey::new(),
            bounds,
            channels: 0..1,
            samples_per_channel: 4096,
            slots: [0, 1],
            samples: (0..4096).map(|i| [1.0, -1.0][i % 2]).collect(),
            colors: [[1.0; 4]; TRACE_COUNT],
        };
        let mut scratch = GeometryScratch::default();
        params.build_vertices(&mut scratch);
        let points = &scratch.points;
        let assert_near = |actual: f32, edge: f32| assert!((actual - edge).abs() < 1e-3);
        assert_near(points[0].0, bounds.x);
        assert_near(points[points.len() - 1].0, bounds.x + bounds.width);
        let top = points.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
        let bottom = points.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
        assert_near(top, bounds.y + 1.0);
        assert_near(bottom, bounds.y + bounds.height - 1.0);
    }
}
