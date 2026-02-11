// Stereometer visualization: vectorscope + correlation meter.

use crate::audio::meter_tap::MeterFormat;
use crate::dsp::stereometer::{
    BandCorrelation, StereometerConfig, StereometerProcessor as CoreProcessor, StereometerSnapshot,
};
use crate::dsp::{AudioBlock, AudioProcessor, Reconfigurable};
use crate::ui::render::stereometer::{StereometerParams, StereometerPrimitive};
use crate::ui::settings::{
    CorrelationMeterMode, CorrelationMeterSide, StereometerMode, StereometerScale,
    StereometerSettings,
};
use crate::ui::theme;
use crate::visualization_widget;
use iced::Color;
use std::collections::VecDeque;

const TRAIL_LEN: usize = 32;
const CORRELATION_SMOOTHING: f32 = 0.85;
const MAX_PERSISTENCE: f32 = 0.9;

#[derive(Debug, Clone)]
pub(crate) struct StereometerProcessor {
    inner: CoreProcessor,
}

impl StereometerProcessor {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            inner: CoreProcessor::new(StereometerConfig {
                sample_rate,
                ..Default::default()
            }),
        }
    }

    pub fn ingest(&mut self, samples: &[f32], format: MeterFormat) -> Option<StereometerSnapshot> {
        if samples.is_empty() {
            return None;
        }
        let sample_rate = format.sample_rate.max(1.0);
        self.inner.process_block(&AudioBlock::now(
            samples,
            format.channels.max(1),
            sample_rate,
        ))
    }

    pub fn config(&self) -> StereometerConfig {
        self.inner.config()
    }

    pub fn update_config(&mut self, c: StereometerConfig) {
        self.inner.update_config(c);
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StereometerState {
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
        let defaults = StereometerSettings::default();
        Self {
            points: Vec::new(),
            corr_trail: VecDeque::with_capacity(TRAIL_LEN),
            band_trail: VecDeque::with_capacity(TRAIL_LEN),
            palette: theme::stereometer::COLORS,
            persistence: defaults.persistence,
            mode: defaults.mode,
            scale: defaults.scale,
            scale_range: defaults.scale_range,
            rotation: defaults.rotation,
            flip: defaults.flip,
            correlation_meter: defaults.correlation_meter,
            correlation_meter_side: defaults.correlation_meter_side,
            key: super::next_key(),
        }
    }

    pub fn update_view_settings(&mut self, s: &StereometerSettings) {
        self.persistence = s.persistence.clamp(0.0, MAX_PERSISTENCE);
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

        let sm =
            |old: f32, new: f32| old * CORRELATION_SMOOTHING + new * (1.0 - CORRELATION_SMOOTHING);
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

    pub fn visual_params(&self, bounds: iced::Rectangle) -> Option<StereometerParams> {
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

visualization_widget!(
    Stereometer,
    StereometerState,
    StereometerPrimitive,
    |state, bounds| state.visual_params(bounds),
    |params| StereometerPrimitive::new(params)
);
