// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{
    DB_FLOOR_RANGE, FFT_OPTIONS, HOP_DIVISORS, get_closest_hop_divisor, quarter_turn_slider, set,
    settings_messages, settings_pane, settings_view, update_fft_size, update_hop_divisor,
};
use crate::persistence::settings::SpectrogramSettings;
use crate::ui::widgets::{SliderRange, chip, pick, split, toggle};
use crate::util::audio::{Channel, FrequencyScale, WindowKind};
use crate::visuals::options::PianoRollOverlay;

const ZERO_PAD_OPTIONS: [usize; 6] = [1, 2, 4, 8, 16, 32];
const TILT_RANGE: SliderRange = SliderRange::new(-6.0, 6.0, 0.5);

settings_pane!(SpectrogramSettings);

settings_messages!(pane, settings, value {
    FftSize(usize) => update_fft_size(&mut settings.fft_size, &mut settings.hop_size, value);
    HopDivisor(usize) => update_hop_divisor(settings.fft_size, &mut settings.hop_size, value);
    Window(WindowKind) => set(&mut settings.window, value);
    Source(Channel) => set(&mut settings.source, value);
    Scale(FrequencyScale) => set(&mut settings.frequency_scale, value);
    UseReassignment(bool) => set(&mut settings.use_reassignment, value);
    FloorDb(f32) => settings.floor_db.set(value);
    TiltDb(f32) => settings.tilt_db.set(value);
    Rotation(i8) => set(&mut settings.rotation, value);
    ZeroPadding(usize) => set(&mut settings.zero_padding_factor, value);
    PianoRoll(PianoRollOverlay) => set(&mut settings.piano_roll_overlay, value);
});

settings_view! {
    pane as settings {
        let hop_divisor = get_closest_hop_divisor(settings.fft_size.get(), settings.hop_size.get());
        let tilt_db = settings.tilt_db.get();
        let tilt = if tilt_db == 0.0 { "Off".to_string() } else { format!("{tilt_db:+.1} dB/oct") };
    }
    "Display" => form!(
        pick("Source", Channel::ALL, settings.source, Source);
        pick("Frequency scale", FrequencyScale::ALL, settings.frequency_scale, Scale);
        pick(
            "Piano roll overlay", PianoRollOverlay::ALL,
            settings.piano_roll_overlay, PianoRoll
        );
        slider!("Floor", settings.floor_db.get(), DB_FLOOR_RANGE, FloorDb, "{:.0} dB");
        slider!("Spectral tilt", tilt_db, TILT_RANGE, TiltDb, tilt);
        quarter_turn_slider(settings.rotation, Rotation);
    );
    "Analysis" [[
        Some(chip(format!("FFT {}", settings.fft_size.get()))),
        Some(chip(settings.window.label())),
        settings.use_reassignment.then(|| chip("Reassign")),
    ].into_iter().flatten()] => split(
        form!(
            pick("FFT size", &FFT_OPTIONS[..], settings.fft_size.get(), FftSize);
            pick("Hop divisor", &HOP_DIVISORS[..], hop_divisor, HopDivisor);
            pick("Window", WindowKind::ALL, settings.window, Window);
        ),
        form!(
            pick("Zero pad", &ZERO_PAD_OPTIONS[..], settings.zero_padding_factor, ZeroPadding);
            toggle("Time-frequency reassignment", settings.use_reassignment, UseReassignment);
        ),
    );
}
