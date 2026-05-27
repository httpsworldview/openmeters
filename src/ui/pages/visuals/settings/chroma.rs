// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::widgets::{SliderRange, set_if_changed, slide, toggle, update_f32_range};
use crate::persistence::settings::ChromaSettings;
use crate::visuals::chroma::processor::{
    MAX_FLOOR_DB, MAX_SMOOTHING, MIN_FLOOR_DB, MIN_SMOOTHING,
};
use crate::visuals::registry::VisualKind;
use iced::Element;

settings_pane!(ChromaSettingsPane, ChromaSettings, VisualKind::Chroma, Chroma);

const SMOOTHING_RANGE: SliderRange = SliderRange::new(MIN_SMOOTHING, MAX_SMOOTHING, 0.005);
const FLOOR_RANGE: SliderRange = SliderRange::new(MIN_FLOOR_DB, MAX_FLOOR_DB, 1.0);

settings_messages!(ChromaSettingsPane as pane, value {
    Smoothing(f32) => update_f32_range(&mut pane.settings.smoothing, value, SMOOTHING_RANGE);
    FloorDb(f32) => update_f32_range(&mut pane.settings.floor_db, value, FLOOR_RANGE);
    ShowPeakHold(bool) => set_if_changed(&mut pane.settings.show_peak_hold, value);
});

impl ChromaSettingsPane {
    fn view(&self) -> Element<'_, Message> {
        let s = &self.settings;
        controls!(16.0;
            slider!(
                "Smoothing",
                s.smoothing,
                SMOOTHING_RANGE,
                Message::Smoothing,
                format!("{:.3}", s.smoothing)
            );
            slider!(
                "Floor",
                s.floor_db,
                FLOOR_RANGE,
                Message::FloorDb,
                format!("{:.0} dB", s.floor_db)
            );
            toggle("Peak hold", s.show_peak_hold, Message::ShowPeakHold);
            super::palette_section(&self.palette, Message::Palette);
        )
        .into()
    }
}
