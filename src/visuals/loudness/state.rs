// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{DEFAULT_FLOOR_DB, LoudnessSnapshot, Programme};
use super::render::{
    DB_RANGE, GUIDE_LEVELS, LEFT_PADDING, LoudnessParams, MeterFill, label_top, value_label_bounds,
};
use crate::dsp::ChannelPosition;
use crate::persistence::settings::LoudnessSettings;
use crate::util::color::{color_to_rgba, lerp_color};
use crate::visuals::options::MeterMode;
use crate::visuals::palettes::{self, loudness::SIZE as PALETTE_SIZE};
use crate::visuals::render::common::{fill_rect, text as raw_text};
use iced::advanced::text::{Paragraph as _, paragraph::Plain};
use iced::advanced::{graphics::text::Paragraph, mouse, text};
use iced::{Color, Point, Rectangle, Size};

const PEAK_HOLD_SECONDS: f32 = 2.0;
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

// Held and decayed in audio seconds.
#[derive(Debug, Clone, Copy)]
struct PeakHold {
    db: f32,
    hold: f32,
}

impl PeakHold {
    const FLOOR: Self = Self {
        db: DB_RANGE.0,
        hold: 0.0,
    };

    fn update(&mut self, value: f32, elapsed: f32) {
        if value >= self.db {
            self.db = value;
            self.hold = PEAK_HOLD_SECONDS;
        } else {
            let decay = (elapsed - self.hold).max(0.0);
            self.hold = (self.hold - elapsed).max(0.0);
            self.db = (self.db - PEAK_DECAY_DB_PER_SEC * decay).max(value);
        }
    }
}

pub(crate) struct LoudnessState {
    snapshot: LoudnessSnapshot,
    programme: Programme,
    pub(in crate::visuals) settings: LoudnessSettings,
    pub(in crate::visuals) palette: [Color; PALETTE_SIZE],
    peaks: [PeakHold; VISIBLE_METER_COUNT],
    guide_labels: [Paragraph; GUIDE_LEVELS.len()],
    value_label: Plain<Paragraph>,
    range_label: Plain<Paragraph>,
    geometry: crate::visuals::GeometryKey,
    last_click: Option<mouse::Click>,
    pub(in crate::visuals) reset_requested: bool,
}

impl Default for LoudnessState {
    fn default() -> Self {
        let mut state = Self {
            snapshot: LoudnessSnapshot::with_floor(DEFAULT_FLOOR_DB, 2),
            programme: Programme::default(),
            settings: LoudnessSettings::default(),
            palette: palettes::loudness::COLORS,
            peaks: [PeakHold::FLOOR; VISIBLE_METER_COUNT],
            guide_labels: GUIDE_LEVELS.map(|level| {
                Paragraph::with_text(raw_text(
                    level.to_string().as_str(),
                    10.0,
                    Size::new(LEFT_PADDING, GUIDE_LABEL_HEIGHT),
                ))
            }),
            value_label: Plain::default(),
            range_label: Plain::default(),
            geometry: crate::visuals::GeometryKey::new(),
            last_click: None,
            reset_requested: false,
        };
        state.refresh();
        state
    }
}

impl LoudnessState {
    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> (LoudnessSnapshot, Programme) {
        (self.snapshot, self.programme)
    }

    pub fn reset_audio(&mut self) {
        self.snapshot = LoudnessSnapshot::with_floor(DEFAULT_FLOOR_DB, 2);
        self.peaks = [PeakHold::FLOOR; VISIBLE_METER_COUNT];
        self.refresh();
    }

    pub fn reset_measurement(&mut self) {
        self.reset_requested = true;
        self.programme = Programme::default();
        self.refresh();
    }

