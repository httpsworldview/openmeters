// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{BandCorrelation, StereometerSnapshot};
use super::render::{
    CORR_LABEL_GAP, CORR_LABEL_H, CORR_LABEL_W, StereometerParams, StereometerPrimitive,
};
use crate::persistence::settings::StereometerSettings;
use crate::util::color::color_to_rgba;
use crate::visuals::{
    options::{CorrelationMeterMode, CorrelationMeterSide, StereometerMode},
    palettes,
    render::common::{fill_rect, make_text},
};
use iced::advanced::text;
use iced::alignment::{Horizontal, Vertical};
use iced::{Color, Point, Size};
use std::{collections::VecDeque, sync::Arc};

const TRAIL_LEN: usize = 32;
const CORR_LABEL_SIZE: f32 = 10.0;

fn tracks_band_correlation(s: &StereometerSettings) -> bool {
    s.mode == StereometerMode::DotCloudBands
        || s.correlation_meter == CorrelationMeterMode::MultiBand
}

#[derive(Debug, Clone)]
pub(in crate::visuals) struct StereometerState {
    points: Arc<[(f32, f32)]>,
    band_points: [Arc<[(f32, f32)]>; 3],
    corr_trail: VecDeque<f32>,
    band_trail: VecDeque<BandCorrelation>,
    pub(in crate::visuals) palette: [Color; 9],
    settings: StereometerSettings,
    key: u64,
}

impl StereometerState {
    pub fn new() -> Self {
        let defaults = StereometerSettings::default();
        Self {
            points: Arc::default(),
            band_points: Default::default(),
            corr_trail: VecDeque::with_capacity(TRAIL_LEN),
            band_trail: VecDeque::with_capacity(TRAIL_LEN),
            palette: palettes::stereometer::COLORS,
            settings: defaults,
            key: crate::visuals::next_key(),
        }
    }

    pub fn update_view_settings(&mut self, s: &StereometerSettings) {
        let defaults = StereometerSettings::default();
        let dot_radius = if s.dot_radius.is_finite() {
            s.dot_radius
        } else {
            defaults.dot_radius
        };
        if tracks_band_correlation(&self.settings) != tracks_band_correlation(s) {
            self.band_trail.clear();
        }
        self.settings = StereometerSettings {
            dot_radius: dot_radius.clamp(0.5, 8.0),
            rotation: s.rotation.clamp(-4, 4),
            ..s.clone()
        };
    }

    pub fn set_palette(&mut self, palette: &[Color; 9]) {
        self.palette = *palette;
    }

    pub fn export_settings(&self) -> StereometerSettings {
        self.settings.clone()
    }

    pub fn apply_snapshot(&mut self, snap: StereometerSnapshot) {
        if snap.xy_points.is_empty() {
            self.points = Arc::default();
            self.band_points = Default::default();
            self.corr_trail.clear();
            self.band_trail.clear();
            return;
        }

        self.points = snap.xy_points;
        self.band_points = snap.band_points;

        self.corr_trail.push_front(snap.correlation);
        if tracks_band_correlation(&self.settings) {
            self.band_trail.push_front(snap.band_correlation);
            self.band_trail.truncate(TRAIL_LEN);
        } else {
            self.band_trail.clear();
        }
        self.corr_trail.truncate(TRAIL_LEN);
    }

    pub fn visual_params(&self, bounds: iced::Rectangle) -> Option<StereometerParams> {
        if self.points.is_empty() { return None; }
        let s = &self.settings;
        let (corr_trail, band_trail) = match s.correlation_meter {
            CorrelationMeterMode::Off => (Vec::new(), Default::default()),
            CorrelationMeterMode::SingleBand => {
                (self.corr_trail.iter().copied().collect(), Default::default())
            }
            CorrelationMeterMode::MultiBand => (
                self.corr_trail.iter().copied().collect(),
                [
                    self.band_trail.iter().map(|values| values.low).collect(),
                    self.band_trail.iter().map(|values| values.mid).collect(),
                    self.band_trail.iter().map(|values| values.high).collect(),
                ],
            ),
        };
        Some(StereometerParams {
            key: self.key,
            bounds,
            points: self.points.clone(),
            band_points: self.band_points.clone(),
            palette: self.palette.map(color_to_rgba),
            mode: s.mode,
            scale: s.scale,
            dot_radius: s.dot_radius,
            rotation: s.rotation,
            flip: s.flip,
            unipolar: s.unipolar && s.mode != StereometerMode::Lissajous,
            correlation_meter: s.correlation_meter,
            correlation_meter_side: s.correlation_meter_side,
            corr_trail,
            band_trail,
        })
    }
}

crate::visuals::visualization_widget!(Stereometer, StereometerState, |this, renderer, theme, bounds| {
    let state = this.state.borrow();
    let Some(params) = state.visual_params(bounds) else {
        fill_rect(renderer, bounds, theme.extended_palette().background.base.color);
        return;
    };
    let side = params.correlation_meter_side;
    let (_, meter) = StereometerPrimitive::meter_layout(&params);
    renderer.draw_primitive(bounds, StereometerPrimitive::new(params));

    if let Some(meter) = meter.filter(|meter| meter.width > 0.0 && meter.height > 0.0) {
        let left = side == CorrelationMeterSide::Left;
        let align = if left { Horizontal::Left } else { Horizontal::Right };
        let x = if left {
            meter.x + meter.width + CORR_LABEL_GAP
        } else {
            meter.x - CORR_LABEL_GAP
        };
        let color = theme.extended_palette().background.base.text;
        for (label, value) in [("+1", 1.0), ("0", 0.0), ("-1", -1.0)] {
            let mut text = make_text(
                label,
                CORR_LABEL_SIZE,
                Size::new(CORR_LABEL_W, CORR_LABEL_H),
            );
            text.align_x = align.into();
            text.align_y = Vertical::Center;
            let y = StereometerPrimitive::correlation_y(meter, value);
            text::Renderer::fill_text(renderer, text, Point::new(x, y), color, bounds);
        }
    }
});
