// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{LoudnessSnapshot, MAX_CHANNELS};
use super::render::{
    DB_RANGE, GUIDE_LEVELS, LEFT_PADDING, LoudnessParams, LoudnessPrimitive, MeterFill, db_to_ratio,
};
use crate::dsp::ChannelPosition;
use crate::persistence::settings::LoudnessSettings;
use crate::visuals::options::MeterMode;
use crate::visuals::palettes::{self, loudness::SIZE as PALETTE_SIZE};
use crate::util::color::color_to_rgba;
use crate::visuals::render::common::{fill_rect, text as raw_text};
use iced::advanced::{graphics::text::Paragraph, text};
use iced::advanced::text::Paragraph as _;
use iced::alignment::{Horizontal, Vertical};
use iced::{Color, Point, Rectangle, Size};
use std::time::{Duration, Instant};

const GUIDE_LABELS: [&str; 6] = ["0", "-6", "-12", "-18", "-24", "-36"];
const PEAK_HOLD: Duration = Duration::from_secs(2);
const PEAK_DECAY_DB_PER_SEC: f32 = 60.0;
const GUIDE_LABEL_HEIGHT: f32 = 12.0;
const GUIDE_LABEL_GAP: f32 = 2.0;
const GUIDE_LABEL_ORDER: [usize; GUIDE_LEVELS.len()] = [0, 2, 5, 3, 4, 1];

const PAL_BACKGROUND: usize = 0;
const PAL_LOW: usize = 1;
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
pub(crate) struct LoudnessState {
    snapshot: LoudnessSnapshot,
    pub(in crate::visuals) settings: LoudnessSettings,
    pub(in crate::visuals) palette: [Color; PALETTE_SIZE],
    peaks: [PeakHold; VISIBLE_METER_COUNT],
    guide_labels: [Paragraph; GUIDE_LABELS.len()],
    value_label: (String, Paragraph),
    key: u64,
    geometry_revision: u64,
}

impl LoudnessState {
    pub fn new() -> Self {
        let mut snapshot = LoudnessSnapshot::with_floor(DB_RANGE.0);
        snapshot.channel_count = 2;
        let peak = PeakHold::new(DB_RANGE.0, Instant::now());
        let mut state = Self {
            snapshot,
            settings: LoudnessSettings::default(),
            palette: palettes::loudness::COLORS,
            peaks: [peak; VISIBLE_METER_COUNT],
            guide_labels: GUIDE_LABELS.map(|label| {
                let mut text = raw_text(label, 10.0, Size::new(LEFT_PADDING, GUIDE_LABEL_HEIGHT));
                text.align_x = Horizontal::Right.into();
                text.align_y = Vertical::Center;
                Paragraph::with_text(text)
            }),
            value_label: (String::new(), Paragraph::default()),
            key: crate::visuals::next_key(),
            geometry_revision: 0,
        };
        state.refresh_value_label();
        state
    }

    pub fn reset_audio(&mut self) {
        let mut snapshot = LoudnessSnapshot::with_floor(DB_RANGE.0);
        snapshot.channel_count = 2;
        self.snapshot = snapshot;
        self.peaks = [PeakHold::new(DB_RANGE.0, Instant::now()); VISIBLE_METER_COUNT];
        self.refresh_value_label();
        self.geometry_revision = self.geometry_revision.wrapping_add(1);
    }

    pub fn apply_snapshot(&mut self, mut snapshot: LoudnessSnapshot) {
        snapshot.channel_count = snapshot.channel_count.clamp(1, MAX_CHANNELS);
        self.snapshot = snapshot;
        self.update_peak_holds(Instant::now());
        self.refresh_value_label();
        self.geometry_revision = self.geometry_revision.wrapping_add(1);
    }

    pub fn set_modes(&mut self, left: MeterMode, right: MeterMode) {
        if self.settings.left_mode != left || self.settings.right_mode != right {
            self.peaks
                .fill(PeakHold::new(DB_RANGE.0, Instant::now()));
        }
        self.settings.left_mode = left;
        self.settings.right_mode = right;
        self.refresh_value_label();
        self.geometry_revision = self.geometry_revision.wrapping_add(1);
    }

    pub fn set_palette(&mut self, palette: &[Color; PALETTE_SIZE]) {
        self.palette = *palette;
        self.geometry_revision = self.geometry_revision.wrapping_add(1);
    }

