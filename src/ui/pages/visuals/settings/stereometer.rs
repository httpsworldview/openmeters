// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::palette::PaletteEvent;
use super::widgets::{
    SliderRange, pick, section, set_if_changed, slide, toggle, update_f32_range,
    update_usize_from_f32,
};
use crate::persistence::settings::{
    CorrelationMeterMode, CorrelationMeterSide, StereometerMode, StereometerScale,
    StereometerSettings,
};
use crate::visuals::registry::VisualKind;
use iced::widget::{column, row};
use iced::{Element, Length};

const ROTATION_RANGE: SliderRange = SliderRange::new(-4.0, 4.0, 1.0);
const SCALE_RANGE: SliderRange = SliderRange::new(1.0, 30.0, 0.5);
const SEGMENT_DURATION_RANGE: SliderRange = SliderRange::new(0.005, 0.2, 0.001);
const TARGET_SAMPLE_COUNT_RANGE: SliderRange = SliderRange::new(100.0, 2000.0, 50.0);
const CORRELATION_WINDOW_RANGE: SliderRange = SliderRange::new(0.05, 1.0, 0.01);
const PERSISTENCE_RANGE: SliderRange = SliderRange::new(0.0, 1.0, 0.01);
const DOT_RADIUS_RANGE: SliderRange = SliderRange::new(0.5, 8.0, 0.1);

settings_pane!(
    StereometerSettingsPane,
    StereometerSettings,
    VisualKind::Stereometer,
    Stereometer
);

#[derive(Debug, Clone)]
pub enum Message {
    SegmentDuration(f32),
    TargetSampleCount(f32),
    CorrelationWindow(f32),
    Persistence(f32),
    DotRadius(f32),
    Rotation(f32),
    Flip(bool),
    Mode(StereometerMode),
    Scale(StereometerScale),
    ScaleRange(f32),
    CorrelationMeter(CorrelationMeterMode),
    CorrelationSide(CorrelationMeterSide),
    Palette(PaletteEvent),
}

impl StereometerSettingsPane {
    fn view(&self) -> Element<'_, Message> {
        use Message::*;
        let s = &self.settings;
        let picks = row![
            pick("Mode", StereometerMode::ALL, s.mode, Mode).width(Length::Fill),
            pick("Scale", StereometerScale::ALL, s.scale, Scale).width(Length::Fill),
        ]
        .spacing(16);
        let mut core = controls!(iced::widget::Column::new().spacing(8.0).push(picks);
            slider!(
                "Segment duration", s.segment_duration, SEGMENT_DURATION_RANGE, SegmentDuration,
                format!("{:.1} ms", s.segment_duration * 1000.0)
            );
            slider!(
                "Sample count", s.target_sample_count as f32, TARGET_SAMPLE_COUNT_RANGE,
                TargetSampleCount, s.target_sample_count.to_string()
            );
        );
        if s.scale == StereometerScale::Exponential {
            core = controls!(core;
                slider!("Scale range", s.scale_range, SCALE_RANGE, ScaleRange, "{:.1}");
            );
        }
        let mut display = controls!(8.0;
            slider!("Rotation", s.rotation as f32, ROTATION_RANGE, Rotation, s.rotation.to_string());
            slider!("Persistence", s.persistence, PERSISTENCE_RANGE, Persistence, "{:.2}");
            toggle("Flip", s.flip, Flip);
        );
        if s.mode == StereometerMode::DotCloud || s.mode == StereometerMode::DotCloudBands {
            display = controls!(display;
                slider!("Dot size", s.dot_radius, DOT_RADIUS_RANGE, DotRadius, "{:.1}px");
            );
        }

        let corr_active = s.correlation_meter != CorrelationMeterMode::Off;
        let mut corr_picks = row![
            pick(
                "Meter",
                CorrelationMeterMode::ALL,
                s.correlation_meter,
                CorrelationMeter,
            )
            .width(Length::Fill),
        ]
        .spacing(16);
        if corr_active {
            corr_picks = corr_picks.push(
                pick(
                    "Side",
                    CorrelationMeterSide::ALL,
                    s.correlation_meter_side,
                    CorrelationSide,
                )
                .width(Length::Fill),
            );
        }
        let mut correlation = column![corr_picks].spacing(8);
        if corr_active {
            correlation = controls!(correlation;
                slider!(
                    "Window", s.correlation_window, CORRELATION_WINDOW_RANGE, CorrelationWindow,
                    format!("{:.0} ms", s.correlation_window * 1000.0)
                );
            );
        }

        column![
            section("Core"),
            core,
            section("Display"),
            display,
            section("Correlation"),
            correlation,
            super::palette_section(&self.palette, Palette)
        ]
        .spacing(12)
        .into()
    }

    fn handle(&mut self, msg: &Message) -> bool {
        use Message::*;
        let s = &mut self.settings;
        match *msg {
            SegmentDuration(v) => {
                update_f32_range(&mut s.segment_duration, v, SEGMENT_DURATION_RANGE)
            }
            TargetSampleCount(v) => {
                update_usize_from_f32(&mut s.target_sample_count, v, TARGET_SAMPLE_COUNT_RANGE)
            }
            CorrelationWindow(v) => {
                update_f32_range(&mut s.correlation_window, v, CORRELATION_WINDOW_RANGE)
            }
            Persistence(v) => update_f32_range(&mut s.persistence, v, PERSISTENCE_RANGE),
            DotRadius(v) => update_f32_range(&mut s.dot_radius, v, DOT_RADIUS_RANGE),
            Rotation(v) => set_if_changed(
                &mut s.rotation,
                (v.round() as i8).clamp(ROTATION_RANGE.min as i8, ROTATION_RANGE.max as i8),
            ),
            Flip(v) => set_if_changed(&mut s.flip, v),
            Mode(m) => set_if_changed(&mut s.mode, m),
            Scale(sc) => set_if_changed(&mut s.scale, sc),
            ScaleRange(v) => update_f32_range(&mut s.scale_range, v, SCALE_RANGE),
            CorrelationMeter(m) => set_if_changed(&mut s.correlation_meter, m),
            CorrelationSide(side) => set_if_changed(&mut s.correlation_meter_side, side),
            Palette(e) => self.palette.update(e),
        }
    }
}
