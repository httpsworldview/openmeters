// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{BandCorrelation, StereometerSnapshot};
use super::render::{StereometerParams, StereometerPrimitive};
use crate::persistence::settings::StereometerSettings;
use crate::util::color::color_to_rgba;
use crate::visuals::{
    options::{CorrelationMeterMode, StereometerMode},
    palettes,
};
use iced::Color;
use std::collections::VecDeque;

const TRAIL_LEN: usize = 32;

fn tracks_band_correlation(s: &StereometerSettings) -> bool {
    s.mode == StereometerMode::DotCloudBands
        || s.correlation_meter == CorrelationMeterMode::MultiBand
}

#[derive(Debug, Clone)]
pub(in crate::visuals) struct StereometerState {
    points: Vec<(f32, f32)>,
    band_points: [Vec<(f32, f32)>; 3],
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
            points: Vec::new(),
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
            self.points.clear();
            self.band_points.iter_mut().for_each(Vec::clear);
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
            corr_trail: self.corr_trail.iter().copied().collect(),
            band_trail: [
                self.band_trail.iter().map(|values| values.low).collect(),
                self.band_trail.iter().map(|values| values.mid).collect(),
                self.band_trail.iter().map(|values| values.high).collect(),
            ],
        })
    }
}

crate::visuals::visualization_widget!(Stereometer, StereometerState, StereometerPrimitive);
