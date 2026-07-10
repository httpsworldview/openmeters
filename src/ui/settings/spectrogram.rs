// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{
    FFT_OPTIONS, HOP_DIVISORS, get_closest_hop_divisor, set, set_f32, update_fft_size,
    update_hop_divisor,
};
use crate::persistence::settings::SpectrogramSettings;
use crate::ui::widgets::{SliderRange, pick, split, toggle};
use crate::util::audio::{FrequencyScale, WindowKind};
use crate::visuals::options::PianoRollOverlay;

const ZERO_PAD_OPTIONS: [usize; 6] = [1, 2, 4, 8, 16, 32];
const FLOOR_RANGE: SliderRange = SliderRange::new(-140.0, -1.0, 1.0);
const TILT_RANGE: SliderRange = SliderRange::new(-6.0, 6.0, 0.5);
const ROTATION_RANGE: SliderRange = SliderRange::new(-1.0, 2.0, 1.0);

settings_pane!(SpectrogramSettings, init_palette(palette) {
    palette.set_show_ramp(true);
});

settings_messages!(pane, settings, value {
    FftSize(usize) => update_fft_size(&mut settings.fft_size, &mut settings.hop_size, value);
    HopDivisor(usize) => update_hop_divisor(settings.fft_size, &mut settings.hop_size, value);
    Window(WindowKind) => set(&mut settings.window, value);
    Scale(FrequencyScale) => set(&mut settings.frequency_scale, value);
    UseReassignment(bool) => set(&mut settings.use_reassignment, value);
    FloorDb(f32) => set_f32(&mut settings.floor_db, value, FLOOR_RANGE);
    TiltDb(f32) => set_f32(&mut settings.tilt_db, value, TILT_RANGE);
    Rotation(f32) => set(&mut settings.rotation, ROTATION_RANGE.snap(value).round() as i8);
    ZeroPadding(usize) => set(&mut settings.zero_padding_factor, value);
    PianoRoll(PianoRollOverlay) => set(&mut settings.piano_roll_overlay, value);
});

settings_view! {
    pane as settings {
        let hop_divisor = get_closest_hop_divisor(settings.fft_size, settings.hop_size);
        let tilt_db = settings.tilt_db;
        let tilt = if tilt_db == 0.0 { "Off".to_string() } else { format!("{tilt_db:+.1} dB/oct") };
    }
    "Analysis" => split(
        form!(
            pick("FFT size", &FFT_OPTIONS[..], settings.fft_size, FftSize);
            pick("Hop divisor", &HOP_DIVISORS[..], hop_divisor, HopDivisor);
            pick("Window", WindowKind::ALL, settings.window, Window);
        ),
        form!(
            pick("Zero pad", &ZERO_PAD_OPTIONS[..], settings.zero_padding_factor, ZeroPadding);
            toggle("Time-frequency reassignment", settings.use_reassignment, UseReassignment);
        ),
    );
    "Display" => form!(
        pick("Frequency scale", FrequencyScale::ALL, settings.frequency_scale, Scale);
        pick(
            "Piano roll overlay", PianoRollOverlay::ALL,
            settings.piano_roll_overlay, PianoRoll
        );
        slider!("Floor", settings.floor_db, FLOOR_RANGE, FloorDb, "{:.0} dB");
        slider!("Spectral tilt", tilt_db, TILT_RANGE, TiltDb, tilt);
        slider!(
            "Rotation", settings.rotation as f32, ROTATION_RANGE, Rotation,
            format!("{}\u{00b0}", settings.rotation as i32 * 90)
        );
    );
}
