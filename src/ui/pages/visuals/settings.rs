// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

macro_rules! settings_pane {
    (
        $pane:ident, $settings_ty:ty, $kind:expr, $variant:ident,
        extra_from_settings($s:ident) { $($field:ident : $ty:ty = $init:expr),* $(,)? }
        $(init_palette($p:ident) $init_body:block)?
    ) => {
        #[derive(Debug)]
        pub struct $pane {
            visual_id: super::VisualId,
            settings: $settings_ty,
            palette: super::palette::PaletteEditor,
            $($field: $ty,)*
        }

        pub fn create(visual_id: super::VisualId, visual_manager: &super::VisualManagerHandle) -> $pane {
            let ($s, mut _palette) = super::load_settings_and_palette::<$settings_ty>(
                visual_manager, $kind,
            );
            $(let $field: $ty = $init;)*
            $(let $p = &mut _palette; $init_body)?
            $pane { visual_id, settings: $s, palette: _palette, $($field,)* }
        }

        impl super::ModuleSettingsPane for $pane {
            fn visual_id(&self) -> super::VisualId { self.visual_id }
            fn view(&self) -> iced::Element<'_, super::SettingsMessage> {
                $pane::view(self).map(super::SettingsMessage::$variant)
            }
            fn handle(
                &mut self,
                message: &super::SettingsMessage,
                visual_manager: &super::VisualManagerHandle,
                settings_handle: &crate::persistence::settings::SettingsHandle,
            ) {
                if let super::SettingsMessage::$variant(msg) = message
                    && $pane::handle(self, msg)
                {
                    super::persist_with_palette(
                        visual_manager, settings_handle, $kind, &self.settings, &self.palette,
                    );
                }
            }
        }
    };
    ($pane:ident, $settings_ty:ty, $kind:expr, $variant:ident) => {
        settings_pane!($pane, $settings_ty, $kind, $variant, extra_from_settings(_s) {});
    };
}

mod loudness;
mod oscilloscope;
pub mod palette;
mod spectrogram;
mod spectrum;
mod stereometer;
mod waveform;
mod widgets;

use self::palette::{PaletteEditor, PaletteEvent};
use crate::persistence::settings::{HasPalette, ModuleSettings, PaletteSettings, SettingsHandle};
use crate::ui::theme::Palette;
use crate::visuals::registry::{VisualId, VisualKind, VisualManagerHandle};
use iced::widget::column;
use iced::{Color, Element};
use serde::{Serialize, de::DeserializeOwned};

#[derive(Debug, Clone)]
pub enum SettingsMessage {
    Loudness(loudness::Message),
    Oscilloscope(oscilloscope::Message),
    Spectrogram(spectrogram::Message),
    Spectrum(spectrum::Message),
    Stereometer(stereometer::Message),
    Waveform(waveform::Message),
}

pub trait ModuleSettingsPane: std::fmt::Debug + 'static {
    fn visual_id(&self) -> VisualId;
    fn view(&self) -> Element<'_, SettingsMessage>;
    fn handle(
        &mut self,
        message: &SettingsMessage,
        visual_manager: &VisualManagerHandle,
        settings_handle: &SettingsHandle,
    );
}

pub type ActiveSettings = Box<dyn ModuleSettingsPane>;

pub fn create_panel(
    visual_id: VisualId,
    kind: VisualKind,
    visual_manager: &VisualManagerHandle,
) -> ActiveSettings {
    match kind {
        VisualKind::Loudness => Box::new(loudness::create(visual_id, visual_manager)),
        VisualKind::Oscilloscope => Box::new(oscilloscope::create(visual_id, visual_manager)),
        VisualKind::Spectrogram => Box::new(spectrogram::create(visual_id, visual_manager)),
        VisualKind::Spectrum => Box::new(spectrum::create(visual_id, visual_manager)),
        VisualKind::Stereometer => Box::new(stereometer::create(visual_id, visual_manager)),
        VisualKind::Waveform => Box::new(waveform::create(visual_id, visual_manager)),
    }
}

pub(super) fn load_settings_and_palette<T: DeserializeOwned + Default + HasPalette>(
    visual_manager: &VisualManagerHandle,
    kind: VisualKind,
) -> (T, PaletteEditor) {
    let settings: T = visual_manager
        .borrow()
        .module_settings(kind)
        .and_then(|stored| stored.parse_config::<T>())
        .unwrap_or_default();
    let mut editor = PaletteEditor::new(Palette::for_kind(kind));
    if let Some(stored) = settings.palette() {
        let stops: Vec<Color> = stored.stops.iter().copied().map(Into::into).collect();
        editor.set_colors(&stops);
        editor.set_positions(stored.stop_positions.as_deref());
        editor.set_spreads(stored.stop_spreads.as_deref());
    }
    (settings, editor)
}

pub(super) fn palette_section<'a, M: 'a>(
    palette: &'a PaletteEditor,
    map: fn(PaletteEvent) -> M,
) -> iced::widget::Column<'a, M> {
    column![widgets::section_title("Colors"), palette.view().map(map)].spacing(8)
}

pub(super) fn persist_with_palette<T: Clone + Serialize + HasPalette>(
    visual_manager: &VisualManagerHandle,
    settings_handle: &SettingsHandle,
    kind: VisualKind,
    config: &T,
    palette: &PaletteEditor,
) -> bool {
    let mut stored = config.clone();
    let positions = palette.positions();
    let spreads = palette.spreads();
    let palette_settings = PaletteSettings::from_state(
        palette.colors(),
        palette.defaults(),
        positions,
        palette.default_positions(),
        spreads,
    );
    stored.set_palette(palette_settings.clone());
    let applied = visual_manager
        .borrow_mut()
        .apply_module_settings(kind, &ModuleSettings::with_config(&stored));
    if applied {
        settings_handle.update(|s| s.set_module_config(kind, &stored));
        settings_handle.borrow().update_active_theme(|theme| {
            if let Some(ps) = palette_settings {
                theme.palettes.insert(kind, ps);
            } else {
                theme.palettes.remove(&kind);
            }
        });
    }
    applied
}
