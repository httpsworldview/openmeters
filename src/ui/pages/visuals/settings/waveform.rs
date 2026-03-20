// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::palette::{PaletteEditor, PaletteEvent};
use super::widgets::{
    SliderRange, labeled_pick_list, labeled_slider, labeled_toggler, set_if_changed,
    update_f32_range,
};
use crate::persistence::settings::{Channel, WaveformColorMode, WaveformSettings};
use crate::ui::theme;
use crate::visuals::registry::VisualKind;
use crate::visuals::waveform::processor::{
    MAX_BAND_DB_FLOOR, MAX_SCROLL_SPEED, MIN_BAND_DB_FLOOR, MIN_SCROLL_SPEED,
};
use iced::Element;
use iced::widget::column;

settings_pane!(
    WaveformSettingsPane, WaveformSettings, VisualKind::Waveform, theme::waveform, Waveform,
    extra_from_settings(settings) {}
    init_palette(palette) {
        configure_palette_for_mode(palette, settings.color_mode);
    }
);

const SCROLL_SPEED_RANGE: SliderRange = SliderRange::new(MIN_SCROLL_SPEED, MAX_SCROLL_SPEED, 1.0);
const BAND_DB_FLOOR_RANGE: SliderRange =
    SliderRange::new(MIN_BAND_DB_FLOOR, MAX_BAND_DB_FLOOR, 1.0);
const COLOR_MODE_OPTIONS: [WaveformColorMode; 3] = [
    WaveformColorMode::Frequency,
    WaveformColorMode::Loudness,
    WaveformColorMode::Static,
];

fn configure_palette_for_mode(palette: &mut PaletteEditor, mode: WaveformColorMode) {
    match mode {
        WaveformColorMode::Static => {
            palette.set_visible_indices(Some(vec![0]));
            palette.set_label_overrides(vec![(0, "Color")]);
        }
        WaveformColorMode::Loudness => {
            palette.set_visible_indices(None);
            palette.set_label_overrides(vec![(0, "Quiet"), (5, "Loud")]);
        }
        WaveformColorMode::Frequency => {
            palette.set_visible_indices(None);
            palette.set_label_overrides(vec![]);
        }
    }
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
        column![
            labeled_slider(
                "Scroll speed",
                self.settings.scroll_speed,
                format!("{:.0} px/s", self.settings.scroll_speed),
                SCROLL_SPEED_RANGE,
                Message::ScrollSpeed
            ),
            labeled_pick_list(
                "Channel 1",
                Channel::ALL,
                Some(self.settings.channel_1),
                Message::Channel1
            ),
            labeled_pick_list(
                "Channel 2",
                Channel::ALL,
                Some(self.settings.channel_2),
                Message::Channel2
            ),
            labeled_pick_list(
                "Color mode",
                &COLOR_MODE_OPTIONS,
                Some(self.settings.color_mode),
                Message::ColorMode
            ),
            labeled_toggler(
                "Peak history",
                self.settings.show_peak_history,
                Message::ShowPeakHistory
            ),
            labeled_slider(
                "Peak range",
                self.settings.band_db_floor,
                format!("{:.0} dB", self.settings.band_db_floor),
                BAND_DB_FLOOR_RANGE,
                Message::BandDbFloor
            ),
            super::palette_section(&self.palette, Message::Palette)
        ]
        .spacing(16)
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
