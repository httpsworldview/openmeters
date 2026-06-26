// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::widgets::{
    SliderRange, card, palette_card, pick, set_if_changed, slide, split, toggle, update_f32_range,
    update_usize_from_f32,
};
use crate::persistence::settings::StereometerSettings;
use crate::visuals::options::{
    CorrelationMeterMode, CorrelationMeterSide, StereometerMode, StereometerScale,
};
use iced::Element;

const ROTATION_RANGE: SliderRange = SliderRange::new(-4.0, 4.0, 1.0);
const SCALE_RANGE: SliderRange = SliderRange::new(1.0, 30.0, 0.5);
const SEGMENT_DURATION_RANGE: SliderRange = SliderRange::new(0.005, 0.2, 0.001);
const TARGET_SAMPLE_COUNT_RANGE: SliderRange = SliderRange::new(100.0, 2000.0, 50.0);
const CORRELATION_WINDOW_RANGE: SliderRange = SliderRange::new(0.05, 1.0, 0.01);
const PERSISTENCE_RANGE: SliderRange = SliderRange::new(0.0, 1.0, 0.01);
const DOT_RADIUS_RANGE: SliderRange = SliderRange::new(0.5, 8.0, 0.1);

settings_pane!(StereometerSettings);

settings_messages!(pane, value {
    SegmentDuration(f32) => update_f32_range(&mut pane.settings.segment_duration, value, SEGMENT_DURATION_RANGE);
    TargetSampleCount(f32) => update_usize_from_f32(&mut pane.settings.target_sample_count, value, TARGET_SAMPLE_COUNT_RANGE);
    CorrelationWindow(f32) => update_f32_range(&mut pane.settings.correlation_window, value, CORRELATION_WINDOW_RANGE);
    Persistence(f32) => update_f32_range(&mut pane.settings.persistence, value, PERSISTENCE_RANGE);
    DotRadius(f32) => update_f32_range(&mut pane.settings.dot_radius, value, DOT_RADIUS_RANGE);
    Rotation(f32) => set_if_changed(
        &mut pane.settings.rotation,
        (value.round() as i8).clamp(ROTATION_RANGE.min as i8, ROTATION_RANGE.max as i8),
    );
    Flip(bool) => set_if_changed(&mut pane.settings.flip, value);
    Mode(StereometerMode) => set_if_changed(&mut pane.settings.mode, value);
    Scale(StereometerScale) => set_if_changed(&mut pane.settings.scale, value);
    ScaleRange(f32) => update_f32_range(&mut pane.settings.scale_range, value, SCALE_RANGE);
    CorrelationMeter(CorrelationMeterMode) => set_if_changed(&mut pane.settings.correlation_meter, value);
    CorrelationSide(CorrelationMeterSide) => set_if_changed(&mut pane.settings.correlation_meter_side, value);
});

impl Pane {
    pub(super) fn view(&self) -> Element<'_, Message> {
        use Message::*;
        let s = &self.settings;
        let mut meter = controls!(8.0;
            split(
                pick("Mode", StereometerMode::ALL, s.mode, Mode),
                pick("Scale", StereometerScale::ALL, s.scale, Scale),
            );
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
            meter = controls!(meter;
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
        let mut correlation = controls!(8.0;
            pick("Meter", CorrelationMeterMode::ALL, s.correlation_meter, CorrelationMeter);
        );
        if corr_active {
            correlation = controls!(correlation;
                pick(
                    "Side",
                    CorrelationMeterSide::ALL,
                    s.correlation_meter_side,
                    CorrelationSide,
                );
                slider!(
                    "Window", s.correlation_window, CORRELATION_WINDOW_RANGE, CorrelationWindow,
                    format!("{:.0} ms", s.correlation_window * 1000.0)
                );
            );
        }

        controls!(12.0;
            card("Meter", meter);
            card("Display", display);
            card("Correlation", correlation);
            palette_card(&self.palette, Palette);
        )
        .into()
    }
}
