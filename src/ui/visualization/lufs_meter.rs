use crate::dsp::loudness::{
    LoudnessConfig, LoudnessProcessor as CoreLoudnessProcessor, LoudnessSnapshot,
};
use crate::dsp::{AudioBlock, AudioProcessor, ProcessorUpdate};
use crate::ui::render::lufs_meter::{ChannelVisual, LufsMeterPrimitive, VisualParams};
use iced::advanced::renderer;
use iced::advanced::widget::Tree;
use iced::advanced::{Layout, Widget, layout, mouse};
use iced::{Element, Length, Rectangle, Size, Theme};
use iced_wgpu::primitive::Renderer as _;
use std::time::Instant;

const CHANNELS: usize = 2;
const DEFAULT_MIN_LUFS: f32 = -60.0;
const DEFAULT_MAX_LUFS: f32 = 0.0;
const DEFAULT_HEIGHT: f32 = 180.0;
const DEFAULT_WIDTH: f32 = 140.0;
const DEFAULT_MOMENTARY_WINDOW_SECS: f32 = 0.4;
const CHANNEL_GAP_FRACTION: f32 = 0.08;
const CHANNEL_LABELS: [&str; CHANNELS] = ["L", "R"];

/// UI wrapper around the shared loudness processor.
#[derive(Debug, Clone)]
pub struct LufsProcessor {
    inner: CoreLoudnessProcessor,
    channels: usize,
}

impl LufsProcessor {
    pub fn new(sample_rate: f32) -> Self {
        let config = LoudnessConfig {
            sample_rate,
            momentary_window: DEFAULT_MOMENTARY_WINDOW_SECS,
            floor_lufs: DEFAULT_MIN_LUFS,
        };
        Self {
            inner: CoreLoudnessProcessor::new(config),
            channels: CHANNELS,
        }
    }

    pub fn ingest(&mut self, samples: &[f32]) -> LoudnessSnapshot {
        if samples.is_empty() {
            return self.inner.snapshot().clone();
        }

        let block = AudioBlock::new(
            samples,
            self.channels,
            self.inner.config().sample_rate,
            Instant::now(),
        );

        match self.inner.process_block(&block) {
            ProcessorUpdate::Snapshot(snapshot) => snapshot,
            ProcessorUpdate::None => self.inner.snapshot().clone(),
        }
    }
}

/// View-model state consumed by the LUFS meter widget.
#[derive(Debug, Clone)]
pub struct LufsMeterState {
    momentary_lufs: [f32; CHANNELS],
    true_peak_db: [f32; CHANNELS],
    range: (f32, f32),
    style: VisualStyle,
}

impl LufsMeterState {
    pub fn new() -> Self {
        Self {
            momentary_lufs: [DEFAULT_MIN_LUFS; CHANNELS],
            true_peak_db: [DEFAULT_MIN_LUFS; CHANNELS],
            range: (DEFAULT_MIN_LUFS, DEFAULT_MAX_LUFS),
            style: VisualStyle::default(),
        }
    }

    pub fn apply_snapshot(&mut self, snapshot: &LoudnessSnapshot) {
        let floor = self.range.0;
        Self::copy_into(&mut self.momentary_lufs, &snapshot.momentary_lufs, floor);
        Self::copy_into(&mut self.true_peak_db, &snapshot.true_peak_db, floor);
    }

    fn copy_into(target: &mut [f32; CHANNELS], source: &[f32], floor: f32) {
        let copied = target.len().min(source.len());
        target[..copied].copy_from_slice(&source[..copied]);
        for value in &mut target[copied..] {
            *value = floor;
        }
    }

    pub fn set_range(&mut self, min: f32, max: f32) {
        self.range = (min, max);
    }

    pub fn set_style(&mut self, style: VisualStyle) {
        self.style = style;
    }

    pub fn range(&self) -> (f32, f32) {
        self.range
    }

    pub fn style(&self) -> &VisualStyle {
        &self.style
    }

