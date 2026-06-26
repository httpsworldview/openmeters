// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::widgets::{
    FFT_OPTIONS, HOP_DIVISORS, SliderRange, card, get_closest_hop_divisor, palette_card, pick,
    set_if_changed, slide, split, toggle, update_f32_range, update_fft_size, update_hop_divisor,
};
use crate::persistence::settings::SpectrogramSettings;
use crate::util::audio::{FrequencyScale, WindowKind};
use crate::visuals::options::PianoRollOverlay;
use iced::Element;

const ZERO_PAD_OPTIONS: [usize; 6] = [1, 2, 4, 8, 16, 32];
const FLOOR_DB_RANGE: SliderRange = SliderRange::new(-140.0, -1.0, 1.0);
const TILT_DB_RANGE: SliderRange = SliderRange::new(-6.0, 6.0, 0.5);
const ROTATION_RANGE: SliderRange = SliderRange::new(-1.0, 2.0, 1.0);

settings_pane!(SpectrogramSettings, init_palette(palette) {
    palette.set_show_ramp(true);
});

settings_messages!(pane, value {
    FftSize(usize) => update_fft_size(&mut pane.settings.fft_size, &mut pane.settings.hop_size, value);
    HopDivisor(usize) => update_hop_divisor(pane.settings.fft_size, &mut pane.settings.hop_size, value);
    Window(WindowKind) => set_if_changed(&mut pane.settings.window, value);
    FrequencyScale(FrequencyScale) => set_if_changed(&mut pane.settings.frequency_scale, value);
    UseReassignment(bool) => set_if_changed(&mut pane.settings.use_reassignment, value);
    FloorDb(f32) => update_f32_range(&mut pane.settings.floor_db, value, FLOOR_DB_RANGE);
    TiltDb(f32) => update_f32_range(&mut pane.settings.tilt_db, value, TILT_DB_RANGE);
    Rotation(f32) => set_if_changed(
        &mut pane.settings.rotation,
        ROTATION_RANGE.snap(value).round() as i8,
    );
    ZeroPadding(usize) => set_if_changed(&mut pane.settings.zero_padding_factor, value);
    PianoRoll(PianoRollOverlay) => set_if_changed(&mut pane.settings.piano_roll_overlay, value);
});

impl Pane {
    pub(super) fn view(&self) -> Element<'_, Message> {
        let s = &self.settings;
        let hop_divisor = get_closest_hop_divisor(s.fft_size, s.hop_size);
        let tilt = if s.tilt_db == 0.0 {
            "Off".to_string()
        } else {
            format!("{:+.1} dB/oct", s.tilt_db)
        };

        let analysis = split(
            controls!(8.0;
                pick("FFT size", &FFT_OPTIONS, s.fft_size, Message::FftSize);
                pick("Hop divisor", &HOP_DIVISORS, hop_divisor, Message::HopDivisor);
                pick("Window", WindowKind::ALL, s.window, Message::Window);
            ),
            controls!(8.0;
                pick("Zero pad", &ZERO_PAD_OPTIONS, s.zero_padding_factor, Message::ZeroPadding);
                toggle("Time-frequency reassignment", s.use_reassignment, Message::UseReassignment);
            ),
        );
        let display = controls!(8.0;
            pick("Frequency scale", FrequencyScale::ALL, s.frequency_scale, Message::FrequencyScale);
            pick(
                "Piano roll overlay", PianoRollOverlay::ALL, s.piano_roll_overlay,
                Message::PianoRoll
            );
            slider!("Floor", s.floor_db, FLOOR_DB_RANGE, Message::FloorDb, "{:.0} dB");
            slider!("Spectral tilt", s.tilt_db, TILT_DB_RANGE, Message::TiltDb, tilt);
            slider!(
                "Rotation", s.rotation as f32, ROTATION_RANGE, Message::Rotation,
                format!("{}\u{00b0}", s.rotation as i32 * 90)
            );
        );

        controls!(12.0;
            card("Analysis", analysis);
            card("Display", display);
            palette_card(&self.palette, Message::Palette);
        )
        .into()
    }
}
