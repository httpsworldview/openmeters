// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{BandCorrelation, StereometerSnapshot};
use super::render::{StereometerParams, StereometerPrimitive, scale_point};
use crate::persistence::settings::StereometerSettings;
use crate::util::color::color_to_rgba;
use crate::visuals::palettes;
use iced::Color;
use std::collections::VecDeque;

const TRAIL_LEN: usize = 32;
const CORRELATION_SMOOTHING: f32 = 0.85;
const MAX_PERSISTENCE: f32 = 0.9;

#[derive(Debug, Clone)]
pub(crate) struct StereometerState {
    points: Vec<(f32, f32)>,
    band_points: [Vec<(f32, f32)>; 3],
    corr_trail: VecDeque<f32>,
    band_trail: VecDeque<BandCorrelation>,
    pub(crate) palette: [Color; 9],
    settings: StereometerSettings,
    key: u64,
}

fn blend_points<F: Fn(f32, f32) -> (f32, f32)>(
    dst: &mut Vec<(f32, f32)>,
    src: &[(f32, f32)],
    scale: F,
    persistence: f32,
) {
    dst.resize(src.len(), (0.0, 0.0));
    let fresh = 1.0 - persistence;
    for (d, &(x, y)) in dst.iter_mut().zip(src) {
        let sp = scale(x, y);
        *d = (
            d.0 * persistence + sp.0 * fresh,
            d.1 * persistence + sp.1 * fresh,
        );
    }
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
        self.settings = StereometerSettings {
            persistence: s.persistence.clamp(0.0, MAX_PERSISTENCE),
            dot_radius: s.dot_radius.clamp(0.5, 8.0),
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
            self.points = Vec::new();
            self.band_points = Default::default();
            self.corr_trail.clear();
            self.band_trail.clear();
            return;
        }

        let s = &self.settings;
        let scale_fn = |x: f32, y: f32| scale_point(s.scale, x, y, s.scale_range);
        blend_points(&mut self.points, &snap.xy_points, scale_fn, s.persistence);
        for (dst, src) in self.band_points.iter_mut().zip(&snap.band_points) {
            blend_points(dst, src, scale_fn, s.persistence);
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
        if self.points.is_empty() {
            return None;
        }
        let s = &self.settings;
        Some(StereometerParams {
            key: self.key,
            bounds,
            points: self.points.clone(),
            band_points: self.band_points.clone(),
            palette: self.palette.map(color_to_rgba),
            mode: s.mode,
            scale: s.scale,
            scale_range: s.scale_range,
            dot_radius: s.dot_radius,
            rotation: s.rotation,
            flip: s.flip,
            correlation_meter: s.correlation_meter,
            correlation_meter_side: s.correlation_meter_side,
            corr_trail: self.corr_trail.iter().copied().collect(),
            band_trail: self.band_trail.iter().copied().collect(),
        })
    }
}

crate::visuals::visualization_widget!(Stereometer, StereometerState, StereometerPrimitive);
