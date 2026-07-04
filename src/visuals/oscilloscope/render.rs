// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use iced::Rectangle;
use iced::advanced::graphics::Viewport;

use super::processor::TRACE_COUNT;
use crate::util::color::rgba_with_alpha;
use crate::visuals::render::common::sdf_primitive;
use crate::visuals::render::common::{
    ChannelLayout, ClipTransform, GeometryScratch, decimate_line_in_place, extend_filled_line,
};

#[derive(Debug, Clone)]
pub struct OscilloscopeParams {
    pub key: u64,
    pub bounds: Rectangle,
    pub channels: usize,
    pub samples_per_channel: usize,
    pub slots: [usize; TRACE_COUNT],
    pub samples: Vec<f32>,
    pub colors: [[f32; 4]; TRACE_COUNT],
    pub stacked: bool,
    pub fill_alpha: f32,
}

impl OscilloscopePrimitive {
    fn build_vertices(&self, viewport: &Viewport, scratch: &mut GeometryScratch) {
        const VERTICAL_PADDING: f32 = 8.0;
        const CHANNEL_GAP: f32 = 12.0;
        const AMPLITUDE_SCALE: f32 = 0.9;
        const STROKE_WIDTH: f32 = 1.0;

        let samples_per_channel = self.params.samples_per_channel;
        let channels = self.params.channels.min(TRACE_COUNT);

        if channels == 0
            || samples_per_channel < 2
            || self.params.samples.len() < channels * samples_per_channel
        {
            return;
        }

        let bounds = self.params.bounds;
        let clip = ClipTransform::from_viewport(viewport);

        let layout = ChannelLayout::new(
            bounds,
            if self.params.stacked { 1 } else { channels },
            VERTICAL_PADDING,
            CHANNEL_GAP,
            AMPLITUDE_SCALE,
        );
        let step = bounds.width.max(1.0) / (samples_per_channel.saturating_sub(1) as f32).max(1.0);
        let pixel_width = bounds.width.ceil().max(1.0) as usize;

        let vertices = &mut scratch.vertices;
        let positions = &mut scratch.points;

        for i in 0..channels {
            let channel_idx = if self.params.stacked { channels - 1 - i } else { i };
            let start = channel_idx * samples_per_channel;
            let channel_samples = &self.params.samples[start..start + samples_per_channel];
            let color = self.params.colors[self.params.slots[channel_idx].min(TRACE_COUNT - 1)];
            let center = layout.center_y(if self.params.stacked { 0 } else { channel_idx });

            positions.clear();
            positions.extend(channel_samples.iter().enumerate().map(|(i, &s)| {
                (
                    bounds.x + i as f32 * step,
                    center - s.clamp(-1.0, 1.0) * layout.amplitude_scale,
                )
            }));
            decimate_line_in_place(positions, pixel_width * 2);

            extend_filled_line(
                vertices,
                positions,
                center,
                STROKE_WIDTH,
                color,
                rgba_with_alpha(color, color[3] * self.params.fill_alpha),
                clip,
            );
        }
    }
}

sdf_primitive!(
    OscilloscopePrimitive(OscilloscopeParams),
    Pipeline,
    u64,
    "Oscilloscope",
    TriangleList,
    |self| self.params.key
);
