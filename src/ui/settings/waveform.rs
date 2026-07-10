// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{set, set_f32};
use crate::persistence::settings::WaveformSettings;
use crate::ui::widgets::{SliderRange, palette_editor::PaletteEditor, pick};
use crate::util::audio::Channel;
use crate::visuals::options::{WaveformColorMode, WaveformHistoryMode};
use crate::visuals::waveform::processor::{
    MAX_BAND_DB_FLOOR, MAX_SCROLL_SPEED, MIN_BAND_DB_FLOOR, MIN_SCROLL_SPEED,
};

settings_pane!(WaveformSettings, init_palette(palette, settings) {
    configure_palette_for_mode(palette, settings.color_mode);
});

const SPEED_RANGE: SliderRange = SliderRange::new(MIN_SCROLL_SPEED, MAX_SCROLL_SPEED, 1.0);
const FLOOR_RANGE: SliderRange =
    SliderRange::new(MIN_BAND_DB_FLOOR, MAX_BAND_DB_FLOOR, 1.0);

fn configure_palette_for_mode(palette: &mut PaletteEditor, mode: WaveformColorMode) {
    palette.set_visible_indices((mode == WaveformColorMode::Static).then_some(&[0][..]));
    palette.set_label_overrides(match mode {
        WaveformColorMode::Static => &[(0, "Color")],
        WaveformColorMode::Loudness => &[(0, "Quiet"), (1, "->"), (2, "Loud")],
        WaveformColorMode::Frequency => &[],
    });
}

settings_messages!(pane, settings, value {
    ScrollSpeed(f32) => set_f32(&mut settings.scroll_speed, value, SPEED_RANGE);
    BandDbFloor(f32) => set_f32(&mut settings.band_db_floor, value, FLOOR_RANGE);
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
    pane as settings {
        let mut display = form!(
            slider!("Scroll speed", settings.scroll_speed, SPEED_RANGE, ScrollSpeed, "{:.0} px/s");
            pick("Color mode", WaveformColorMode::ALL, settings.color_mode, ColorMode);
            pick("History", WaveformHistoryMode::ALL, settings.history_mode, HistoryMode);
        );
        if settings.history_mode != WaveformHistoryMode::Off {
            display = display.push(slider!(
                "History floor", settings.band_db_floor, FLOOR_RANGE,
                BandDbFloor, "{:.0} dB"
            ));
        }
    }
    "Signal" => form!(
        pick("Channel 1", Channel::ALL, settings.channel_1, Channel1);
        pick("Channel 2", Channel::ALL, settings.channel_2, Channel2);
    );
    "Display" => display;
}
