// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{set, set_f32, set_usize};
use crate::persistence::settings::StereometerSettings;
use crate::ui::widgets::{SliderRange, pick, split, toggle};
use crate::visuals::options::{
    CorrelationMeterMode, CorrelationMeterSide, StereometerMode, StereometerScale,
};

const ROTATION_RANGE: SliderRange = SliderRange::new(-4.0, 4.0, 1.0);
const DURATION_RANGE: SliderRange = SliderRange::new(0.005, 0.2, 0.001);
const SAMPLE_COUNT_RANGE: SliderRange = SliderRange::new(100.0, 2000.0, 50.0);
const CORRELATION_RANGE: SliderRange = SliderRange::new(0.05, 1.0, 0.01);
const DOT_RANGE: SliderRange = SliderRange::new(0.5, 8.0, 0.1);

settings_pane!(StereometerSettings);

settings_messages!(pane, settings, value {
    SegmentDuration(f32) => set_f32(&mut settings.segment_duration, value, DURATION_RANGE);
    SampleCount(f32) => set_usize(&mut settings.target_sample_count, value, SAMPLE_COUNT_RANGE);
    CorrelationWindow(f32) => set_f32(&mut settings.correlation_window, value, CORRELATION_RANGE);
    DotRadius(f32) => set_f32(&mut settings.dot_radius, value, DOT_RANGE);
    Rotation(f32) => set(
        &mut settings.rotation,
        (value.round() as i8).clamp(ROTATION_RANGE.min as i8, ROTATION_RANGE.max as i8)
    );
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
        let mut meter = form!(
            pick("Mode", StereometerMode::ALL, settings.mode, Mode);
            slider!(
                "Segment duration", settings.segment_duration, DURATION_RANGE,
                SegmentDuration, format!("{:.1} ms", settings.segment_duration * 1000.0)
            );
            slider!(
                "Sample count", settings.target_sample_count as f32, SAMPLE_COUNT_RANGE,
                SampleCount, settings.target_sample_count.to_string()
            );
        );
        if dot_mode {
            meter = meter.push(pick("Scale", StereometerScale::ALL, settings.scale, Scale));
        }

        let mut display = form!(
            slider!(
                "Rotation", settings.rotation as f32, ROTATION_RANGE,
                Rotation, settings.rotation.to_string()
            );
        );
        if dot_mode {
            display = display
                .push(split(
                    toggle("Flip", settings.flip, Flip),
                    toggle("Unipolar", settings.unipolar, Unipolar),
                ))
                .push(slider!(
                    "Dot size", settings.dot_radius, DOT_RANGE, DotRadius, "{:.1}px"
                ));
        } else {
            display = display.push(toggle("Flip", settings.flip, Flip));
        }

        let mut correlation = form!(
            pick(
                "Meter", CorrelationMeterMode::ALL, settings.correlation_meter,
                CorrelationMeter
            );
        );
        if settings.correlation_meter != CorrelationMeterMode::Off {
            correlation = correlation
                .push(pick(
                    "Side", CorrelationMeterSide::ALL, settings.correlation_meter_side,
                    CorrelationSide,
                ))
                .push(slider!(
                    "Window", settings.correlation_window, CORRELATION_RANGE,
                    CorrelationWindow,
                    format!("{:.0} ms", settings.correlation_window * 1000.0)
                ));
        }
    }
    "Meter" => meter;
    "Display" => display;
    "Phase Correlation" => correlation;
}