    pub fn channels(&self) -> impl Iterator<Item = (&'static str, f32, f32)> + '_ {
        let momentary = self.momentary_lufs;
        let peaks = self.true_peak_db;
        CHANNEL_LABELS
            .into_iter()
            .zip(momentary)
            .zip(peaks)
            .map(|((label, momentary), peak)| (label, momentary, peak))
    }

    pub fn momentary_average(&self) -> f32 {
        self.momentary_lufs.iter().copied().sum::<f32>() / CHANNELS as f32
    }

    pub fn peak_max(&self) -> f32 {
        self.true_peak_db
            .into_iter()
            .reduce(f32::max)
            .unwrap_or(self.range.0)
    }

    pub fn visual_params(&self, range: (f32, f32)) -> VisualParams {
        let (min, max) = range;
        let style = *self.style();
        let channels = self
            .channels()
            .map(|(_, momentary, peak)| ChannelVisual {
                momentary_lufs: momentary,
                peak_lufs: peak,
                background_color: style.background,
                fill_color: style.bar_fill(momentary, max),
                peak_color: style.peak,
            })
            .collect();

        VisualParams {
            min_lufs: min,
            max_lufs: max,
            channels,
            channel_gap_fraction: CHANNEL_GAP_FRACTION,
        }
    }
}

/// Palette for the LUFS meter.
#[derive(Debug, Clone, Copy)]
pub struct VisualStyle {
    pub background: [f32; 4],
    pub fill_safe: [f32; 4],
    pub fill_warn: [f32; 4],
    pub fill_danger: [f32; 4],
    pub peak: [f32; 4],
}

impl VisualStyle {
    fn bar_fill(&self, value: f32, max: f32) -> [f32; 4] {
        if value >= max - 3.0 {
            self.fill_danger
        } else if value >= max - 10.0 {
            self.fill_warn
        } else {
            self.fill_safe
        }
    }
}

impl Default for VisualStyle {
    fn default() -> Self {
        Self {
            background: [0.08, 0.08, 0.10, 1.0],
            fill_safe: [0.20, 0.70, 0.35, 1.0],
            fill_warn: [0.95, 0.70, 0.20, 1.0],
            fill_danger: [0.95, 0.20, 0.20, 1.0],
            peak: [1.0, 1.0, 1.0, 0.9],
        }
    }
}

/// Declare a LUFS meter widget that renders using the iced_wgpu backend.
#[derive(Debug)]
pub struct LufsMeter<'a> {
    state: &'a LufsMeterState,
    explicit_range: Option<(f32, f32)>,
    height: f32,
    width: f32,
}

impl<'a> LufsMeter<'a> {
    pub fn new(state: &'a LufsMeterState) -> Self {
        Self {
            state,
            explicit_range: None,
            height: DEFAULT_HEIGHT,
            width: DEFAULT_WIDTH,
        }
    }

    pub fn with_range(mut self, min: f32, max: f32) -> Self {
        self.explicit_range = Some((min, max));
        self
    }

    pub fn with_height(mut self, height: f32) -> Self {
        self.height = height.max(32.0);
        self
    }

    pub fn with_width(mut self, width: f32) -> Self {
        self.width = width.max(96.0);
        self
    }

    fn active_range(&self) -> (f32, f32) {
        self.explicit_range.unwrap_or_else(|| self.state.range())
    }
}

impl<'a, Message> Widget<Message, Theme, iced::Renderer> for LufsMeter<'a> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(self.width), Length::Fixed(self.height))
    }

    fn layout(
        &self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let size = limits.resolve(
            Length::Fixed(self.width),
            Length::Fixed(self.height),
            Size::new(0.0, 0.0),
        );

        layout::Node::new(size)
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let (min, max) = self.active_range();
        let params = self.state.visual_params((min, max));

        renderer.draw_primitive(bounds, LufsMeterPrimitive::new(params));
    }

    fn children(&self) -> Vec<Tree> {
        Vec::new()
    }

    fn diff(&self, _tree: &mut Tree) {}
}

/// Convenience conversion into an [`iced::Element`].
pub fn widget<'a, Message>(state: &'a LufsMeterState) -> Element<'a, Message>
where
    Message: 'a,
{
    let (min, max) = state.range();
    Element::new(
        LufsMeter::new(state)
            .with_range(min, max)
            .with_height(DEFAULT_HEIGHT)
            .with_width(DEFAULT_WIDTH),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_aggregates_channels() {
        let mut state = LufsMeterState::new();
        state.apply_snapshot(&LoudnessSnapshot {
            momentary_lufs: vec![-10.0, -5.0],
            true_peak_db: vec![-1.0, -3.0],
        });

        assert!((state.momentary_average() + 7.5).abs() < f32::EPSILON);
        assert_eq!(state.peak_max(), -1.0);

        let labels: Vec<_> = state.channels().map(|(label, _, _)| label).collect();
        assert_eq!(labels, vec!["L", "R"]);
    }
}
