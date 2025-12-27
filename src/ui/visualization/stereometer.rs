//! Stereometer visualization: vectorscope + correlation meter.

use crate::audio::meter_tap::MeterFormat;
use crate::dsp::stereometer::{
    BandCorrelation, StereometerConfig, StereometerProcessor as CoreProcessor, StereometerSnapshot,
};
use crate::dsp::{AudioBlock, AudioProcessor, ProcessorUpdate, Reconfigurable};
use crate::ui::render::stereometer::{StereometerParams, StereometerPrimitive};
use crate::ui::settings::{
    CorrelationMeterMode, CorrelationMeterSide, StereometerMode, StereometerScale,
    StereometerSettings,
};
use crate::ui::theme;
use iced::advanced::renderer::{self, Quad};
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Layout, Renderer as _, Widget, layout, mouse};
use iced::{Background, Color, Element, Length, Rectangle, Size};
use iced_wgpu::primitive::Renderer as _;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

const TRAIL_LEN: usize = 32;

fn next_key() -> u64 {
    static KEY: AtomicU64 = AtomicU64::new(1);
    KEY.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone)]
pub struct StereometerProcessor(CoreProcessor);

impl StereometerProcessor {
    pub fn new(sample_rate: f32) -> Self {
        Self(CoreProcessor::new(StereometerConfig {
            sample_rate,
            ..Default::default()
        }))
    }

    pub fn ingest(&mut self, samples: &[f32], format: MeterFormat) -> StereometerSnapshot {
        if samples.is_empty() {
            return self.0.snapshot().clone();
        }
        let sr = format.sample_rate.max(1.0);
        if (self.0.config().sample_rate - sr).abs() > f32::EPSILON {
            self.0.update_config(StereometerConfig {
                sample_rate: sr,
                ..self.0.config()
            });
        }
        let block = AudioBlock::new(samples, format.channels.max(1), sr, Instant::now());
        match self.0.process_block(&block) {
            ProcessorUpdate::Snapshot(s) => s,
            ProcessorUpdate::None => self.0.snapshot().clone(),
        }
    }

    pub fn config(&self) -> StereometerConfig {
        self.0.config()
    }

    pub fn update_config(&mut self, c: StereometerConfig) {
        self.0.update_config(c);
    }
}

#[derive(Debug, Clone)]
pub struct StereometerState {
    points: Vec<(f32, f32)>,
    corr_trail: VecDeque<f32>,
    band_trail: VecDeque<BandCorrelation>,
    palette: [Color; 8],
    persistence: f32,
    mode: StereometerMode,
    scale: StereometerScale,
    scale_range: f32,
    rotation: i8,
    flip: bool,
    correlation_meter: CorrelationMeterMode,
    correlation_meter_side: CorrelationMeterSide,
    key: u64,
}

impl StereometerState {
    pub fn new() -> Self {
        Self {
            points: Vec::new(),
            corr_trail: VecDeque::with_capacity(TRAIL_LEN),
            band_trail: VecDeque::with_capacity(TRAIL_LEN),
            palette: theme::DEFAULT_STEREOMETER_PALETTE,
            persistence: 0.0,
            mode: StereometerMode::default(),
            scale: StereometerScale::default(),
            scale_range: 15.0,
            rotation: -1,
            flip: true,
            correlation_meter: CorrelationMeterMode::default(),
            correlation_meter_side: CorrelationMeterSide::default(),
            key: next_key(),
        }
    }

    pub fn update_view_settings(&mut self, s: &StereometerSettings) {
        self.persistence = s.persistence.clamp(0.0, 0.9);
        self.mode = s.mode;
        self.scale = s.scale;
        self.scale_range = s.scale_range;
        self.rotation = s.rotation.clamp(-4, 4);
        self.flip = s.flip;
        self.correlation_meter = s.correlation_meter;
        self.correlation_meter_side = s.correlation_meter_side;
    }

    pub fn set_palette(&mut self, p: &[Color]) {
        for (dst, src) in self.palette.iter_mut().zip(p) {
            *dst = *src;
        }
    }

    pub fn palette(&self) -> [Color; 8] {
        self.palette
    }

