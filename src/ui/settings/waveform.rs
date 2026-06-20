// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::ui::widgets::palette_editor::PaletteEditor;
use super::widgets::{SliderRange, pick, set_if_changed, slide, update_f32_range};
use crate::persistence::settings::WaveformSettings;
use crate::util::audio::Channel;
use crate::visuals::options::{WaveformColorMode, WaveformHistoryMode};
use crate::visuals::registry::VisualKind;
use crate::visuals::waveform::processor::{
    MAX_BAND_DB_FLOOR, MAX_SCROLL_SPEED, MIN_BAND_DB_FLOOR, MIN_SCROLL_SPEED,
};
use iced::Element;

settings_pane!(
    WaveformSettingsPane, WaveformSettings, VisualKind::Waveform, Waveform,
    extra_from_settings(settings) {}
    init_palette(palette) {
        configure_palette_for_mode(palette, settings.color_mode);
    }
);

const SCROLL_SPEED_RANGE: SliderRange = SliderRange::new(MIN_SCROLL_SPEED, MAX_SCROLL_SPEED, 1.0);
const BAND_DB_FLOOR_RANGE: SliderRange =
    SliderRange::new(MIN_BAND_DB_FLOOR, MAX_BAND_DB_FLOOR, 1.0);

fn configure_palette_for_mode(palette: &mut PaletteEditor, mode: WaveformColorMode) {
    let (visible, labels) = match mode {
        WaveformColorMode::Static => (Some(vec![0]), vec![(0, "Color")]),
        WaveformColorMode::Loudness => (None, vec![(0, "Quiet"), (1, "->"), (2, "Loud")]),
        WaveformColorMode::Frequency => (None, Vec::new()),
    };
    palette.set_visible_indices(visible);
    palette.set_label_overrides(labels);
}

settings_messages!(WaveformSettingsPane as pane, value {
    ScrollSpeed(f32) => update_f32_range(&mut pane.settings.scroll_speed, value, SCROLL_SPEED_RANGE);
    BandDbFloor(f32) => update_f32_range(&mut pane.settings.band_db_floor, value, BAND_DB_FLOOR_RANGE);
    Channel1(Channel) => set_if_changed(&mut pane.settings.channel_1, value);
    Channel2(Channel) => set_if_changed(&mut pane.settings.channel_2, value);
    ColorMode(WaveformColorMode) => {
        let changed = set_if_changed(&mut pane.settings.color_mode, value);
        if changed { configure_palette_for_mode(&mut pane.palette, value); }
        changed
    };
    HistoryMode(WaveformHistoryMode) => set_if_changed(&mut pane.settings.history_mode, value);
});

impl WaveformSettingsPane {
    fn view(&self) -> Element<'_, Message> {
        let s = &self.settings;
        let mut controls = controls!(16.0;
            slider!("Scroll speed", s.scroll_speed, SCROLL_SPEED_RANGE, Message::ScrollSpeed, "{:.0} px/s");
            pick("Channel 1", Channel::ALL, s.channel_1, Message::Channel1);
            pick("Channel 2", Channel::ALL, s.channel_2, Message::Channel2);
            pick("Color mode", WaveformColorMode::ALL, s.color_mode, Message::ColorMode);
            pick("History", WaveformHistoryMode::ALL, s.history_mode, Message::HistoryMode);
        );
        if s.history_mode != WaveformHistoryMode::Off {
            controls = controls.push(slider!(
                "History floor",
                s.band_db_floor,
                BAND_DB_FLOOR_RANGE,
                Message::BandDbFloor,
                "{:.0} dB"
            ));
        }
        controls
            .push(super::palette_section(&self.palette, Message::Palette))
            .into()
    }
}