    fn get_value(&self, mode: MeterMode, channel: usize) -> f32 {
        let per_channel =
            |buf: &[f32; MAX_CHANNELS]| buf.get(channel).copied().unwrap_or(DB_RANGE.0);
        match mode {
            MeterMode::LufsShortTerm => self.snapshot.short_term_loudness,
            MeterMode::LufsMomentary => self.snapshot.momentary_loudness,
            MeterMode::RmsFast => per_channel(&self.snapshot.rms_fast_db),
            MeterMode::RmsSlow => per_channel(&self.snapshot.rms_slow_db),
            MeterMode::TruePeak => per_channel(&self.snapshot.true_peak_db),
        }
    }

    fn visual_params(&self, bounds: Rectangle) -> LoudnessParams {
        let values = self.visible_values();
        LoudnessParams {
            key: self.key,
            geometry_revision: self.geometry_revision,
            bounds,
            bg_color: color_to_rgba(self.palette[PAL_BACKGROUND]),
            bars: [
                [
                    self.meter_fill(0, self.settings.left_mode, values[0]),
                    self.meter_fill(1, self.settings.left_mode, values[1]),
                ],
                [self.meter_fill(2, self.settings.right_mode, values[2]); 2],
            ],
            guide_color: color_to_rgba(self.palette[PAL_GUIDE]),
        }
    }

    fn aggregate_channels(&self, mode: MeterMode, wanted: MeterSide) -> f32 {
        if matches!(mode, MeterMode::LufsShortTerm | MeterMode::LufsMomentary) {
            return self.get_value(mode, 0);
        }
        (0..self.snapshot.channel_count)
            .filter(|&ch| {
                let side = channel_side(
                    self.snapshot.positions[ch],
                    ch,
                    self.snapshot.channel_count,
                );
                side == MeterSide::Both || side == wanted
            })
            .map(|ch| self.get_value(mode, ch))
            .fold(DB_RANGE.0, f32::max)
    }

    fn visible_values(&self) -> [f32; VISIBLE_METER_COUNT] {
        [
            self.aggregate_channels(self.settings.left_mode, MeterSide::Left),
            self.aggregate_channels(self.settings.left_mode, MeterSide::Right),
            self.get_value(self.settings.right_mode, 0),
        ]
    }

    fn meter_fill(&self, peak_index: usize, mode: MeterMode, db: f32) -> MeterFill {
        let peak_db = self.peaks[peak_index].db;
        MeterFill {
            db,
            segments: self.meter_segments(mode),
            peak: (peak_db > DB_RANGE.0).then(|| {
                let danger = peak_db >= zone_thresholds(mode)[DANGER_THRESHOLD_INDEX];
                let color = self.palette[if danger { PAL_DANGER } else { PAL_PEAK }];
                (peak_db, color_to_rgba(color))
            }),
        }
    }

    fn meter_segments(&self, mode: MeterMode) -> [(f32, [f32; 4]); ZONE_COUNT] {
        let [low, mid, high] = zone_thresholds(mode);
        let thresholds = [low, mid, high, DB_RANGE.1];
        std::array::from_fn(|i| (thresholds[i], color_to_rgba(self.palette[PAL_LOW + i])))
    }

    fn refresh_value_label(&mut self) {
        let mode = self.settings.right_mode;
        let unit = match mode {
            MeterMode::LufsShortTerm | MeterMode::LufsMomentary => "LUFS",
            MeterMode::RmsFast | MeterMode::RmsSlow => "dB",
            MeterMode::TruePeak => "dBTP",
        };
        let text = format!("{:.1} {unit}", self.get_value(mode, 0));
        if self.value_label.0 != text {
            let paragraph = value_label(&text);
            self.value_label = (text, paragraph);
        }
    }

