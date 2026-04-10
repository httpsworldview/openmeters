// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{
    BandCorrelation, StereometerConfig, StereometerProcessor as CoreProcessor, StereometerSnapshot,
};
use super::render::{StereometerParams, StereometerPrimitive, scale_point};
use crate::persistence::settings::{
    CorrelationMeterMode, CorrelationMeterSide, StereometerMode, StereometerScale,
    StereometerSettings,
};
use crate::util::color::color_to_rgba;
use crate::visuals::palettes;
use crate::{vis_processor, visualization_widget};
use iced::Color;
use std::collections::VecDeque;

const TRAIL_LEN: usize = 32;
const CORRELATION_SMOOTHING: f32 = 0.85;
const MAX_PERSISTENCE: f32 = 0.9;

vis_processor!(
    StereometerProcessor,
    CoreProcessor,
    StereometerConfig,
    StereometerSnapshot
);

#[derive(Debug, Clone)]
pub(crate) struct StereometerState {
    points: Vec<(f32, f32)>,
    corr_trail: VecDeque<f32>,
    band_trail: VecDeque<BandCorrelation>,
    pub(crate) palette: [Color; 9],
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
            palette: palettes::stereometer::COLORS,
            persistence: defaults.persistence,
            mode: defaults.mode,
            scale: defaults.scale,
            scale_range: defaults.scale_range,
            rotation: defaults.rotation,
            flip: defaults.flip,
            correlation_meter: defaults.correlation_meter,
            correlation_meter_side: defaults.correlation_meter_side,
            key: crate::visuals::next_key(),
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

    pub fn set_palette(&mut self, palette: &[Color; 9]) {
        self.palette = *palette;
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
            self.corr_trail.clear();
            self.band_trail.clear();
            return;
        }

        let scale = |x: f32, y: f32| scale_point(self.scale, x, y, self.scale_range);

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
            .map_or(snap.correlation, |&o| sm(o, snap.correlation));
        let b = self
            .band_trail
            .front()
            .map_or(snap.band_correlation, |o| BandCorrelation {
                low: sm(o.low, snap.band_correlation.low),
                mid: sm(o.mid, snap.band_correlation.mid),
                high: sm(o.high, snap.band_correlation.high),
            });

        self.corr_trail.push_front(c);
        self.band_trail.push_front(b);
        self.corr_trail.truncate(TRAIL_LEN);
        self.band_trail.truncate(TRAIL_LEN);
    }

    pub fn visual_params(&self, bounds: iced::Rectangle) -> Option<StereometerParams> {
        Some(StereometerParams {
            key: self.key,
            bounds,
            points: self.points.clone(),
            palette: self.palette.map(color_to_rgba),
            mode: self.mode,
            scale: self.scale,
            scale_range: self.scale_range,
            rotation: self.rotation,
            flip: self.flip,
            correlation_meter: self.correlation_meter,
            correlation_meter_side: self.correlation_meter_side,
            corr_trail: self.corr_trail.iter().copied().collect(),
            band_trail: self.band_trail.iter().copied().collect(),
        })
    }
}

visualization_widget!(Stereometer, StereometerState, StereometerPrimitive);