    fn update(&mut self, event: &iced::Event, cursor: mouse::Cursor, bounds: Rectangle) -> bool {
        let (iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)), Some(position)) =
            (event, cursor.position_over(bounds))
        else {
            return false;
        };
        let click = mouse::Click::new(position, mouse::Button::Left, self.last_click);
        self.last_click = Some(click);
        let reset = click.kind() == mouse::click::Kind::Double
            && [self.settings.left_mode, self.settings.right_mode]
                .contains(&MeterMode::LufsIntegrated);
        if reset {
            self.reset_measurement();
        }
        reset
    }

    pub fn apply_snapshot(&mut self, (snapshot, programme): (LoudnessSnapshot, Programme)) {
        self.snapshot = snapshot;
        self.programme = programme;
        self.update_peak_holds();
        self.refresh();
    }

    pub(in crate::visuals) fn is_quiescent(&self) -> bool {
        let settled = LoudnessSnapshot {
            elapsed: self.snapshot.elapsed,
            positions: self.snapshot.positions,
            ..LoudnessSnapshot::with_floor(
                self.snapshot.short_term_loudness,
                self.snapshot.channel_count,
            )
        };
        self.snapshot == settled && self.peaks.iter().all(|peak| peak.db <= DB_RANGE.0)
    }

    pub fn update_view_settings(&mut self, settings: &LoudnessSettings) {
        if self.settings != *settings {
            self.peaks.fill(PeakHold::FLOOR);
        }
        self.settings = *settings;
        self.refresh();
    }

    crate::visuals::palette_setter!(loudness => geometry);

    fn visual_params(&self, bounds: Rectangle) -> LoudnessParams {
        let meters = self.visible_meters();
        let range = (self.programme.range)
            .map(|range| (range, color_to_rgba(self.dim(self.palette[PAL_GUIDE]))));
        let wick = |mode| range.filter(|_| mode == MeterMode::LufsIntegrated);
        LoudnessParams {
            geometry: self.geometry,
            bounds,
            bg_color: color_to_rgba(self.palette[PAL_BACKGROUND]),
            bars: std::array::from_fn(|i| self.meter_fill(self.peaks[i].db, meters[i])),
            wicks: [self.settings.left_mode, self.settings.right_mode].map(wick),
            guide_color: color_to_rgba(self.palette[PAL_GUIDE]),
        }
    }

    fn dim(&self, color: Color) -> Color {
        if self.programme.stable {
            color
        } else {
            lerp_color(color, self.palette[PAL_BACKGROUND], 0.5)
        }
    }

    fn aggregate_channels(&self, mode: MeterMode, wanted: MeterSide) -> f32 {
        // Programme loudness is one value; the other modes are per channel.
        let levels = match mode {
            MeterMode::LufsShortTerm => return self.snapshot.short_term_loudness,
            MeterMode::LufsMomentary => return self.snapshot.momentary_loudness,
            MeterMode::LufsIntegrated => {
                return (self.programme.integrated).unwrap_or(DEFAULT_FLOOR_DB);
            }
            MeterMode::RmsFast => &self.snapshot.rms_fast_db,
            MeterMode::RmsSlow => &self.snapshot.rms_slow_db,
            MeterMode::TruePeak => &self.snapshot.true_peak_db,
        };
        (0..self.snapshot.channel_count)
            .filter(|&ch| {
                let side =
                    channel_side(self.snapshot.positions[ch], ch, self.snapshot.channel_count);
                wanted == MeterSide::Both || side == MeterSide::Both || side == wanted
            })
            .map(|ch| levels[ch])
            .reduce(f32::max)
            .unwrap_or(DB_RANGE.0)
    }

    fn visible_meters(&self) -> [(MeterMode, f32); VISIBLE_METER_COUNT] {
        let (left, right) = (self.settings.left_mode, self.settings.right_mode);
        [
            (left, MeterSide::Left),
            (left, MeterSide::Right),
            (right, MeterSide::Both),
        ]
        .map(|(mode, side)| (mode, self.aggregate_channels(mode, side)))
    }

    fn meter_fill(&self, peak_db: f32, (mode, db): (MeterMode, f32)) -> MeterFill {
        let (_, thresholds) = scale(mode);
        let colors = &self.palette[PAL_LOW..];
        MeterFill {
            db,
            segments: std::array::from_fn(|i| (thresholds[i], color_to_rgba(colors[i]))),
            peak: (peak_db > DB_RANGE.0).then(|| {
                let danger = peak_db >= thresholds[DANGER_THRESHOLD_INDEX];
                let color = self.palette[if danger { PAL_DANGER } else { PAL_PEAK }];
                (peak_db, color_to_rgba(color))
            }),
        }
    }

    fn refresh(&mut self) {
        let mode = self.settings.right_mode;
        let value = self.aggregate_channels(mode, MeterSide::Both);
        let text = format!("{value:.1} {}", scale(mode).0);
        self.value_label.update(value_label(&text));

        let range = match self.programme.range {
            _ if mode != MeterMode::LufsIntegrated => String::new(),
            Some([low, high]) => format!("{:.1} LU", high - low),
            None => "--".to_owned(),
        };
        (self.range_label).update(raw_text(&range, 10.0, Size::INFINITE));
        self.geometry.invalidate();
    }

    fn update_peak_holds(&mut self) {
        let meters = self.visible_meters();
        for (peak, (mode, value)) in self.peaks.iter_mut().zip(meters) {
            // Integrated loudness holds its level, so its peak would never settle.
            if mode != MeterMode::LufsIntegrated {
                peak.update(value.clamp(DB_RANGE.0, DB_RANGE.1), self.snapshot.elapsed);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeterSide {
    Left,
    Right,
    Both,
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
        ChannelPosition::FrontLeft
        | ChannelPosition::FrontLeftCenter
        | ChannelPosition::RearLeft
        | ChannelPosition::SideLeft => MeterSide::Left,
        ChannelPosition::FrontRight
        | ChannelPosition::FrontRightCenter
        | ChannelPosition::RearRight
        | ChannelPosition::SideRight => MeterSide::Right,
        _ => MeterSide::Both,
    }
}

// Unit and zone ceilings.
fn scale(mode: MeterMode) -> (&'static str, [f32; ZONE_COUNT]) {
    const DBFS: [f32; ZONE_COUNT] = [-12.0, -6.0, -1.0, DB_RANGE.1];
    match mode {
        MeterMode::LufsShortTerm | MeterMode::LufsMomentary | MeterMode::LufsIntegrated => {
            ("LUFS", [-24.0, -18.0, -9.0, DB_RANGE.1])
        }
        MeterMode::RmsFast | MeterMode::RmsSlow => ("dB", DBFS),
        MeterMode::TruePeak => ("dBTP", DBFS),
    }
}

fn value_label(label: &str) -> text::Text<&str> {
    let mut text = raw_text(label, 12.0, Size::INFINITE);
    text.font = iced::Font {
        weight: iced::font::Weight::Bold,
        ..Default::default()
    };
    text
}

fn visible_guide_labels(
    bounds: Rectangle,
) -> [Option<(usize, Rectangle)>; GUIDE_LABEL_ORDER.len()] {
    let mut labels = [None; GUIDE_LABEL_ORDER.len()];
    if bounds.width < LEFT_PADDING || bounds.height < GUIDE_LABEL_HEIGHT {
        return labels;
    }

    let mut len = 0;
    for &i in &GUIDE_LABEL_ORDER {
        let rect = Rectangle::new(
            Point::new(
                bounds.x,
                label_top(bounds, GUIDE_LEVELS[i], GUIDE_LABEL_HEIGHT),
            ),
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

crate::visuals::visualization_widget!(
    Loudness,
    LoudnessState,
    update,
    |this, renderer, theme, bounds| {
        let state = this.state.borrow();
        let params = state.visual_params(bounds);
        let meter_bounds = params.meter_bounds();
        let value = params.bars[2].db;

        renderer.draw_primitive(bounds, params);

        if let Some(meters @ (meter_x, _, _)) = meter_bounds {
            let guide_bounds = Rectangle {
                width: meter_x - bounds.x,
                ..bounds
            };
            for (i, rect) in visible_guide_labels(guide_bounds).into_iter().flatten() {
                let size = state.guide_labels[i].min_bounds();
                text::Renderer::fill_paragraph(
                    renderer,
                    &state.guide_labels[i],
                    Point::new(
                        rect.x + rect.width - 4.0 - size.width,
                        rect.y + (rect.height - size.height) * 0.5,
                    ),
                    state.palette[PAL_GUIDE],
                    bounds,
                );
            }

            let range = &state.range_label;
            let Some((label_rect, heights)) =
                value_label_bounds(bounds, meters, value, !range.content().is_empty())
            else {
                return;
            };

            fill_rect(renderer, label_rect, state.palette[PAL_BACKGROUND]);

            let text_color = theme.extended_palette().background.base.text;
            let lines = [
                (&state.value_label, text_color),
                (range, state.dim(text_color)),
            ];
            let mut top = label_rect.y;
            for (height, (label, color)) in heights.iter().zip(lines) {
                let size = label.min_bounds();
                text::Renderer::fill_paragraph(
                    renderer,
                    label.raw(),
                    Point::new(
                        label_rect.center_x() - size.width * 0.5,
                        top + (height - size.height) * 0.5,
                    ),
                    color,
                    bounds,
                );
                top += height;
            }
        }
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::MAX_AUDIO_CHANNELS;

    fn set_modes(state: &mut LoudnessState, left_mode: MeterMode, right_mode: MeterMode) {
        state.update_view_settings(&LoudnessSettings {
            left_mode,
            right_mode,
        });
    }

    fn visible_bar_values(state: &LoudnessState) -> [f32; VISIBLE_METER_COUNT] {
        state
            .visual_params(Rectangle::new(Point::ORIGIN, Size::new(200.0, 100.0)))
            .bars
            .map(|fill| fill.db)
    }

    #[test]
    fn compact_meters_and_labels_stay_inside_the_pane() {
        use super::super::render::MIN_LABELED_WIDTH;

        let state = LoudnessState::default();
        for (width, height, guides_visible, value_visible) in [
            (1.0, 100.0, false, false),
            (80.0, 100.0, true, false),
            (MIN_LABELED_WIDTH - 1.0, 100.0, true, false),
            (MIN_LABELED_WIDTH, 100.0, true, true),
            (MIN_LABELED_WIDTH, 20.0, true, true),
            (MIN_LABELED_WIDTH, 10.0, false, false),
        ] {
            let bounds = Rectangle::new(Point::new(10.0, 20.0), Size::new(width, height));
            let meters @ (x, bar_width, stride) =
                state.visual_params(bounds).meter_bounds().unwrap();
            assert!(bar_width > 0.0 && x >= bounds.x && x + stride + bar_width <= bounds.x + width);
            let guide_bounds = Rectangle {
                width: x - bounds.x,
                ..bounds
            };
            let guides = visible_guide_labels(guide_bounds);
            assert_eq!(guides.iter().any(Option::is_some), guides_visible);
            assert!(
                guides
                    .into_iter()
                    .flatten()
                    .all(|(_, r)| r.is_within(&guide_bounds))
            );
            let value = value_label_bounds(bounds, meters, -12.0, true).map(|(rect, _)| rect);
            assert_eq!(value.is_some(), value_visible);
            assert!(value.is_none_or(|r| r.is_within(&bounds) && r.x >= x + stride + bar_width));
        }
        assert!(
            state
                .visual_params(Rectangle::default())
                .meter_bounds()
                .is_none()
        );
    }

    #[test]
    fn visible_bars_use_configured_modes_and_channel_aggregation() {
        let mut state = LoudnessState::default();
        let snapshot = LoudnessSnapshot {
            elapsed: 0.0,
            short_term_loudness: -9.0,
            momentary_loudness: -7.5,
            rms_fast_db: [-15.0, -12.0, -20.0, -60.0, -6.0, -3.0, 0.0, 0.0],
            rms_slow_db: [-14.0, -8.0, -20.0, -60.0, -6.0, -3.0, 0.0, 0.0],
            true_peak_db: [-12.0, -18.0, -2.0, -60.0, -9.0, -6.0, 0.0, 0.0],
            channel_count: 6,
            positions: ChannelPosition::fallback(6),
        };
        let programme = Programme {
            integrated: Some(-14.0),
            range: Some([-20.0, -12.0]),
            stable: false,
        };
        state.apply_snapshot((snapshot, programme));

        assert_eq!(visible_bar_values(&state), [-2.0, -2.0, -9.0]);

        set_modes(&mut state, MeterMode::RmsFast, MeterMode::LufsMomentary);
        assert_eq!(visible_bar_values(&state), [-6.0, -3.0, -7.5]);
        assert_eq!(state.value_label.content(), "-7.5 LUFS");

        set_modes(&mut state, MeterMode::RmsSlow, MeterMode::RmsFast);
        assert_eq!(visible_bar_values(&state), [-6.0, -3.0, -3.0]);
        assert_eq!(state.value_label.content(), "-3.0 dB");

        set_modes(&mut state, MeterMode::LufsShortTerm, MeterMode::RmsSlow);
        assert_eq!(visible_bar_values(&state), [-9.0, -9.0, -3.0]);
        assert_eq!(state.value_label.content(), "-3.0 dB");
        assert!(state.range_label.content().is_empty());

        set_modes(&mut state, MeterMode::RmsSlow, MeterMode::LufsIntegrated);
        state.apply_snapshot((snapshot, programme));
        assert_eq!(visible_bar_values(&state), [-6.0, -3.0, -14.0]);
        fn labels(state: &LoudnessState) -> [&str; 2] {
            [state.value_label.content(), state.range_label.content()]
        }
        assert_eq!(labels(&state), ["-14.0 LUFS", "8.0 LU"]);
        let bounds = Rectangle::new(Point::ORIGIN, Size::new(200.0, 100.0));
        let params = state.visual_params(bounds);
        assert!(params.bars[0].peak.is_some() && params.bars[2].peak.is_none());
        assert!(params.wicks[0].is_none());
        let (range, color) = params.wicks[1].unwrap();
        assert_eq!(range, [-20.0, -12.0]);
        assert_ne!(color, color_to_rgba(state.palette[PAL_GUIDE]));

        let click = |state: &mut LoudnessState, position| {
            let press = iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
            state.update(&press, mouse::Cursor::Available(position), bounds)
        };
        let outside = Point::new(-1.0, 10.0);
        assert!(!click(&mut state, outside) && !click(&mut state, outside));
        let position = Point::new(10.0, 10.0);
        assert!(!click(&mut state, position) && click(&mut state, position));
        assert!(std::mem::take(&mut state.reset_requested));
        assert_eq!(visible_bar_values(&state)[2], DEFAULT_FLOOR_DB);
        assert_eq!(labels(&state), ["-99.9 LUFS", "--"]);
        set_modes(&mut state, MeterMode::RmsSlow, MeterMode::RmsSlow);
        assert!(!click(&mut state, position) && !click(&mut state, position));
        assert!(!state.reset_requested);
    }

    #[test]
    fn true_peak_bars_and_label_include_every_channel_regardless_of_order() {
        let floor = DB_RANGE.0;
        let expected = [
            [6.0, floor, 6.0],
            [floor, 6.0, 6.0],
            [6.0; 3],
            [6.0; 3],
            [6.0, floor, 6.0],
            [floor, 6.0, 6.0],
            [6.0, floor, 6.0],
            [floor, 6.0, 6.0],
        ];
        let mut state = LoudnessState::default();
        set_modes(&mut state, MeterMode::TruePeak, MeterMode::TruePeak);
        for order in [[0, 1, 2, 3, 4, 5, 6, 7], [7, 4, 2, 0, 6, 5, 1, 3]] {
            for (channel, &position) in order.iter().enumerate() {
                let mut snapshot = LoudnessSnapshot::with_floor(floor, MAX_AUDIO_CHANNELS);
                snapshot.positions = order.map(|index| ChannelPosition::SURROUND[index]);
                snapshot.true_peak_db[channel] = 6.0;
                state.apply_snapshot((snapshot, Programme::default()));
                assert_eq!(
                    visible_bar_values(&state),
                    expected[position],
                    "channel={channel}, order={order:?}"
                );
                assert_eq!(state.value_label.content(), "6.0 dBTP");
            }
        }
    }

    #[test]
    fn true_peak_label_preserves_levels_below_the_bar_range() {
        let mut state = LoudnessState::default();
        set_modes(&mut state, MeterMode::TruePeak, MeterMode::TruePeak);
        let mut snapshot = LoudnessSnapshot::with_floor(-99.9, 2);
        snapshot.true_peak_db[1] = -80.0;
        state.apply_snapshot((snapshot, Programme::default()));
        assert_eq!(state.value_label.content(), "-80.0 dBTP");
    }

    #[test]
    fn visible_bars_follow_fallback_channel_layouts() {
        let snapshot = |true_peak_db, channel_count| LoudnessSnapshot {
            elapsed: 0.0,
            short_term_loudness: -9.0,
            momentary_loudness: -9.0,
            rms_fast_db: [DB_RANGE.0; MAX_AUDIO_CHANNELS],
            rms_slow_db: [DB_RANGE.0; MAX_AUDIO_CHANNELS],
            true_peak_db,
            channel_count,
            positions: [ChannelPosition::Unknown; MAX_AUDIO_CHANNELS],
        };
        let mut state = LoudnessState::default();
        set_modes(&mut state, MeterMode::TruePeak, MeterMode::TruePeak);

        let mut mono = [DB_RANGE.0; MAX_AUDIO_CHANNELS];
        mono[0] = -12.0;
        state.apply_snapshot((snapshot(mono, 1), Programme::default()));
        assert_eq!(visible_bar_values(&state), [-12.0; 3]);

        let mut quad = [DB_RANGE.0; MAX_AUDIO_CHANNELS];
        quad[2] = -6.0;
        quad[3] = -3.0;
        state.apply_snapshot((snapshot(quad, 4), Programme::default()));
        assert_eq!(visible_bar_values(&state), [-6.0, -3.0, -3.0]);

        let mut surround = [DB_RANGE.0; MAX_AUDIO_CHANNELS];
        surround[3] = 6.0;
        state.apply_snapshot((snapshot(surround, 6), Programme::default()));
        assert_eq!(visible_bar_values(&state), [6.0; 3]);
    }

    #[test]
    fn peak_hold_restarts_before_decaying() {
        let mut peak = PeakHold::FLOOR;
        peak.update(-1.0, 0.0);
        peak.update(-1.0, 1.0);
        peak.update(-60.0, 2.0);
        assert_eq!(peak.db, -1.0);
        peak.update(-60.0, 0.5);
        assert!((peak.db + 31.0).abs() < 0.01);
    }
}
