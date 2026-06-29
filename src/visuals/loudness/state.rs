// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{LoudnessSnapshot, MAX_CHANNELS};
use super::render::{LoudnessParams, LoudnessPrimitive, MeterFill};
use crate::persistence::settings::LoudnessSettings;
use crate::visuals::options::MeterMode;
use crate::visuals::palettes;
use crate::util::color::{color_to_rgba, with_alpha};
use crate::visuals::render::common::{fill_rect, make_text};
use iced::advanced::text;
use iced::alignment::{Horizontal, Vertical};
use iced::{Color, Point, Rectangle, Size};
use std::time::{Duration, Instant};

const DEFAULT_RANGE: (f32, f32) = (-60.0, 4.0);
const GUIDE_LEVELS: [f32; 6] = [0.0, -6.0, -12.0, -18.0, -24.0, -36.0];
const PEAK_HOLD: Duration = Duration::from_secs(2);
const PEAK_DECAY_DB_PER_SEC: f32 = 60.0;
const LEFT_PADDING: f32 = 28.0;
const RIGHT_PADDING: f32 = 64.0;
const LABEL_FONT_SIZE: f32 = 10.0;
const GUIDE_LABEL_HEIGHT: f32 = 12.0;
const GUIDE_LABEL_GAP: f32 = 2.0;
const GUIDE_LABEL_ORDER: [usize; GUIDE_LEVELS.len()] = [0, 2, 5, 3, 4, 1];
const VALUE_FONT_SIZE: f32 = 12.0;

pub const LOUDNESS_PALETTE_SIZE: usize = palettes::loudness::COLORS.len();

const PAL_BACKGROUND: usize = 0;
const PAL_LOW: usize = 1;
const PAL_MID: usize = 2;
const PAL_HIGH: usize = 3;
const PAL_DANGER: usize = 4;
const PAL_PEAK: usize = 5;
const PAL_GUIDE: usize = 6;
const ZONE_COUNT: usize = 4;
const DANGER_THRESHOLD_INDEX: usize = ZONE_COUNT - 2;
const VISIBLE_METER_COUNT: usize = 3;

#[derive(Debug, Clone, Copy)]
struct PeakHold {
    db: f32,
    decay_from: Instant,
}

impl PeakHold {
    fn new(db: f32, now: Instant) -> Self {
        Self {
            db,
            decay_from: now,
        }
    }

    fn update(&mut self, value: f32, now: Instant) {
        if value > self.db {
            self.db = value;
            self.decay_from = now + PEAK_HOLD;
        } else if now > self.decay_from {
            let decay_dt = now.duration_since(self.decay_from).as_secs_f32();
            self.db = (self.db - PEAK_DECAY_DB_PER_SEC * decay_dt).max(value);
            self.decay_from = now;
        }
    }
}

#[derive(Debug, Clone)]
pub(in crate::visuals) struct LoudnessState {
    short_term_loudness: f32,
    momentary_loudness: f32,
    rms_fast_db: [f32; MAX_CHANNELS],
    rms_slow_db: [f32; MAX_CHANNELS],
    true_peak_db: [f32; MAX_CHANNELS],
    channel_count: usize,
    pub(in crate::visuals) left_mode: MeterMode,
    pub(in crate::visuals) right_mode: MeterMode,
    pub(in crate::visuals) palette: [Color; LOUDNESS_PALETTE_SIZE],
    peaks: [PeakHold; VISIBLE_METER_COUNT],
    key: u64,
}

impl LoudnessState {
    pub fn new() -> Self {
        let defaults = LoudnessSettings::default();
        let now = Instant::now();
        let peak = PeakHold::new(DEFAULT_RANGE.0, now);
        Self {
            short_term_loudness: DEFAULT_RANGE.0,
            momentary_loudness: DEFAULT_RANGE.0,
            rms_fast_db: [DEFAULT_RANGE.0; MAX_CHANNELS],
            rms_slow_db: [DEFAULT_RANGE.0; MAX_CHANNELS],
            true_peak_db: [DEFAULT_RANGE.0; MAX_CHANNELS],
            channel_count: 2,
            left_mode: defaults.left_mode,
            right_mode: defaults.right_mode,
            palette: palettes::loudness::COLORS,
            peaks: [peak; VISIBLE_METER_COUNT],
            key: crate::visuals::next_key(),
        }
    }

