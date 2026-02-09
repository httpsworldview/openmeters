use super::SettingsMessage;
use super::palette::PaletteEvent;
use super::widgets::{
    SliderRange, labeled_pick_list, labeled_slider, section_title, set_if_changed,
    update_f32_range, update_usize_from_f32,
};
use crate::ui::settings::{
    CorrelationMeterMode, CorrelationMeterSide, SettingsHandle, StereometerMode, StereometerScale,
    StereometerSettings,
};
use crate::ui::theme;
use crate::ui::visualization::visual_manager::{VisualKind, VisualManagerHandle};
use iced::widget::{column, row, toggler};
use iced::{Element, Length};

const MODE_OPTIONS: [StereometerMode; 2] = [StereometerMode::Lissajous, StereometerMode::DotCloud];
const SCALE_OPTIONS: [StereometerScale; 2] =
    [StereometerScale::Linear, StereometerScale::Exponential];
const CORR_METER_OPTIONS: [CorrelationMeterMode; 3] = [
    CorrelationMeterMode::Off,
    CorrelationMeterMode::SingleBand,
    CorrelationMeterMode::MultiBand,
];
const CORR_SIDE_OPTIONS: [CorrelationMeterSide; 2] =
    [CorrelationMeterSide::Left, CorrelationMeterSide::Right];
const ROTATION_RANGE: SliderRange = SliderRange::new(-4.0, 4.0, 1.0);
const SCALE_RANGE: SliderRange = SliderRange::new(1.0, 30.0, 0.5);
const SEGMENT_DURATION_RANGE: SliderRange = SliderRange::new(0.005, 0.2, 0.001);
const TARGET_SAMPLE_COUNT_RANGE: SliderRange = SliderRange::new(100.0, 2000.0, 50.0);
const CORRELATION_WINDOW_RANGE: SliderRange = SliderRange::new(0.05, 1.0, 0.01);
const PERSISTENCE_RANGE: SliderRange = SliderRange::new(0.0, 1.0, 0.01);

settings_pane!(
    StereometerSettingsPane,
    StereometerSettings,
    VisualKind::Stereometer,
    theme::stereometer
);

#[derive(Debug, Clone)]
pub enum Message {
    SegmentDuration(f32),
    TargetSampleCount(f32),
    CorrelationWindow(f32),
    Persistence(f32),
    Rotation(f32),
    Flip(bool),
    Mode(StereometerMode),
    Scale(StereometerScale),
    ScaleRange(f32),
    CorrelationMeter(CorrelationMeterMode),
    CorrelationMeterSide(CorrelationMeterSide),
    Palette(PaletteEvent),
}

impl StereometerSettingsPane {
    fn view(&self) -> Element<'_, SettingsMessage> {
        use Message::*;
        let s = &self.settings;

        let picks = row![
            column![labeled_pick_list(
                "Mode",
                &MODE_OPTIONS,
                Some(s.mode),
                |m| { SettingsMessage::Stereometer(Mode(m)) }
            ),]
            .width(Length::Fill),
            column![labeled_pick_list(
                "Scale",
                &SCALE_OPTIONS,
                Some(s.scale),
                |v| { SettingsMessage::Stereometer(Scale(v)) }
            ),]
            .width(Length::Fill),
        ]
        .spacing(16);

        let mut core = column![
            picks,
            labeled_slider(
                "Segment duration",
                s.segment_duration,
                format!("{:.1} ms", s.segment_duration * 1000.0),
                SEGMENT_DURATION_RANGE,
                |v| SettingsMessage::Stereometer(SegmentDuration(v))
            ),
            labeled_slider(
                "Sample count",
                s.target_sample_count as f32,
                s.target_sample_count.to_string(),
                TARGET_SAMPLE_COUNT_RANGE,
                |v| SettingsMessage::Stereometer(TargetSampleCount(v))
            ),
        ]
        .spacing(8);

        if s.scale == StereometerScale::Exponential {
            core = core.push(labeled_slider(
                "Scale range",
                s.scale_range,
                format!("{:.1}", s.scale_range),
                SCALE_RANGE,
                |v| SettingsMessage::Stereometer(ScaleRange(v)),
            ));
        }

