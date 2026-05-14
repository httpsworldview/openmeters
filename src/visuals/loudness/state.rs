// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

// Note: This processor intentionally diverges from project patterns by
// omitting `config()` and `update_config()` methods. this is because
// loudness settings are not user-configurable
use super::processor::{
    LoudnessConfig, LoudnessProcessor as CoreLoudnessProcessor, LoudnessSnapshot, MAX_CHANNELS,
};
use super::render::{LoudnessParams, LoudnessPrimitive, MeterBar};
use crate::persistence::settings::{LoudnessSettings, MeterMode};
use crate::util::color::{color_to_rgba, with_alpha};
use crate::visuals::palettes;
use crate::visuals::render::common::{fill_rect, make_text};
use crate::{vis_processor, visualization_widget};
use iced::advanced::text;
use iced::alignment::{Horizontal, Vertical};
use iced::{Color, Point, Rectangle, Size};
const DEFAULT_RANGE: (f32, f32) = (-60.0, 4.0);
const GUIDE_LEVELS: [f32; 6] = [0.0, -6.0, -12.0, -18.0, -24.0, -36.0];
const LEFT_PADDING: f32 = 28.0;
const RIGHT_PADDING: f32 = 64.0;
const LABEL_FONT_SIZE: f32 = 10.0;
const VALUE_FONT_SIZE: f32 = 12.0;

// Standard channel map assumptions for stereo/surround downmix
// 0: FL, 1: FR, 2: FC, 3: LFE, 4: BL, 5: BR, 6: SL, 7: SR
const LEFT_CHANNEL_INDICES: &[usize] = &[0, 4, 6];
const RIGHT_CHANNEL_INDICES: &[usize] = &[1, 5, 7];
const CENTER_CHANNEL_INDEX: usize = 2;

vis_processor!(
    LoudnessProcessor,
    CoreLoudnessProcessor,
    LoudnessConfig,
    LoudnessSnapshot,
    no_config
);

pub const LOUDNESS_PALETTE_SIZE: usize = 5;

#[derive(Debug, Clone)]
pub(crate) struct LoudnessState {
    short_term_loudness: f32,
    momentary_loudness: f32,
    rms_fast_db: [f32; MAX_CHANNELS],
    rms_slow_db: [f32; MAX_CHANNELS],
    true_peak_db: [f32; MAX_CHANNELS],
    channel_count: usize,
    range: (f32, f32),
    pub(crate) left_mode: MeterMode,
    pub(crate) right_mode: MeterMode,
    pub(crate) palette: [Color; LOUDNESS_PALETTE_SIZE],
    key: u64,
}

impl LoudnessState {
    pub fn new() -> Self {
        let defaults = LoudnessSettings::default();
        Self {
            short_term_loudness: DEFAULT_RANGE.0,
            momentary_loudness: DEFAULT_RANGE.0,
            rms_fast_db: [DEFAULT_RANGE.0; MAX_CHANNELS],
            rms_slow_db: [DEFAULT_RANGE.0; MAX_CHANNELS],
            true_peak_db: [DEFAULT_RANGE.0; MAX_CHANNELS],
            channel_count: 2,
            range: DEFAULT_RANGE,
            left_mode: defaults.left_mode,
            right_mode: defaults.right_mode,
            palette: palettes::loudness::COLORS,
            key: crate::visuals::next_key(),
        }
    }

    pub fn apply_snapshot(&mut self, snapshot: LoudnessSnapshot) {
        self.short_term_loudness = snapshot.short_term_loudness;
        self.momentary_loudness = snapshot.momentary_loudness;
        self.channel_count = snapshot.channel_count.max(1);
        for i in 0..self.channel_count.min(MAX_CHANNELS) {
            self.rms_fast_db[i] = snapshot.rms_fast_db[i];
            self.rms_slow_db[i] = snapshot.rms_slow_db[i];
            self.true_peak_db[i] = snapshot.true_peak_db[i];
        }
    }

    pub fn set_palette(&mut self, palette: &[Color; LOUDNESS_PALETTE_SIZE]) {
        self.palette = *palette;
    }

    #[cfg(test)]
    pub fn short_term_average(&self) -> f32 {
        self.short_term_loudness
    }

