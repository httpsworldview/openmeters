// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::palette::{PaletteEditor, PaletteEvent};
use super::widgets::{SliderRange, pick, set_if_changed, slide, toggle, update_f32_range};
use crate::persistence::settings::{Channel, WaveformColorMode, WaveformSettings};
use crate::visuals::palettes::waveform::GRADIENT_STOPS;
use crate::visuals::registry::VisualKind;
use crate::visuals::waveform::processor::{
    MAX_BAND_DB_FLOOR, MAX_SCROLL_SPEED, MIN_BAND_DB_FLOOR, MIN_SCROLL_SPEED, NUM_BANDS,
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
        WaveformColorMode::Static => (
            Some(
                std::iter::once(0)
                    .chain(GRADIENT_STOPS..GRADIENT_STOPS + NUM_BANDS)
                    .collect(),
            ),
            vec![(0, "Color")],
        ),
        WaveformColorMode::Loudness => (None, vec![(0, "Quiet"), (5, "Loud")]),
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
    ShowPeakHistory(bool) => set_if_changed(&mut pane.settings.show_peak_history, value);
    Palette(PaletteEvent) => pane.palette.update(value);
});

impl WaveformSettingsPane {
    fn view(&self) -> Element<'_, Message> {
        let s = &self.settings;
        controls!(16.0;
            slider!("Scroll speed", s.scroll_speed, SCROLL_SPEED_RANGE, Message::ScrollSpeed, "{:.0} px/s");
            pick("Channel 1", Channel::ALL, s.channel_1, Message::Channel1);
            pick("Channel 2", Channel::ALL, s.channel_2, Message::Channel2);
            pick("Color mode", WaveformColorMode::ALL, s.color_mode, Message::ColorMode);
            toggle("Peak history", s.show_peak_history, Message::ShowPeakHistory);
            slider!("Peak range", s.band_db_floor, BAND_DB_FLOOR_RANGE, Message::BandDbFloor, "{:.0} dB");
            super::palette_section(&self.palette, Message::Palette);
        )
        .into()
    }
}
