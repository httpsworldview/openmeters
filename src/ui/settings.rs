// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

macro_rules! settings_modules {
    ($($module:ident => $variant:ident),+ $(,)?) => {
        $(mod $module;)+

        #[derive(Debug, Clone)]
        pub(in crate::ui) enum SettingsMessage { $($variant($module::Message),)+ }

        enum SettingsPane { $($variant($module::Pane),)+ }

        impl SettingsPane {
            fn new(kind: VisualKind, visual_manager: &VisualManagerHandle) -> Self {
                match kind {
                    $(VisualKind::$variant => Self::$variant($module::create(visual_manager, kind)),)+
                }
            }

            fn view(&self) -> Element<'_, SettingsMessage> {
                match self {
                    $(Self::$variant(pane) => pane.view().map(SettingsMessage::$variant),)+
                }
            }

            fn handle(
                &mut self,
                message: SettingsMessage,
                visual_manager: &VisualManagerHandle,
                settings_handle: &SettingsHandle,
            ) {
                match (self, message) {
                    $((Self::$variant(pane), SettingsMessage::$variant(message)) => {
                        pane.apply(message, visual_manager, settings_handle, VisualKind::$variant);
                    })+
                    _ => {}
                }
            }
        }
    };
}

macro_rules! settings_pane {
    (
        $settings_ty:ty
        $(, extra_from_settings($s:ident) { $($field:ident : $ty:ty = $init:expr),* $(,)? })?
        $(, init_palette($p:ident $(, $ps:ident)?) $init_body:block)?
        $(,)?
    ) => {
        pub(super) struct Pane {
            pub(super) settings: $settings_ty,
            pub(super) palette: crate::ui::widgets::palette_editor::PaletteEditor,
            $($($field: $ty,)*)?
        }

        pub(super) fn create(
            visual_manager: &super::VisualManagerHandle,
            kind: crate::visuals::registry::VisualKind,
        ) -> Pane {
            let (_settings, mut _palette) = super::load_settings_and_palette::<$settings_ty>(
                visual_manager, kind,
            );
            $($(
                let $field: $ty = {
                    let $s = &_settings;
                    $init
                };
            )*)?
            $(let $p = &mut _palette; $(let $ps = &_settings;)? $init_body)?
            Pane { settings: _settings, palette: _palette, $($($field,)*)? }
        }
    };
}

macro_rules! settings_messages {
    ($this:ident, $value:ident { $($variant:ident($ty:ty) => $handler:expr;)+ }) => {
        #[derive(Debug, Clone)]
        pub enum Message {
            $($variant($ty),)+
            Palette(crate::ui::widgets::palette_editor::PaletteEvent),
        }

        impl Pane {
            fn handle(&mut self, msg: Message) -> bool {
                let $this = self;
                match msg {
                    $(Message::$variant($value) => $handler,)+
                    Message::Palette($value) => $this.palette.update($value),
                }
            }

            pub(super) fn apply(
                &mut self,
                msg: Message,
                visual_manager: &super::VisualManagerHandle,
                settings_handle: &super::SettingsHandle,
                kind: super::VisualKind,
            ) {
                if self.handle(msg) {
                    super::persist_with_palette(
                        visual_manager,
                        settings_handle,
                        kind,
                        &self.settings,
                        &self.palette,
                    );
                }
            }
        }
    };
}

pub(in crate::ui) mod widgets;

use crate::persistence::settings::{
    BUILTIN_THEME, HasPalette, ModuleSettings, PaletteSettings, SettingsConfig, SettingsHandle,
};
use crate::ui::theme::Palette;
use crate::ui::widgets::palette_editor::PaletteEditor;
use crate::visuals::registry::{VisualKind, VisualManagerHandle};
use iced::{Color, Element};
use serde::Serialize;

settings_modules! {
    loudness => Loudness,
    oscilloscope => Oscilloscope,
    spectrogram => Spectrogram,
    spectrum => Spectrum,
    stereometer => Stereometer,
    waveform => Waveform,
}

pub(in crate::ui) struct ActiveSettings {
    pub(in crate::ui) kind: VisualKind,
    pane: SettingsPane,
}

pub(in crate::ui) fn create_panel(
    kind: VisualKind,
    visual_manager: &VisualManagerHandle,
) -> ActiveSettings {
    ActiveSettings {
        kind,
        pane: SettingsPane::new(kind, visual_manager),
    }
}

impl ActiveSettings {
    pub(in crate::ui) fn view(&self) -> Element<'_, SettingsMessage> {
        self.pane.view()
    }

    pub(in crate::ui) fn handle(
        &mut self,
        message: SettingsMessage,
        visual_manager: &VisualManagerHandle,
        settings_handle: &SettingsHandle,
    ) {
        self.pane.handle(message, visual_manager, settings_handle);
    }
}

pub(super) fn load_settings_and_palette<T: SettingsConfig + HasPalette>(
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

pub(super) fn persist_with_palette<T: Clone + Serialize + HasPalette>(
    visual_manager: &VisualManagerHandle,
    settings_handle: &SettingsHandle,
    kind: VisualKind,
    config: &T,
    palette: &PaletteEditor,
) {
    let mut stored = config.clone();
    let palette_settings = PaletteSettings::from_state(
        palette.colors(),
        palette.defaults(),
        palette.positions(),
        palette.default_positions(),
        palette.spreads(),
    );
    stored.set_palette(palette_settings.clone());
    visual_manager
        .borrow_mut()
        .apply_module_settings(kind, &ModuleSettings::with_config(&stored));
    settings_handle.update(move |s| {
        s.data
            .visuals
            .modules
            .entry(kind)
            .or_default()
            .set_config(&stored);
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
