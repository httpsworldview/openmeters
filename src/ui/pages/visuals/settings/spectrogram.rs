// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::palette::PaletteEvent;
use super::widgets::{
    FFT_OPTIONS, FREQ_SCALE_OPTIONS, HOP_DIVISORS, SliderRange, get_closest_hop_divisor,
    labeled_pick_list, labeled_slider, labeled_toggler, section_title, set_if_changed,
    update_f32_range, update_fft_size, update_hop_divisor,
};
use crate::persistence::settings::{PianoRollOverlay, SpectrogramSettings};
use crate::visuals::registry::VisualKind;
use crate::visuals::spectrogram::processor::{FrequencyScale, WindowKind};
use iced::widget::{column, row};
use iced::{Element, Length};

const ZERO_PAD_OPTIONS: [usize; 6] = [1, 2, 4, 8, 16, 32];
const PIANO_ROLL_OVERLAY_OPTIONS: [PianoRollOverlay; 3] = [
    PianoRollOverlay::Off,
    PianoRollOverlay::Right,
    PianoRollOverlay::Left,
];
const FLOOR_DB_RANGE: SliderRange = SliderRange::new(-140.0, -1.0, 1.0);
const TILT_DB_RANGE: SliderRange = SliderRange::new(-6.0, 6.0, 0.5);
const ROTATION_RANGE: SliderRange = SliderRange::new(-1.0, 2.0, 1.0);
settings_pane!(
    SpectrogramSettingsPane, SpectrogramSettings, VisualKind::Spectrogram, Spectrogram,
    extra_from_settings(_s) {}
    init_palette(palette) {
        palette.set_show_ramp(true);
    }
);

#[derive(Debug, Clone, Copy)]
pub enum Message {
    FftSize(usize),
    HopDivisor(usize),
    Window(WindowKind),
    FrequencyScale(FrequencyScale),
    UseReassignment(bool),
    FloorDb(f32),
    TiltDb(f32),
    Rotation(f32),
    ZeroPadding(usize),
    PianoRoll(PianoRollOverlay),
    Palette(PaletteEvent),
}

impl SpectrogramSettingsPane {
    fn view(&self) -> Element<'_, Message> {
        let s = &self.settings;
        let hop_divisor = get_closest_hop_divisor(s.fft_size, s.hop_size);

        let left_col = column![
            labeled_pick_list("FFT size", &FFT_OPTIONS, Some(s.fft_size), Message::FftSize),
            labeled_pick_list(
                "Hop divisor",
                &HOP_DIVISORS,
                Some(hop_divisor),
                Message::HopDivisor
            ),
            labeled_pick_list(
                "Piano roll overlay",
                &PIANO_ROLL_OVERLAY_OPTIONS,
                Some(s.piano_roll_overlay),
                Message::PianoRoll
            ),
        ]
        .spacing(8)
        .width(Length::Fill);

        let right_col = column![
            labeled_pick_list("Window", &WindowKind::ALL, Some(s.window), Message::Window),
            labeled_pick_list(
                "Freq scale",
                &FREQ_SCALE_OPTIONS,
                Some(s.frequency_scale),
                Message::FrequencyScale
            ),
            labeled_pick_list(
                "Zero pad",
                &ZERO_PAD_OPTIONS,
                Some(s.zero_padding_factor),
                Message::ZeroPadding
            ),
        ]
        .spacing(8)
        .width(Length::Fill);

        let mut core =
            column![row![left_col, right_col].spacing(10).width(Length::Fill)].spacing(8);
        core = core
            .push(labeled_slider(
                "Floor",
                s.floor_db,
                format!("{:.0} dB", s.floor_db),
                FLOOR_DB_RANGE,
                Message::FloorDb,
            ))
            .push(labeled_slider(
                "Spectral tilt",
                s.tilt_db,
                if s.tilt_db == 0.0 {
                    "Off".to_string()
                } else {
                    format!("{:+.1} dB/dec", s.tilt_db)
                },
                TILT_DB_RANGE,
                Message::TiltDb,
            ))
            .push(labeled_slider(
                "Rotation",
                s.rotation as f32,
                format!("{}\u{00b0}", s.rotation as i32 * 90),
                ROTATION_RANGE,
                Message::Rotation,
            ));

        let adv = column![labeled_toggler(
            "Time-frequency reassignment",
            s.use_reassignment,
            Message::UseReassignment
        )]
        .spacing(8);

        column![
            section_title("Core controls"),
            core,
            section_title("Advanced"),
            adv,
            super::palette_section(&self.palette, Message::Palette)
        ]
        .spacing(16)
        .into()
    }

    fn handle(&mut self, msg: &Message) -> bool {
        let s = &mut self.settings;
        match *msg {
            Message::FftSize(size) => update_fft_size(&mut s.fft_size, &mut s.hop_size, size),
            Message::HopDivisor(div) => update_hop_divisor(s.fft_size, &mut s.hop_size, div),
            Message::Window(kind) => set_if_changed(&mut s.window, kind),
            Message::FrequencyScale(sc) => set_if_changed(&mut s.frequency_scale, sc),
            Message::UseReassignment(v) => set_if_changed(&mut s.use_reassignment, v),
            Message::FloorDb(v) => update_f32_range(&mut s.floor_db, v, FLOOR_DB_RANGE),
            Message::TiltDb(v) => update_f32_range(&mut s.tilt_db, v, TILT_DB_RANGE),
            Message::Rotation(v) => set_if_changed(&mut s.rotation, v.round() as i8),
            Message::ZeroPadding(v) => set_if_changed(&mut s.zero_padding_factor, v),
            Message::PianoRoll(opt) => set_if_changed(&mut s.piano_roll_overlay, opt),
            Message::Palette(e) => self.palette.update(e),
        }
    }
}
