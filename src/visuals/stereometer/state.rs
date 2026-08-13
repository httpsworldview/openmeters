// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{BAND_COUNT, FULL_BAND, StereometerSnapshot};
use super::render::{
    CORR_LABEL_GAP, CORR_LABEL_H, CORR_LABEL_W, FixedTrail, StereometerParams,
};
use crate::persistence::settings::StereometerSettings;
use crate::util::{color::color_to_rgba, finite_or};
use crate::visuals::{
    options::{CorrelationMeterMode, CorrelationMeterSide, StereometerMode},
    palettes::{self, stereometer::SIZE as PALETTE_SIZE},
    render::common::{fill_rect, text as raw_text},
};
use iced::advanced::{graphics::text::Paragraph, text};
use iced::advanced::text::Paragraph as _;
use iced::{Color, Point, Size};
use std::sync::Arc;

const CORR_LABEL_SIZE: f32 = 10.0;

fn tracks_band_correlation(s: &StereometerSettings) -> bool {
    s.mode == StereometerMode::DotCloudBands
        || s.correlation_meter == CorrelationMeterMode::MultiBand
}

pub(crate) struct StereometerState {
    points: [Arc<[(f32, f32)]>; BAND_COUNT + 1],
    band_colors: [Arc<[[f32; 4]]>; BAND_COUNT],
    trails: [FixedTrail; BAND_COUNT + 1],
    pub(in crate::visuals) palette: [Color; PALETTE_SIZE],
    pub(in crate::visuals) settings: StereometerSettings,
    labels: [Paragraph; 3],
    geometry: crate::visuals::GeometryKey,
    grid: crate::visuals::GeometryKey,
}

impl StereometerState {
    pub fn new() -> Self {
        Self {
            points: Default::default(),
            band_colors: Default::default(),
            trails: Default::default(),
            palette: palettes::stereometer::COLORS,
            settings: StereometerSettings::default(),
            labels: ["+1", "0", "-1"].map(|label| {
                Paragraph::with_text(raw_text(label, CORR_LABEL_SIZE, Size::new(CORR_LABEL_W, CORR_LABEL_H)))
            }),
            geometry: crate::visuals::GeometryKey::new(),
            grid: crate::visuals::GeometryKey::new(),
        }
    }

    pub fn update_view_settings(&mut self, s: &StereometerSettings) {
        let dot_radius = finite_or(s.dot_radius, StereometerSettings::default().dot_radius);
        if tracks_band_correlation(&self.settings) != tracks_band_correlation(s) {
            self.trails[1..].fill(FixedTrail::default());
        }
        self.settings = StereometerSettings {
            dot_radius: dot_radius.clamp(0.5, 8.0),
            rotation: s.rotation.clamp(-4, 4),
            ..s.clone()
        };
        self.geometry.invalidate();
        self.grid.invalidate();
    }

    pub fn set_palette(&mut self, palette: &[Color; PALETTE_SIZE]) {
        self.palette = *palette; self.band_colors = Default::default();
        self.sync_band_colors();
        self.geometry.invalidate(); self.grid.invalidate();
    }

    fn sync_band_colors(&mut self) {
        for (band, colors) in self.band_colors.iter_mut().enumerate() {
            let len = self.points[band + 1].len();
            if colors.len() == len { continue; }
            let [r, g, b, a] = color_to_rgba(self.palette[5 + band]);
            *colors = (0..len)
                .map(|i| {
                    let factor = a * (i + 1) as f32 / len as f32;
                    [r * factor, g * factor, b * factor, 0.0]
                })
                .collect();
        }
    }

    pub fn reset_audio(&mut self) {
        self.points = Default::default();
        self.trails = Default::default();
        self.geometry.invalidate();
    }

    pub fn apply_snapshot(&mut self, snap: StereometerSnapshot) {
        self.geometry.invalidate();
        self.points = snap.points;
        self.sync_band_colors();
        self.trails[FULL_BAND].push_front(snap.correlations[FULL_BAND]);
        if tracks_band_correlation(&self.settings) {
            for (trail, value) in self.trails[1..].iter_mut().zip(&snap.correlations[1..]) {
                trail.push_front(*value);
            }
        } else {
            self.trails[1..].fill(FixedTrail::default());
        }
    }

    pub(in crate::visuals) fn is_quiescent(&self) -> bool {
        let bands = 1 + BAND_COUNT * usize::from(self.settings.mode == StereometerMode::DotCloudBands);
        let trails = match self.settings.correlation_meter {
            CorrelationMeterMode::Off => 0,
            CorrelationMeterMode::SingleBand => 1,
            CorrelationMeterMode::MultiBand => BAND_COUNT + 1,
        };
        self.points[..bands].iter().all(|points| {
            !points.is_empty() && points.iter().all(|&(left, right)| left == 0.0 && right == 0.0)
        }) && self.trails[..trails]
            .iter()
            .all(|trail| !trail.is_empty() && trail.iter().all(|value| *value == 0.0))
    }

    pub fn visual_params(&self, bounds: iced::Rectangle) -> Option<StereometerParams> {
        let s = &self.settings;
        if self.points[FULL_BAND].is_empty() { return None; }
        Some(StereometerParams {
            geometry: self.geometry,
            grid: self.grid,
            bounds,
            points: self.points.clone(),
            band_colors: self.band_colors.clone(),
            palette: self.palette.map(color_to_rgba),
            mode: s.mode,
            scale: s.scale,
            dot_radius: s.dot_radius,
            rotation: s.rotation,
            flip: s.flip,
            unipolar: s.unipolar && s.mode != StereometerMode::Lissajous,
            correlation_meter: s.correlation_meter,
            correlation_meter_side: s.correlation_meter_side,
            trails: self.trails,
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
    let (_, meter) = StereometerParams::meter_layout(&params);
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
});