    fn get_value(&self, mode: MeterMode, channel: usize) -> f32 {
        let per_channel =
            |buf: &[f32; MAX_CHANNELS]| buf.get(channel).copied().unwrap_or(self.range.0);
        match mode {
            MeterMode::LufsShortTerm => self.short_term_loudness,
            MeterMode::LufsMomentary => self.momentary_loudness,
            MeterMode::RmsFast => per_channel(&self.rms_fast_db),
            MeterMode::RmsSlow => per_channel(&self.rms_slow_db),
            MeterMode::TruePeak => per_channel(&self.true_peak_db),
        }
    }

    fn visual_params(&self, bounds: Rectangle) -> LoudnessParams {
        let (min, max) = self.range;
        let guide_color = color_to_rgba(self.palette[4]);
        let bg_color = color_to_rgba(with_alpha(self.palette[0], 1.0));

        // Stereo L/R display with surround aggregation (ITU-R BS.775 layout)
        let left_value = self.aggregate_channels(self.left_mode, LEFT_CHANNEL_INDICES);
        let right_value = self.aggregate_channels(self.left_mode, RIGHT_CHANNEL_INDICES);

        LoudnessParams {
            key: self.key,
            bounds,
            min_db: min,
            max_db: max,
            bars: vec![
                MeterBar {
                    bg_color,
                    fills: vec![
                        (left_value, color_to_rgba(self.palette[1])),
                        (right_value, color_to_rgba(self.palette[2])),
                    ],
                },
                MeterBar {
                    bg_color,
                    fills: vec![(
                        self.get_value(self.right_mode, 0),
                        color_to_rgba(self.palette[3]),
                    )],
                },
            ],
            guides: GUIDE_LEVELS
                .iter()
                .filter(|&&l| l >= min && l <= max)
                .copied()
                .collect(),
            guide_color,
            threshold_db: Some(0.0),
            left_padding: LEFT_PADDING,
            right_padding: RIGHT_PADDING,
        }
    }

    fn aggregate_channels(&self, mode: MeterMode, indices: &[usize]) -> f32 {
        if matches!(mode, MeterMode::LufsShortTerm | MeterMode::LufsMomentary) {
            return self.get_value(mode, 0);
        }
        let mut max_val = self.range.0;
        for &ch in indices {
            if ch < self.channel_count {
                max_val = max_val.max(self.get_value(mode, ch));
            }
        }
        if self.channel_count > CENTER_CHANNEL_INDEX {
            max_val = max_val.max(self.get_value(mode, CENTER_CHANNEL_INDEX));
        }
        max_val
    }
}

visualization_widget!(Loudness, LoudnessState, |this, renderer, theme, bounds| {
    let state = this.state.borrow();
    let params = state.visual_params(bounds);

    renderer.draw_primitive(bounds, LoudnessPrimitive::new(params.clone()));

    let palette = theme.extended_palette();
    let label_color = state.palette[4];

    if let Some((meter_x, bar_width, stride)) = params.meter_bounds() {
        let y_of = |db| bounds.y + bounds.height * (1.0 - params.db_to_ratio(db));

        for &db in &params.guides {
            let y = y_of(db);
            let label = format!("{:.0}", db.abs());

            let mut text = make_text(&label, LABEL_FONT_SIZE, Size::new(LEFT_PADDING, 20.0));
            text.align_x = Horizontal::Right.into();
            text.align_y = Vertical::Center;
            text::Renderer::fill_text(
                renderer,
                text,
                Point::new(bounds.x + LEFT_PADDING - 4.0, y),
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

        fill_rect(renderer, label_rect, with_alpha(state.palette[0], 1.0));

        let mut text = make_text(
            &label,
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

    #[test]
    fn state_aggregates_channels() {
        let mut state = LoudnessState::new();
        state.apply_snapshot(LoudnessSnapshot {
            short_term_loudness: -9.0,
            momentary_loudness: -7.5,
            rms_fast_db: [-15.0, -9.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            rms_slow_db: [-14.0, -8.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            true_peak_db: [-1.0, -3.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            channel_count: 2,
        });

        assert!((state.short_term_average() + 9.0).abs() < f32::EPSILON);
        assert_eq!(state.true_peak_db[0], -1.0);
        assert_eq!(state.true_peak_db[1], -3.0);

        let params = state.visual_params(Rectangle::new(Point::ORIGIN, Size::new(200.0, 100.0)));
        assert_eq!(params.bars.len(), 2);
        assert_eq!(params.bars[0].fills.len(), 2);
        assert_eq!(params.bars[1].fills.len(), 1);
    }
}
