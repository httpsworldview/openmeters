// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{BAND_COUNT, FULL_BAND, StereometerSnapshot};
use super::render::{
    CORR_LABEL_GAP, CORR_LABEL_H, CORR_LABEL_W, CORR_TRAIL_STEP, FixedTrail, StereometerParams,
};
use crate::persistence::settings::StereometerSettings;
use crate::util::color::color_to_rgba;
use crate::visuals::{
    options::{CorrelationMeterMode, CorrelationMeterSide, StereometerMode},
    palettes::{self, stereometer::SIZE as PALETTE_SIZE},
    render::common::text as raw_text,
};
use iced::advanced::text::Paragraph as _;
use iced::advanced::{graphics::text::Paragraph, text};
use iced::{Color, Point, Size};
use std::sync::Arc;

const CORR_LABEL_SIZE: f32 = 10.0;
// Limits applied when drawing; the settings sliders span the same ranges.
pub const MAX_ROTATION: i8 = 4;
pub const DOT_RADIUS_RANGE: (f32, f32) = (0.5, 8.0);

crate::macros::default_struct! {
    pub(crate) struct StereometerState {
        points: [Arc<[(f32, f32)]>; BAND_COUNT + 1] = Default::default(),
        trails: [FixedTrail; BAND_COUNT + 1] = Default::default(),
        trail_clock: f32 = 0.0,
        pub(in crate::visuals) palette: [Color; PALETTE_SIZE] = palettes::stereometer::COLORS,
        pub(in crate::visuals) settings: StereometerSettings = StereometerSettings::default(),
        labels: [Paragraph; 3] = ["+1", "0", "-1"].map(|label| {
            Paragraph::with_text(raw_text(label, CORR_LABEL_SIZE, Size::new(CORR_LABEL_W, CORR_LABEL_H)))
        }),
        geometry: crate::visuals::GeometryKey = crate::visuals::GeometryKey::new(),
        grid: crate::visuals::GeometryKey = crate::visuals::GeometryKey::new(),
    }
}

impl StereometerState {
    pub fn update_view_settings(&mut self, s: &StereometerSettings) {
        if self.settings.bands().analyzed() != s.bands().analyzed() {
            self.trails[1..].fill(FixedTrail::default());
        }
        self.settings = StereometerSettings {
            rotation: s.rotation.clamp(-MAX_ROTATION, MAX_ROTATION),
            ..s.clone()
        };
        self.geometry.invalidate();
        self.grid.invalidate();
    }

    crate::visuals::palette_setter!(stereometer => geometry, grid);

    pub fn reset_audio(&mut self) {
        self.points = Default::default();
        (self.trails, self.trail_clock) = Default::default();
        self.geometry.invalidate();
    }

    pub fn apply_snapshot(&mut self, snap: StereometerSnapshot) {
        self.geometry.invalidate();
        self.points = snap.points;
        let clock = self.trail_clock + snap.elapsed / CORR_TRAIL_STEP;
        self.trail_clock = clock.fract();
        let bands = 1 + BAND_COUNT * usize::from(self.settings.bands().analyzed());
        for (trail, &value) in self.trails[..bands].iter_mut().zip(&snap.correlations) {
            trail.advance(value, clock as usize);
        }
    }

    pub(in crate::visuals) fn is_quiescent(&self) -> bool {
        let bands =
            1 + BAND_COUNT * usize::from(self.settings.mode == StereometerMode::DotCloudBands);
        let trails = match self.settings.correlation_meter {
            CorrelationMeterMode::Off => 0,
            CorrelationMeterMode::SingleBand => 1,
            CorrelationMeterMode::MultiBand => BAND_COUNT + 1,
        };
        self.points[..bands].iter().all(|points| {
            !points.is_empty()
                && points
                    .iter()
                    .all(|&(left, right)| left == 0.0 && right == 0.0)
        }) && self.trails[..trails]
            .iter()
            .all(|trail| !trail.is_empty() && trail.iter().all(|value| *value == 0.0))
    }

    pub fn visual_params(&self, bounds: iced::Rectangle) -> Option<StereometerParams> {
        let s = &self.settings;
        if self.points[FULL_BAND].is_empty() {
            return None;
        }
        Some(StereometerParams {
            geometry: self.geometry,
            grid: self.grid,
            bounds,
            points: self.points.clone(),
            palette: self.palette.map(color_to_rgba),
            mode: s.mode,
            scale: s.scale,
            dot_radius: (s.dot_radius.get()).clamp(DOT_RADIUS_RANGE.0, DOT_RADIUS_RANGE.1),
            rotation: s.rotation,
            flip: s.flip,
            unipolar: s.unipolar && s.mode != StereometerMode::Lissajous,
            correlation_meter: s.correlation_meter,
            correlation_meter_side: s.correlation_meter_side,
            trails: self.trails,
        })
    }
}

crate::visuals::visualization_widget!(
    Stereometer,
    StereometerState,
    |this, renderer, theme, bounds| {
        let state = this.state.borrow();
        let Some(params) = state.visual_params(bounds) else {
            return;
        };
        let side = params.correlation_meter_side;
        let (_, meter) = params.meter_layout();
        renderer.draw_primitive(bounds, params);

        if let Some(meter) = meter.filter(|meter| meter.width > 0.0 && meter.height > 0.0) {
            let left = side == CorrelationMeterSide::Left;
            let x = if left {
                meter.x + meter.width + CORR_LABEL_GAP
            } else {
                meter.x - CORR_LABEL_GAP
            };
            let color = theme.extended_palette().background.base.text;
            for (label, value) in state.labels.iter().zip([1.0, 0.0, -1.0]) {
                let size = label.min_bounds();
                let x = if left { x } else { x - size.width };
                let y = StereometerParams::correlation_y(meter, value) - size.height * 0.5;
                text::Renderer::fill_paragraph(renderer, label, Point::new(x, y), color, bounds);
            }
        }
    }
);
