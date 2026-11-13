// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{set, settings_messages, settings_pane, settings_view};
use crate::persistence::settings::WaveformSettings;
use crate::ui::widgets::{SliderRange, palette_editor::PaletteEditor, pick};
use crate::util::audio::Channel;
use crate::visuals::options::{WaveformColorMode, WaveformHistoryMode};

settings_pane!(WaveformSettings, init_palette(palette, settings) {
    configure_palette_for_mode(palette, settings.color_mode);
});

// Columns per second.
const SPEED_RANGE: SliderRange = SliderRange::new(10.0, 1000.0, 1.0);
const FLOOR_RANGE: SliderRange = SliderRange::new(-96.0, -12.0, 1.0);

fn configure_palette_for_mode(palette: &mut PaletteEditor, mode: WaveformColorMode) {
    palette.set_only_first_visible(mode == WaveformColorMode::Static);
    palette.set_label_overrides(match mode {
        WaveformColorMode::Static => &["Color"],
        WaveformColorMode::Loudness => &["Quiet", "->", "Loud"],
        WaveformColorMode::Frequency => &[],
    });
}

settings_messages!(pane, settings, value {
    ScrollSpeed(f32) => settings.scroll_speed.set(value);
    BandDbFloor(f32) => settings.band_db_floor.set(value);
    Channel1(Channel) => set(&mut settings.channel_1, value);
    Channel2(Channel) => set(&mut settings.channel_2, value);
    ColorMode(WaveformColorMode) => {
        let changed = set(&mut settings.color_mode, value);
        if changed {
            configure_palette_for_mode(&mut pane.palette, value);
        }
        changed
    };
    HistoryMode(WaveformHistoryMode) => set(&mut settings.history_mode, value);
});

settings_view! {
    pane as settings {}
    "Signal" => form!(
        pick("Channel 1", Channel::ALL, settings.channel_1, Channel1);
        pick("Channel 2", Channel::ALL, settings.channel_2, Channel2);
    );
    "Display" => form!(
        slider!("Scroll speed", settings.scroll_speed.get(), SPEED_RANGE, ScrollSpeed, "{:.0} px/s");
        pick("Color mode", WaveformColorMode::ALL, settings.color_mode, ColorMode);
        pick("History", WaveformHistoryMode::ALL, settings.history_mode, HistoryMode);
        (settings.history_mode != WaveformHistoryMode::Off).then(|| slider!(
            "History floor", settings.band_db_floor.get(), FLOOR_RANGE, BandDbFloor, "{:.0} dB"
        ));
    );
}
