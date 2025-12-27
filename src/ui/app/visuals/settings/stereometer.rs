use super::palette::{PaletteEditor, PaletteEvent};
use super::widgets::{SliderRange, labeled_pick_list, labeled_slider, set_f32, set_if_changed};
use super::{ModuleSettingsPane, SettingsMessage};
use crate::ui::settings::{
    CorrelationMeterMode, CorrelationMeterSide, SettingsHandle, StereometerMode, StereometerScale,
    StereometerSettings,
};
use crate::ui::theme;
use crate::ui::visualization::visual_manager::{VisualId, VisualKind, VisualManagerHandle};
use iced::widget::{column, row, toggler};
use iced::{Element, Length};

const MODE_OPTIONS: [StereometerMode; 2] = [StereometerMode::Lissajous, StereometerMode::DotCloud];
const SCALE_OPTIONS: [StereometerScale; 2] =
    [StereometerScale::Linear, StereometerScale::Exponential];
const CORRELATION_METER_OPTIONS: [CorrelationMeterMode; 3] = [
    CorrelationMeterMode::Off,
    CorrelationMeterMode::SingleBand,
    CorrelationMeterMode::MultiBand,
];
const CORRELATION_METER_SIDE_OPTIONS: [CorrelationMeterSide; 2] =
    [CorrelationMeterSide::Left, CorrelationMeterSide::Right];

const PALETTE_LABELS: [&str; 8] = [
    "Trace",
    "Corr background",
    "Corr center line",
    "Corr positive",
    "Corr negative",
    "Low band",
    "Mid band",
    "High band",
];

#[derive(Debug)]
pub struct StereometerSettingsPane {
    visual_id: VisualId,
    settings: StereometerSettings,
    palette: PaletteEditor,
}

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

pub fn create(
    visual_id: VisualId,
    visual_manager: &VisualManagerHandle,
) -> StereometerSettingsPane {
    let (settings, palette) = super::load_settings_and_palette(
        visual_manager,
        VisualKind::Stereometer,
        &theme::DEFAULT_STEREOMETER_PALETTE,
        &PALETTE_LABELS,
    );
    StereometerSettingsPane {
        visual_id,
        settings,
        palette,
    }
}

impl ModuleSettingsPane for StereometerSettingsPane {
    fn visual_id(&self) -> VisualId {
        self.visual_id
    }

    fn view(&self) -> Element<'_, SettingsMessage> {
        let s = &self.settings;
        let left = column![
            labeled_pick_list("Mode", &MODE_OPTIONS, Some(s.mode), |m| {
                SettingsMessage::Stereometer(Message::Mode(m))
            }),
            labeled_slider(
                "Rotation",
                s.rotation as f32,
                s.rotation.to_string(),
                SliderRange::new(-4.0, 4.0, 1.0),
                |v| SettingsMessage::Stereometer(Message::Rotation(v))
            ),
            toggler(s.flip)
                .label("Flip")
                .on_toggle(|v| { SettingsMessage::Stereometer(Message::Flip(v)) }),
        ]
        .spacing(16)
        .width(Length::Fill);

        let mut right = column![
            labeled_pick_list("Scale", &SCALE_OPTIONS, Some(s.scale), |v| {
                SettingsMessage::Stereometer(Message::Scale(v))
            }),
            labeled_pick_list(
                "Correlation meter",
                &CORRELATION_METER_OPTIONS,
                Some(s.correlation_meter),
                |v| SettingsMessage::Stereometer(Message::CorrelationMeter(v))
            ),
        ]
        .spacing(16)
        .width(Length::Fill);

        if s.correlation_meter != CorrelationMeterMode::Off {
            right = right.push(labeled_pick_list(
                "Correlation side",
                &CORRELATION_METER_SIDE_OPTIONS,
                Some(s.correlation_meter_side),
                |v| SettingsMessage::Stereometer(Message::CorrelationMeterSide(v)),
            ));
        }
        if s.scale == StereometerScale::Exponential {
            right = right.push(labeled_slider(
                "Scale range",
                s.scale_range,
                format!("{:.1}", s.scale_range),
                SliderRange::new(1.0, 30.0, 0.5),
                |v| SettingsMessage::Stereometer(Message::ScaleRange(v)),
            ));
        }

        column![
            row![left, right].spacing(24),
            labeled_slider(
                "Segment duration",
                s.segment_duration,
                format!("{:.1} ms", s.segment_duration * 1000.0),
                SliderRange::new(0.005, 0.2, 0.001),
                |v| SettingsMessage::Stereometer(Message::SegmentDuration(v))
            ),
            labeled_slider(
                "Sample count",
                s.target_sample_count as f32,
                s.target_sample_count.to_string(),
                SliderRange::new(100.0, 2000.0, 50.0),
                |v| SettingsMessage::Stereometer(Message::TargetSampleCount(v))
            ),
            labeled_slider(
                "Correlation window",
                s.correlation_window,
                format!("{:.0} ms", s.correlation_window * 1000.0),
                SliderRange::new(0.05, 1.0, 0.01),
                |v| SettingsMessage::Stereometer(Message::CorrelationWindow(v))
            ),
            labeled_slider(
                "Persistence",
                s.persistence,
                format!("{:.2}", s.persistence),
                SliderRange::new(0.0, 1.0, 0.01),
                |v| SettingsMessage::Stereometer(Message::Persistence(v))
            ),
            super::palette_section(
                &self.palette,
                Message::Palette,
                SettingsMessage::Stereometer
            )
        ]
        .spacing(16)
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
            Message::SegmentDuration(v) => set_f32(&mut s.segment_duration, v.clamp(0.005, 0.2)),
            Message::TargetSampleCount(v) => set_if_changed(
                &mut s.target_sample_count,
                (v.round() as usize).clamp(100, 2000),
            ),
            Message::CorrelationWindow(v) => set_f32(&mut s.correlation_window, v.clamp(0.05, 1.0)),
            Message::Persistence(v) => set_f32(&mut s.persistence, v.clamp(0.0, 1.0)),
            Message::Rotation(v) => set_if_changed(&mut s.rotation, (v.round() as i8).clamp(-4, 4)),
            Message::Flip(v) => set_if_changed(&mut s.flip, *v),
            Message::Mode(m) => set_if_changed(&mut s.mode, *m),
            Message::Scale(sc) => set_if_changed(&mut s.scale, *sc),
            Message::ScaleRange(v) => set_f32(&mut s.scale_range, v.clamp(1.0, 30.0)),
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
                theme::DEFAULT_STEREOMETER_PALETTE
            );
        }
    }
}