        let display = column![
            labeled_slider(
                "Rotation",
                s.rotation as f32,
                s.rotation.to_string(),
                ROTATION_RANGE,
                |v| SettingsMessage::Stereometer(Rotation(v))
            ),
            labeled_slider(
                "Persistence",
                s.persistence,
                format!("{:.2}", s.persistence),
                PERSISTENCE_RANGE,
                |v| SettingsMessage::Stereometer(Persistence(v))
            ),
            toggler(s.flip)
                .label("Flip")
                .spacing(4)
                .text_size(11)
                .on_toggle(|b| SettingsMessage::Stereometer(Flip(b))),
        ]
        .spacing(8);

        let mut corr_picks = row![
            column![labeled_pick_list(
                "Meter",
                &CORR_METER_OPTIONS,
                Some(s.correlation_meter),
                |v| SettingsMessage::Stereometer(CorrelationMeter(v))
            ),]
            .width(Length::Fill),
        ]
        .spacing(16);

        if s.correlation_meter != CorrelationMeterMode::Off {
            corr_picks = corr_picks.push(
                column![labeled_pick_list(
                    "Side",
                    &CORR_SIDE_OPTIONS,
                    Some(s.correlation_meter_side),
                    |v| SettingsMessage::Stereometer(CorrelationMeterSide(v)),
                ),]
                .width(Length::Fill),
            );
        }

        let mut correlation = column![corr_picks].spacing(8);

        if s.correlation_meter != CorrelationMeterMode::Off {
            correlation = correlation.push(labeled_slider(
                "Window",
                s.correlation_window,
                format!("{:.0} ms", s.correlation_window * 1000.0),
                CORRELATION_WINDOW_RANGE,
                |v| SettingsMessage::Stereometer(CorrelationWindow(v)),
            ));
        }

        column![
            section_title("Core"),
            core,
            section_title("Display"),
            display,
            section_title("Correlation"),
            correlation,
            super::palette_section(&self.palette, Palette, SettingsMessage::Stereometer)
        ]
        .spacing(12)
        .into()
    }

    fn handle(
        &mut self,
        message: &SettingsMessage,
        visual_manager: &VisualManagerHandle,
        settings_handle: &SettingsHandle,
    ) {
        let SettingsMessage::Stereometer(msg) = message else {
            return;
        };
        let s = &mut self.settings;
        let changed = match msg {
            Message::SegmentDuration(v) => {
                update_f32_range(&mut s.segment_duration, *v, SEGMENT_DURATION_RANGE)
            }
            Message::TargetSampleCount(v) => {
                update_usize_from_f32(&mut s.target_sample_count, *v, TARGET_SAMPLE_COUNT_RANGE)
            }
            Message::CorrelationWindow(v) => {
                update_f32_range(&mut s.correlation_window, *v, CORRELATION_WINDOW_RANGE)
            }
            Message::Persistence(v) => update_f32_range(&mut s.persistence, *v, PERSISTENCE_RANGE),
            Message::Rotation(v) => set_if_changed(&mut s.rotation, (v.round() as i8).clamp(-4, 4)),
            Message::Flip(v) => set_if_changed(&mut s.flip, *v),
            Message::Mode(m) => set_if_changed(&mut s.mode, *m),
            Message::Scale(sc) => set_if_changed(&mut s.scale, *sc),
            Message::ScaleRange(v) => update_f32_range(&mut s.scale_range, *v, SCALE_RANGE),
            Message::CorrelationMeter(m) => set_if_changed(&mut s.correlation_meter, *m),
            Message::CorrelationMeterSide(side) => {
                set_if_changed(&mut s.correlation_meter_side, *side)
            }
            Message::Palette(e) => self.palette.update(*e),
        };
        if changed {
            persist_palette!(
                visual_manager,
                settings_handle,
                VisualKind::Stereometer,
                self,
                &theme::stereometer::COLORS
            );
        }
    }
}
