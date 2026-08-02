// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{BAND_COUNT, StereometerSnapshot};
use super::render::{
    CORR_LABEL_GAP, CORR_LABEL_H, CORR_LABEL_W, CORR_TRAIL_LEN, StereometerParams,
    StereometerPrimitive,
};
use crate::persistence::settings::StereometerSettings;
use crate::util::color::color_to_rgba;
use crate::visuals::{
    options::{CorrelationMeterMode, CorrelationMeterSide, StereometerMode},
    palettes::{self, stereometer::SIZE as PALETTE_SIZE},
    render::common::{fill_rect, text as raw_text},
};
use iced::advanced::{graphics::text::Paragraph, text};
use iced::advanced::text::Paragraph as _;
use iced::{Color, Point, Size};
use std::{collections::VecDeque, sync::Arc};

const CORR_LABEL_SIZE: f32 = 10.0;

fn tracks_band_correlation(s: &StereometerSettings) -> bool {
    s.mode == StereometerMode::DotCloudBands
        || s.correlation_meter == CorrelationMeterMode::MultiBand
}

#[derive(Debug, Clone)]
pub(crate) struct StereometerState {
    points: Arc<[(f32, f32)]>,
    band_points: [Arc<[(f32, f32)]>; BAND_COUNT],
    corr_trail: VecDeque<f32>,
    band_trail: VecDeque<[f32; BAND_COUNT]>,
    pub(in crate::visuals) palette: [Color; PALETTE_SIZE],
    pub(in crate::visuals) settings: StereometerSettings,
    labels: [Paragraph; 3],
    key: u64,
    geometry_revision: u64,
    grid_revision: u64,
}

impl StereometerState {
    pub fn new() -> Self {
        let defaults = StereometerSettings::default();
        Self {
            points: Arc::default(),
            band_points: Default::default(),
            corr_trail: VecDeque::with_capacity(CORR_TRAIL_LEN),
            band_trail: VecDeque::with_capacity(CORR_TRAIL_LEN),
            palette: palettes::stereometer::COLORS,
            settings: defaults,
            labels: ["+1", "0", "-1"].map(|label| {
                Paragraph::with_text(raw_text(label, CORR_LABEL_SIZE, Size::new(CORR_LABEL_W, CORR_LABEL_H)))
            }),
            key: crate::visuals::next_key(),
            geometry_revision: 0,
            grid_revision: 0,
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
        self.geometry_revision = self.geometry_revision.wrapping_add(1);
        self.grid_revision = self.grid_revision.wrapping_add(1);
    }

    pub fn set_palette(&mut self, palette: &[Color; PALETTE_SIZE]) {
        self.palette = *palette;
        self.geometry_revision = self.geometry_revision.wrapping_add(1);
        self.grid_revision = self.grid_revision.wrapping_add(1);
    }

    pub fn reset_audio(&mut self) {
        self.points = Arc::default();
        self.band_points = Default::default();
        self.corr_trail.clear();
        self.band_trail.clear();
        self.geometry_revision = self.geometry_revision.wrapping_add(1);
    }

    pub fn apply_snapshot(&mut self, snap: StereometerSnapshot) {
        self.geometry_revision = self.geometry_revision.wrapping_add(1);
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
            self.band_trail.truncate(CORR_TRAIL_LEN);
        } else {
            self.band_trail.clear();
        }
        self.corr_trail.truncate(CORR_TRAIL_LEN);
    }

    pub fn visual_params(&self, bounds: iced::Rectangle) -> Option<StereometerParams> {
        if self.points.is_empty() { return None; }
        let s = &self.settings;
        let (corr_trail, band_trail) = match s.correlation_meter {
            CorrelationMeterMode::Off => (Default::default(), Default::default()),
            CorrelationMeterMode::SingleBand => {
                (self.corr_trail.iter().copied().collect(), Default::default())
            }
            CorrelationMeterMode::MultiBand => (
                self.corr_trail.iter().copied().collect(),
                std::array::from_fn(|band| {
                    self.band_trail.iter().map(|values| values[band]).collect()
                }),
            ),
        };
        Some(StereometerParams {
            key: self.key,
            geometry_revision: self.geometry_revision,
            grid_revision: self.grid_revision,
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
        let x = if left {
            meter.x + meter.width + CORR_LABEL_GAP
        } else {
            meter.x - CORR_LABEL_GAP
        };
        let color = theme.extended_palette().background.base.text;
        for (label, value) in state.labels.iter().zip([1.0, 0.0, -1.0]) {
            let size = label.min_bounds();
            let x = if left { x } else { x - size.width };
            let y = StereometerPrimitive::correlation_y(meter, value) - size.height * 0.5;
            text::Renderer::fill_paragraph(renderer, label, Point::new(x, y), color, bounds);
        }
    }
});