    pub fn apply_snapshot(&mut self, snapshot: LoudnessSnapshot) {
        self.short_term_loudness = snapshot.short_term_loudness;
        self.momentary_loudness = snapshot.momentary_loudness;
        self.channel_count = snapshot.channel_count.clamp(1, MAX_CHANNELS);
        for i in 0..self.channel_count {
            self.rms_fast_db[i] = snapshot.rms_fast_db[i];
            self.rms_slow_db[i] = snapshot.rms_slow_db[i];
            self.true_peak_db[i] = snapshot.true_peak_db[i];
        }

        self.update_peak_holds(Instant::now());
    }

    pub fn set_modes(&mut self, left: MeterMode, right: MeterMode) {
        if self.left_mode != left || self.right_mode != right {
            self.reset_peaks(Instant::now());
        }
        self.left_mode = left;
        self.right_mode = right;
    }

    pub fn set_palette(&mut self, palette: &[Color; LOUDNESS_PALETTE_SIZE]) {
        self.palette = *palette;
    }

    fn get_value(&self, mode: MeterMode, channel: usize) -> f32 {
        let per_channel =
            |buf: &[f32; MAX_CHANNELS]| buf.get(channel).copied().unwrap_or(DEFAULT_RANGE.0);
        match mode {
            MeterMode::LufsShortTerm => self.short_term_loudness,
            MeterMode::LufsMomentary => self.momentary_loudness,
            MeterMode::RmsFast => per_channel(&self.rms_fast_db),
            MeterMode::RmsSlow => per_channel(&self.rms_slow_db),
            MeterMode::TruePeak => per_channel(&self.true_peak_db),
        }
    }

    fn visual_params(&self, bounds: Rectangle) -> LoudnessParams {
        let (min, max) = DEFAULT_RANGE;
        let guide_color = color_to_rgba(self.palette[PAL_GUIDE]);
        let bg_color = color_to_rgba(with_alpha(self.palette[PAL_BACKGROUND], 1.0));
        let values = self.visible_values();

        LoudnessParams {
            key: self.key,
            bounds,
            min_db: min,
            max_db: max,
            bg_color,
            bars: [
                [
                    self.meter_fill(0, self.left_mode, values[0]),
                    self.meter_fill(1, self.left_mode, values[1]),
                ],
                [self.meter_fill(2, self.right_mode, values[2]); 2],
            ],
            fill_counts: [2, 1],
            guides: &GUIDE_LEVELS,
            guide_color,
            threshold_db: Some(0.0),
            left_padding: LEFT_PADDING,
            right_padding: RIGHT_PADDING,
        }
    }

    fn aggregate_channels(&self, mode: MeterMode, wanted: MeterSide) -> f32 {
        if matches!(mode, MeterMode::LufsShortTerm | MeterMode::LufsMomentary) {
            return self.get_value(mode, 0);
        }
        (0..self.channel_count)
            .filter(|&ch| {
                let side = fallback_side(ch, self.channel_count);
                side == MeterSide::Both || side == wanted
            })
            .map(|ch| self.get_value(mode, ch))
            .fold(DEFAULT_RANGE.0, f32::max)
    }

    fn visible_values(&self) -> [f32; VISIBLE_METER_COUNT] {
        [
            self.aggregate_channels(self.left_mode, MeterSide::Left),
            self.aggregate_channels(self.left_mode, MeterSide::Right),
            self.get_value(self.right_mode, 0),
        ]
    }

    fn meter_fill(&self, peak_index: usize, mode: MeterMode, db: f32) -> MeterFill {
        let peak_db = self.peaks[peak_index].db;
        MeterFill {
            db,
            segments: self.meter_segments(mode),
            peak: (peak_db > DEFAULT_RANGE.0).then(|| {
                let color = self.palette[if is_danger_zone(mode, peak_db) {
                    PAL_DANGER
                } else {
                    PAL_PEAK
                }];
                (peak_db, color_to_rgba(color))
            }),
        }
    }

