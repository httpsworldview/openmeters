// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::palette::PaletteEvent;
use super::widgets::{
    FFT_OPTIONS, HOP_DIVISORS, SliderRange, get_closest_hop_divisor, pick, section, set_if_changed,
    slide, toggle, update_f32_range, update_fft_size, update_hop_divisor,
};
use crate::persistence::settings::{PianoRollOverlay, SpectrogramSettings};
use crate::util::audio::{FrequencyScale, WindowKind};
use crate::visuals::registry::VisualKind;
use iced::widget::{column, row};
use iced::{Element, Length};

const ZERO_PAD_OPTIONS: [usize; 6] = [1, 2, 4, 8, 16, 32];
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
        let left = controls!(8.0;
            pick("FFT size", &FFT_OPTIONS, s.fft_size, Message::FftSize);
            pick("Hop divisor", &HOP_DIVISORS, hop_divisor, Message::HopDivisor);
            pick(
                "Piano roll overlay", PianoRollOverlay::ALL, s.piano_roll_overlay,
                Message::PianoRoll
            );
        )
        .width(Length::Fill);
        let right = controls!(8.0;
            pick("Window", WindowKind::ALL, s.window, Message::Window);
            pick("Freq scale", FrequencyScale::ALL, s.frequency_scale, Message::FrequencyScale);
            pick("Zero pad", &ZERO_PAD_OPTIONS, s.zero_padding_factor, Message::ZeroPadding);
        )
        .width(Length::Fill);
        let tilt = if s.tilt_db == 0.0 {
            "Off".to_string()
        } else {
            format!("{:+.1} dB/dec", s.tilt_db)
        };
        let core = controls!(
            iced::widget::Column::new()
                .spacing(8.0)
                .push(row![left, right].spacing(10).width(Length::Fill));
            slider!("Floor", s.floor_db, FLOOR_DB_RANGE, Message::FloorDb, "{:.0} dB");
            slider!("Spectral tilt", s.tilt_db, TILT_DB_RANGE, Message::TiltDb, tilt);
            slider!(
                "Rotation", s.rotation as f32, ROTATION_RANGE, Message::Rotation,
                format!("{}\u{00b0}", s.rotation as i32 * 90)
            );
        );
        let advanced = controls!(8.0;
            toggle("Time-frequency reassignment", s.use_reassignment, Message::UseReassignment);
        );

        column![
            section("Core controls"),
            core,
            section("Advanced"),
            advanced,
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
