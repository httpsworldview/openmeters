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

#[derive(Debug, Clone, Copy)]
pub enum Message {
    ScrollSpeed(f32),
    BandDbFloor(f32),
    Channel1(Channel),
    Channel2(Channel),
    ColorMode(WaveformColorMode),
    ShowPeakHistory(bool),
    Palette(PaletteEvent),
}

impl WaveformSettingsPane {
    fn view(&self) -> Element<'_, Message> {
        let s = &self.settings;
        controls!(16.0;
            slide(
                "Scroll speed", s.scroll_speed, format!("{:.0} px/s", s.scroll_speed),
                SCROLL_SPEED_RANGE, Message::ScrollSpeed
            );
            pick("Channel 1", Channel::ALL, s.channel_1, Message::Channel1);
            pick("Channel 2", Channel::ALL, s.channel_2, Message::Channel2);
            pick("Color mode", WaveformColorMode::ALL, s.color_mode, Message::ColorMode);
            toggle("Peak history", s.show_peak_history, Message::ShowPeakHistory);
            slide(
                "Peak range", s.band_db_floor, format!("{:.0} dB", s.band_db_floor),
                BAND_DB_FLOOR_RANGE, Message::BandDbFloor
            );
            super::palette_section(&self.palette, Message::Palette);
        )
        .into()
    }

    fn handle(&mut self, msg: &Message) -> bool {
        match *msg {
            Message::ScrollSpeed(v) => {
                update_f32_range(&mut self.settings.scroll_speed, v, SCROLL_SPEED_RANGE)
            }
            Message::BandDbFloor(v) => {
                update_f32_range(&mut self.settings.band_db_floor, v, BAND_DB_FLOOR_RANGE)
            }
            Message::Channel1(ch) => set_if_changed(&mut self.settings.channel_1, ch),
            Message::Channel2(ch) => set_if_changed(&mut self.settings.channel_2, ch),
            Message::ColorMode(m) => {
                let changed = set_if_changed(&mut self.settings.color_mode, m);
                if changed {
                    configure_palette_for_mode(&mut self.palette, m);
                }
                changed
            }
            Message::ShowPeakHistory(v) => set_if_changed(&mut self.settings.show_peak_history, v),
            Message::Palette(e) => self.palette.update(e),
        }
    }
}