    fn meter_segments(&self, mode: MeterMode) -> [(f32, [f32; 4]); ZONE_COUNT] {
        let [low, mid, high] = zone_thresholds(mode);
        [
            (low, color_to_rgba(self.palette[PAL_LOW])),
            (mid, color_to_rgba(self.palette[PAL_MID])),
            (high, color_to_rgba(self.palette[PAL_HIGH])),
            (DEFAULT_RANGE.1, color_to_rgba(self.palette[PAL_DANGER])),
        ]
    }

    fn reset_peaks(&mut self, now: Instant) {
        self.peaks.fill(PeakHold::new(DEFAULT_RANGE.0, now));
    }

    fn update_peak_holds(&mut self, now: Instant) {
        let values = self.visible_values();
        let (min, max) = DEFAULT_RANGE;
        for (peak, value) in self.peaks.iter_mut().zip(values) {
            peak.update(value.clamp(min, max), now);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeterSide {
    Left,
    Right,
    Both,
    Neither,
}

fn fallback_side(channel_index: usize, total_channels: usize) -> MeterSide {
    match (total_channels, channel_index) {
        (1, 0) => MeterSide::Both,
        (_, 0) => MeterSide::Left,
        (_, 1) => MeterSide::Right,
        (3 | 5, 2) | (6.., 2) => MeterSide::Both,
        (4, 2) | (5, 3) => MeterSide::Left,
        (4, 3) | (5, 4) => MeterSide::Right,
        (6.., 3) => MeterSide::Neither,
        (6.., i) if i % 2 == 0 => MeterSide::Left,
        (6.., _) => MeterSide::Right,
        _ => MeterSide::Neither,
    }
}

fn zone_thresholds(mode: MeterMode) -> [f32; 3] {
    match mode {
        MeterMode::LufsShortTerm | MeterMode::LufsMomentary => [-24.0, -18.0, -9.0],
        MeterMode::RmsFast | MeterMode::RmsSlow | MeterMode::TruePeak => [-12.0, -6.0, -1.0],
    }
}

fn is_danger_zone(mode: MeterMode, db: f32) -> bool {
    db >= zone_thresholds(mode)[DANGER_THRESHOLD_INDEX]
}

fn visible_guide_labels(params: &LoudnessParams, bounds: Rectangle) -> Vec<(f32, Rectangle)> {
    if bounds.height < GUIDE_LABEL_HEIGHT {
        return Vec::new();
    }

    let max_top = bounds.y + bounds.height - GUIDE_LABEL_HEIGHT;
    let mut labels: Vec<(f32, Rectangle)> = Vec::with_capacity(params.guides.len());

    for &i in &GUIDE_LABEL_ORDER {
        let db = params.guides[i];
        let y = bounds.y + bounds.height * (1.0 - params.db_to_ratio(db));
        let rect = Rectangle::new(
            Point::new(bounds.x, (y - GUIDE_LABEL_HEIGHT * 0.5).clamp(bounds.y, max_top)),
            Size::new(LEFT_PADDING, GUIDE_LABEL_HEIGHT),
        );

        if !labels
            .iter()
            .any(|(_, r)| r.expand(GUIDE_LABEL_GAP).intersects(&rect))
        {
            labels.push((db, rect));
        }
    }

    labels
}

crate::visuals::visualization_widget!(Loudness, LoudnessState, |this, renderer, theme, bounds| {
    let state = this.state.borrow();
    let params = state.visual_params(bounds);

    renderer.draw_primitive(bounds, LoudnessPrimitive::new(params.clone()));

    let palette = theme.extended_palette();
    let label_color = state.palette[PAL_GUIDE];

    if let Some((meter_x, bar_width, stride)) = params.meter_bounds() {
        let y_of = |db| bounds.y + bounds.height * (1.0 - params.db_to_ratio(db));

        for (db, rect) in visible_guide_labels(&params, bounds) {
            let label = if db == 0.0 { "0".to_owned() } else { format!("{db:+.0}") };

            let mut text = make_text(label, LABEL_FONT_SIZE, rect.size());
            text.align_x = Horizontal::Right.into();
            text.align_y = Vertical::Center;
            text::Renderer::fill_text(
                renderer,
                text,
                Point::new(rect.x + rect.width - 4.0, rect.y + rect.height * 0.5),
                label_color,
                bounds,
            );
        }

        let value = state.get_value(state.right_mode, 0);
        let unit = state.right_mode.unit_label();
        let y = y_of(value);
        let label = format!("{value:.1} {unit}");

        let label_x = meter_x + stride + bar_width + 4.0;
        let clamp_max = (bounds.y + bounds.height - 20.0).max(bounds.y);
        let label_rect = Rectangle {
            x: label_x,
            y: (y - 10.0).clamp(bounds.y, clamp_max),
            width: 68.0,
            height: 20.0,
        };

        fill_rect(
            renderer,
            label_rect,
            with_alpha(state.palette[PAL_BACKGROUND], 1.0),
        );

        let mut text = make_text(
            label,
            VALUE_FONT_SIZE,
            Size::new(label_rect.width, label_rect.height),
        );
        text.font = iced::Font {
            weight: iced::font::Weight::Bold,
            ..Default::default()
        };
        text.align_x = Horizontal::Center.into();
        text.align_y = Vertical::Center;
        text::Renderer::fill_text(
            renderer,
            text,
            Point::new(
                label_rect.x + label_rect.width / 2.0,
                label_rect.y + label_rect.height / 2.0,
            ),
            palette.background.base.text,
            bounds,
        );
    }
});

#[cfg(test)]
mod tests {
    use super::*;

    fn visible_bar_values(state: &LoudnessState) -> Vec<Vec<f32>> {
        let params = state.visual_params(Rectangle::new(Point::ORIGIN, Size::new(200.0, 100.0)));
        params
            .bars
            .iter()
            .zip(params.fill_counts)
            .map(|(bar, n)| bar.iter().take(n).map(|fill| fill.db).collect())
            .collect()
    }

    #[test]
    fn visible_bars_use_configured_modes_and_channel_aggregation() {
        let mut state = LoudnessState::new();
        state.apply_snapshot(LoudnessSnapshot {
            short_term_loudness: -9.0,
            momentary_loudness: -7.5,
            rms_fast_db: [-15.0, -12.0, -20.0, -60.0, -6.0, -3.0, 0.0, 0.0],
            rms_slow_db: [-14.0, -8.0, -20.0, -60.0, -6.0, -3.0, 0.0, 0.0],
            true_peak_db: [-12.0, -18.0, -2.0, -60.0, -9.0, -6.0, 0.0, 0.0],
            channel_count: 6,
        });

        assert_eq!(visible_bar_values(&state), vec![vec![-2.0, -2.0], vec![-9.0]]);

        state.set_modes(MeterMode::RmsFast, MeterMode::LufsMomentary);
        assert_eq!(visible_bar_values(&state), vec![vec![-6.0, -3.0], vec![-7.5]]);
    }

    #[test]
    fn visible_bars_follow_fallback_channel_layouts() {
        let snapshot = |true_peak_db, channel_count| LoudnessSnapshot {
            short_term_loudness: -9.0,
            momentary_loudness: -9.0,
            rms_fast_db: [DEFAULT_RANGE.0; MAX_CHANNELS],
            rms_slow_db: [DEFAULT_RANGE.0; MAX_CHANNELS],
            true_peak_db,
            channel_count,
        };
        let mut state = LoudnessState::new();
        state.set_modes(MeterMode::TruePeak, MeterMode::LufsShortTerm);

        let mut mono = [DEFAULT_RANGE.0; MAX_CHANNELS];
        mono[0] = -12.0;
        state.apply_snapshot(snapshot(mono, 1));
        assert_eq!(visible_bar_values(&state)[0], vec![-12.0, -12.0]);

        let mut quad = [DEFAULT_RANGE.0; MAX_CHANNELS];
        quad[2] = -6.0;
        quad[3] = -3.0;
        state.apply_snapshot(snapshot(quad, 4));
        assert_eq!(visible_bar_values(&state)[0], vec![-6.0, -3.0]);
    }

    #[test]
    fn peak_hold_waits_before_decaying() {
        let mut state = LoudnessState::new();
        let start = Instant::now();

        for (input, elapsed, expected) in
            [(-1.0, 0.0, -1.0), (-20.0, 1.0, -1.0), (-60.0, 2.5, -31.0)]
        {
            state.true_peak_db[0] = input;
            state.update_peak_holds(start + Duration::from_secs_f32(elapsed));
            assert!((state.peaks[0].db - expected).abs() < 0.01);
        }
    }
}