    pub fn export_settings(&self) -> StereometerSettings {
        StereometerSettings {
            persistence: self.persistence,
            mode: self.mode,
            scale: self.scale,
            scale_range: self.scale_range,
            rotation: self.rotation,
            flip: self.flip,
            correlation_meter: self.correlation_meter,
            correlation_meter_side: self.correlation_meter_side,
            ..Default::default()
        }
    }

    pub fn apply_snapshot(&mut self, snap: &StereometerSnapshot) {
        if snap.xy_points.is_empty() {
            self.points.clear();
            return;
        }

        let scale = |x: f32, y: f32| match self.scale {
            StereometerScale::Linear => (x, y),
            StereometerScale::Exponential => {
                let len = x.hypot(y);
                if len < f32::EPSILON {
                    return (0.0, 0.0);
                }
                let k = (len.max((-self.scale_range).exp2()).log2() + self.scale_range)
                    / (-self.scale_range * len);
                (k * x, k * y)
            }
        };

        self.points.resize(snap.xy_points.len(), (0.0, 0.0));
        let fresh = 1.0 - self.persistence;
        for (dst, src) in self.points.iter_mut().zip(&snap.xy_points) {
            let s = scale(src.0, src.1);
            *dst = if self.persistence <= f32::EPSILON {
                s
            } else {
                (
                    dst.0 * self.persistence + s.0 * fresh,
                    dst.1 * self.persistence + s.1 * fresh,
                )
            };
        }

        let sm = |old: f32, new: f32| old * 0.85 + new * 0.15;
        let c = self
            .corr_trail
            .front()
            .map(|&o| sm(o, snap.correlation))
            .unwrap_or(snap.correlation);
        let b = self
            .band_trail
            .front()
            .map(|o| BandCorrelation {
                low: sm(o.low, snap.band_correlation.low),
                mid: sm(o.mid, snap.band_correlation.mid),
                high: sm(o.high, snap.band_correlation.high),
            })
            .unwrap_or(snap.band_correlation);

        self.corr_trail.push_front(c);
        self.band_trail.push_front(b);
        self.corr_trail.truncate(TRAIL_LEN);
        self.band_trail.truncate(TRAIL_LEN);
    }

    fn params(&self, bounds: Rectangle) -> Option<StereometerParams> {
        (self.points.len() >= 2).then(|| StereometerParams {
            key: self.key,
            bounds,
            points: self.points.clone(),
            palette: self.palette.map(theme::color_to_rgba),
            mode: self.mode,
            rotation: self.rotation,
            flip: self.flip,
            correlation_meter: self.correlation_meter,
            correlation_meter_side: self.correlation_meter_side,
            corr_trail: self.corr_trail.iter().copied().collect(),
            band_trail: self.band_trail.iter().copied().collect(),
        })
    }
}

#[derive(Debug)]
pub struct Stereometer<'a>(&'a RefCell<StereometerState>);

impl<'a> Stereometer<'a> {
    pub fn new(state: &'a RefCell<StereometerState>) -> Self {
        Self(state)
    }
}

impl<M> Widget<M, iced::Theme, iced::Renderer> for Stereometer<'_> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::stateless()
    }
    fn state(&self) -> tree::State {
        tree::State::new(())
    }
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }
    fn children(&self) -> Vec<Tree> {
        Vec::new()
    }
    fn diff(&self, _: &mut Tree) {}

    fn layout(&mut self, _: &mut Tree, _: &iced::Renderer, lim: &layout::Limits) -> layout::Node {
        layout::Node::new(lim.resolve(Length::Fill, Length::Fill, Size::ZERO))
    }

    fn draw(
        &self,
        _: &Tree,
        r: &mut iced::Renderer,
        th: &iced::Theme,
        _: &renderer::Style,
        lay: Layout<'_>,
        _: mouse::Cursor,
        _: &Rectangle,
    ) {
        let b = lay.bounds();
        match self.0.borrow().params(b) {
            Some(p) => r.draw_primitive(b, StereometerPrimitive::from(p)),
            None => r.fill_quad(
                Quad {
                    bounds: b,
                    border: Default::default(),
                    shadow: Default::default(),
                    snap: true,
                },
                Background::Color(th.extended_palette().background.base.color),
            ),
        }
    }
}

pub fn widget<'a, M: 'a>(state: &'a RefCell<StereometerState>) -> Element<'a, M> {
    Element::new(Stereometer::new(state))
}
