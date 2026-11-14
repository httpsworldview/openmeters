// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{fmt_ms, set, settings_messages, settings_pane, settings_view};
use crate::persistence::settings::StereometerSettings;
use crate::ui::widgets::{SliderRange, chip, pick, split, toggle};
use crate::visuals::options::{
    self, CorrelationMeterMode, CorrelationMeterSide, StereometerMode, StereometerScale,
};

const ROTATION_RANGE: SliderRange = SliderRange::new(
    options::EighthTurns::MIN as f32,
    options::EighthTurns::MAX as f32,
    1.0,
);
const DURATION_RANGE: SliderRange = SliderRange::new(0.005, 0.2, 0.001);
const SAMPLE_COUNT_RANGE: SliderRange = SliderRange::new(100.0, 2000.0, 50.0);
const CORRELATION_RANGE: SliderRange = SliderRange::new(0.05, 1.0, 0.01);
const DOT_RANGE: SliderRange =
    SliderRange::new(options::DotRadius::MIN, options::DotRadius::MAX, 0.1);

settings_pane!(StereometerSettings);

settings_messages!(pane, settings, value {
    SegmentDuration(f32) => settings.segment_duration.set(value);
    SampleCount(usize) => set(&mut settings.target_sample_count, value);
    CorrelationWindow(f32) => settings.correlation_window.set(value);
    DotRadius(f32) => settings.dot_radius.set(value);
    Rotation(i8) => settings.rotation.set(value);
    Flip(bool) => set(&mut settings.flip, value);
    Unipolar(bool) => set(&mut settings.unipolar, value);
    Mode(StereometerMode) => set(&mut settings.mode, value);
    Scale(StereometerScale) => set(&mut settings.scale, value);
    CorrelationMeter(CorrelationMeterMode) => set(&mut settings.correlation_meter, value);
    CorrelationSide(CorrelationMeterSide) => set(&mut settings.correlation_meter_side, value);
});

settings_view! {
    pane as settings {
        let dot_mode = settings.mode != StereometerMode::Lissajous;
        let meter = form!(
            pick("Mode", StereometerMode::ALL, settings.mode, Mode);
            slider!(
                "Segment duration", settings.segment_duration.get(), DURATION_RANGE,
                SegmentDuration, fmt_ms(settings.segment_duration.get(), 1)
            );
            slider!(
                "Sample count", settings.target_sample_count as f32, SAMPLE_COUNT_RANGE,
                |value| SampleCount(value.round() as usize),
                settings.target_sample_count.to_string()
            );
            dot_mode.then(|| pick("Scale", StereometerScale::ALL, settings.scale, Scale));
        );

        let display = form!(
            slider!(
                "Rotation", f32::from(settings.rotation.get()), ROTATION_RANGE,
                |value| Rotation(value.round() as i8), settings.rotation.get().to_string()
            );
            dot_mode.then(|| split(
                toggle("Flip", settings.flip, Flip),
                toggle("Unipolar", settings.unipolar, Unipolar),
            ));
            (!dot_mode).then(|| toggle("Flip", settings.flip, Flip));
            dot_mode.then(|| slider!(
                "Dot size", settings.dot_radius.get(), DOT_RANGE, DotRadius, "{:.1}px"
            ));
        );

        let metered = settings.correlation_meter != CorrelationMeterMode::Off;
        let correlation = form!(
            pick(
                "Meter", CorrelationMeterMode::ALL, settings.correlation_meter,
                CorrelationMeter
            );
            metered.then(|| pick(
                "Side", CorrelationMeterSide::ALL, settings.correlation_meter_side,
                CorrelationSide,
            ));
            metered.then(|| slider!(
                "Window", settings.correlation_window.get(), CORRELATION_RANGE,
                CorrelationWindow, fmt_ms(settings.correlation_window.get(), 0)
            ));
        );
    }
    "Meter" => meter;
    "Display" => display;
    "Phase Correlation" [[chip(settings.correlation_meter.label())]] => correlation;
}
