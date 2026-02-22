use iced::Rectangle;
use iced::advanced::graphics::Viewport;

use crate::sdf_primitive;
use crate::ui::render::common::{
    ClipTransform, SdfVertex, baseline_segment_vertices, build_aa_line_list, decimate_line,
};

#[derive(Debug, Clone)]
pub struct OscilloscopeParams {
    pub key: u64,
    pub bounds: Rectangle,
    pub channels: usize,
    pub samples_per_channel: usize,
    pub samples: Vec<f32>,
    pub colors: Vec<[f32; 4]>,
    pub fill_alpha: f32,
}

#[derive(Debug)]
pub struct OscilloscopePrimitive {
    params: OscilloscopeParams,
}

impl OscilloscopePrimitive {
    pub fn new(params: OscilloscopeParams) -> Self {
        Self { params }
    }

    fn build_vertices(&self, viewport: &Viewport) -> Vec<SdfVertex> {
        let samples_per_channel = self.params.samples_per_channel;
        let channels = self.params.channels.max(1);

        if samples_per_channel < 2 || self.params.samples.len() < channels * samples_per_channel {
            return Vec::new();
        }

        let bounds = self.params.bounds;
        let clip = ClipTransform::from_viewport(viewport);

        const VERTICAL_PADDING: f32 = 8.0;
        const CHANNEL_GAP: f32 = 12.0;
        const AMPLITUDE_SCALE: f32 = 0.9;
        const STROKE_WIDTH: f32 = 1.0;
        const LINE_ALPHA: f32 = 1.0;

        let usable_height = (bounds.height
            - VERTICAL_PADDING * 2.0
            - CHANNEL_GAP * (channels.saturating_sub(1) as f32))
            .max(1.0);
        let channel_height = usable_height / channels as f32;
        let amplitude_scale = channel_height * 0.5 * AMPLITUDE_SCALE;
        let step = bounds.width.max(1.0) / (samples_per_channel.saturating_sub(1) as f32).max(1.0);
        let pixel_width = bounds.width.ceil().max(1.0) as usize;

        let mut vertices = Vec::new();

        for (channel_idx, channel_samples) in self
            .params
            .samples
            .chunks_exact(samples_per_channel)
            .take(channels)
            .enumerate()
        {
            let color = self
                .params
                .colors
                .get(channel_idx)
                .copied()
                .unwrap_or([0.6, 0.8, 0.9, 1.0]);
            let center = bounds.y
                + VERTICAL_PADDING
                + channel_idx as f32 * (channel_height + CHANNEL_GAP)
                + channel_height * 0.5;

            let positions: Vec<_> = channel_samples
                .iter()
                .enumerate()
                .map(|(i, &s)| {
                    (
                        bounds.x + i as f32 * step,
                        center - s.clamp(-1.0, 1.0) * amplitude_scale,
                    )
                })
                .collect();
            let positions = decimate_line(&positions, pixel_width * 2);

            let fill_color = [color[0], color[1], color[2], self.params.fill_alpha];
            for pair in positions.windows(2) {
                let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
                vertices.extend(baseline_segment_vertices(
                    (x0, y0),
                    (x1, y1),
                    center,
                    clip,
                    [fill_color, fill_color],
                ));
            }

            let line_color = [color[0], color[1], color[2], LINE_ALPHA];
            vertices.extend(build_aa_line_list(
                &positions,
                STROKE_WIDTH,
                line_color,
                &clip,
            ));
        }

        vertices
    }
}

sdf_primitive!(
    OscilloscopePrimitive,
    Pipeline,
    u64,
    "Oscilloscope",
    TriangleList,
    |self| self.params.key
);
