// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

macro_rules! controls {
    ($spacing:literal; $($control:expr;)*) => {{
        controls!(@push iced::widget::Column::new().spacing($spacing); $($control;)*)
    }};
    ($base:expr; $($control:expr;)*) => {{
        controls!(@push $base; $($control;)*)
    }};
    (@push $base:expr; $($control:expr;)*) => {{
        let mut column = $base;
        $(column = column.push($control);)*
        column
    }};
}

macro_rules! slider {
    ($label:expr, $value:expr, $range:expr, $on_change:expr, $fmt:literal) => {
        slide($label, $value, format!($fmt, $value), $range, $on_change)
    };
    ($label:expr, $value:expr, $range:expr, $on_change:expr, $display:expr) => {
        slide($label, $value, $display, $range, $on_change)
    };
}

macro_rules! settings_modules {
    ($($module:ident => $variant:ident),+ $(,)?) => {
        $(mod $module;)+

        #[derive(Debug, Clone)]
        pub enum SettingsMessage {
            $($variant($module::Message),)+
        }

        pub fn create_panel(
            visual_id: VisualId,
            kind: VisualKind,
            visual_manager: &VisualManagerHandle,
        ) -> ActiveSettings {
            match kind {
                $(VisualKind::$variant => Box::new($module::create(visual_id, visual_manager)),)+
            }
        }
    };
}

macro_rules! settings_pane {
    (
        $pane:ident, $settings_ty:ty, $kind:expr, $variant:ident,
        extra_from_settings($s:ident) { $($field:ident : $ty:ty = $init:expr),* $(,)? }
        $(init_palette($p:ident) $init_body:block)?
    ) => {
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

macro_rules! settings_messages {
    ($pane:ident as $this:ident, $value:ident { $($variant:ident($ty:ty) => $handler:expr;)+ }) => {
        #[derive(Debug, Clone)]
        pub enum Message { $($variant($ty),)+ Palette(super::palette::PaletteEvent) }

        impl $pane {
            fn handle(&mut self, msg: &Message) -> bool {
                let $this = self;
                match (*msg).clone() {
                    $(Message::$variant($value) => $handler,)+
                    Message::Palette($value) => $this.palette.update($value),
                }
            }
        }
    };
}

pub mod palette;
mod widgets;

use self::palette::{PaletteEditor, PaletteEvent};
use crate::persistence::settings::{
    BUILTIN_THEME, HasPalette, ModuleSettings, PaletteSettings, SettingsHandle,
};
use crate::ui::theme::Palette;
use crate::visuals::registry::{VisualId, VisualKind, VisualManagerHandle};
use iced::widget::column;
use iced::{Color, Element};
use serde::{Serialize, de::DeserializeOwned};

settings_modules! {
    loudness => Loudness,
    oscilloscope => Oscilloscope,
    spectrogram => Spectrogram,
    spectrum => Spectrum,
    stereometer => Stereometer,
    waveform => Waveform,
}

pub trait ModuleSettingsPane: 'static {
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
    column![widgets::section("Colors"), palette.view().map(map)].spacing(8)
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
        settings_handle.update(move |s| {
            s.set_module_config(kind, &stored);
            if palette_settings.is_some() || s.active_theme() != BUILTIN_THEME {
                s.update_active_theme(|theme| {
                    if let Some(ps) = palette_settings {
                        theme.palettes.insert(kind, ps);
                    } else {
                        theme.palettes.remove(&kind);
                    }
                });
            }
        });
    }
    applied
}