    fn update_peak_holds(&mut self, now: Instant) {
        let values = self.visible_values();
        let (min, max) = DB_RANGE;
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

fn channel_side(
    position: ChannelPosition,
    channel_index: usize,
    total_channels: usize,
) -> MeterSide {
    let position = if matches!(position, ChannelPosition::Aux(_) | ChannelPosition::Unknown) {
        ChannelPosition::fallback(total_channels)[channel_index]
    } else {
        position
    };
    match position {
        ChannelPosition::FrontLeft | ChannelPosition::RearLeft | ChannelPosition::SideLeft => {
            MeterSide::Left
        }
        ChannelPosition::FrontRight | ChannelPosition::RearRight | ChannelPosition::SideRight => {
            MeterSide::Right
        }
        ChannelPosition::FrontCenter | ChannelPosition::Mono => MeterSide::Both,
        ChannelPosition::LowFrequency | ChannelPosition::Aux(_) | ChannelPosition::Unknown => {
            MeterSide::Neither
        }
    }
}

fn zone_thresholds(mode: MeterMode) -> [f32; 3] {
    match mode {
        MeterMode::LufsShortTerm | MeterMode::LufsMomentary => [-24.0, -18.0, -9.0],
        MeterMode::RmsFast | MeterMode::RmsSlow | MeterMode::TruePeak => [-12.0, -6.0, -1.0],
    }
}

fn value_label(label: &str) -> Paragraph {
    let mut text = raw_text(label, 12.0, Size::INFINITE);
    text.font = iced::Font {
        weight: iced::font::Weight::Bold,
        ..Default::default()
    };
    Paragraph::with_text(text)
}

fn visible_guide_labels(
    bounds: Rectangle,
) -> [Option<(usize, Rectangle)>; GUIDE_LABEL_ORDER.len()] {
    let mut labels = [None; GUIDE_LABEL_ORDER.len()];
    if bounds.height < GUIDE_LABEL_HEIGHT {
        return labels;
    }

    let max_top = bounds.y + bounds.height - GUIDE_LABEL_HEIGHT;
    let mut len = 0;
    for &i in &GUIDE_LABEL_ORDER {
        let db = GUIDE_LEVELS[i];
        let y = bounds.y + bounds.height * (1.0 - db_to_ratio(db));
        let rect = Rectangle::new(
            Point::new(bounds.x, (y - GUIDE_LABEL_HEIGHT * 0.5).clamp(bounds.y, max_top)),
            Size::new(LEFT_PADDING, GUIDE_LABEL_HEIGHT),
        );

        if !labels[..len]
            .iter()
            .flatten()
            .any(|(_, r)| r.expand(GUIDE_LABEL_GAP).intersects(&rect))
        {
            labels[len] = Some((i, rect));
            len += 1;
        }
    }

    labels
}

crate::visuals::visualization_widget!(Loudness, LoudnessState, |this, renderer, theme, bounds| {
    let state = this.state.borrow();
    let params = state.visual_params(bounds);
    let meter_bounds = params.meter_bounds();

    renderer.draw_primitive(bounds, LoudnessPrimitive::new(params));

    let palette = theme.extended_palette();
    let label_color = state.palette[PAL_GUIDE];

    if let Some((meter_x, bar_width, stride)) = meter_bounds {
        let y_of = |db| bounds.y + bounds.height * (1.0 - db_to_ratio(db));

        for (i, rect) in visible_guide_labels(bounds).into_iter().flatten() {
            let size = state.guide_labels[i].min_bounds();
            text::Renderer::fill_paragraph(
                renderer,
                &state.guide_labels[i],
                Point::new(rect.x + rect.width - 4.0 - size.width, rect.y + (rect.height - size.height) * 0.5),
                label_color,
                bounds,
            );
        }

        let value = state.get_value(state.settings.right_mode, 0);
        let y = y_of(value);

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
            state.palette[PAL_BACKGROUND],
        );

        let label = &state.value_label.1;
        let label_size = label.min_bounds();
        text::Renderer::fill_paragraph(
            renderer,
            label,
            Point::new(
                label_rect.center_x() - label_size.width * 0.5,
                label_rect.center_y() - label_size.height * 0.5,
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
            .zip([2, 1])
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
            positions: ChannelPosition::fallback(6),
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
            rms_fast_db: [DB_RANGE.0; MAX_CHANNELS],
            rms_slow_db: [DB_RANGE.0; MAX_CHANNELS],
            true_peak_db,
            channel_count,
            positions: [ChannelPosition::Unknown; MAX_CHANNELS],
        };
        let mut state = LoudnessState::new();
        state.set_modes(MeterMode::TruePeak, MeterMode::LufsShortTerm);

        let mut mono = [DB_RANGE.0; MAX_CHANNELS];
        mono[0] = -12.0;
        state.apply_snapshot(snapshot(mono, 1));
        assert_eq!(visible_bar_values(&state)[0], vec![-12.0, -12.0]);

        let mut quad = [DB_RANGE.0; MAX_CHANNELS];
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
            state.snapshot.true_peak_db[0] = input;
            state.update_peak_holds(start + Duration::from_secs_f32(elapsed));
            assert!((state.peaks[0].db - expected).abs() < 0.01);
        }
    }
}
