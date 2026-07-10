// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

macro_rules! settings_view {
    (
        $pane:ident as $settings:ident { $($body:tt)* }
        $($label:expr => $content:expr;)*
    ) => {
        impl Pane {
            pub(super) fn view(&self) -> iced::Element<'_, Message> {
                use Message::*;
                let $pane = self;
                let $settings = &$pane.settings;
                $($body)*
                iced::widget::Column::new()
                    .spacing($crate::ui::theme::SECTION_GAP)
                    $(.push($crate::ui::widgets::card($label, $content)))*
                    .push($crate::ui::widgets::card(
                        "Colors",
                        $pane.palette.view().map(Message::Palette),
                    ))
                    .into()
            }
        }
    };
}

macro_rules! settings_modules {
    ($($module:ident => $variant:ident),+ $(,)?) => {
        $(mod $module;)+

        #[derive(Debug, Clone)]
        pub(in crate::ui) enum SettingsMessage { $($variant($module::Message),)+ }

        enum SettingsPane { $($variant($module::Pane),)+ }

        impl SettingsPane {
            fn new(kind: VisualKind, manager: &VisualManagerHandle) -> Self {
                match kind {
                    $(VisualKind::$variant => Self::$variant($module::create(manager, kind)),)+
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
                manager: &VisualManagerHandle,
                settings: &SettingsHandle,
            ) {
                match (self, message) {
                    $((Self::$variant(pane), SettingsMessage::$variant(message)) => {
                        if pane.handle(message) {
                            persist_with_palette(
                                manager, settings, VisualKind::$variant,
                                &pane.settings, &pane.palette,
                            );
                        }
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
        $(, extra_from_settings($source:ident) {
            $($field:ident: $ty:ty = $init:expr),* $(,)?
        })?
        $(, init_palette($editor:ident $(, $palette_source:ident)?) $init_body:block)?
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
            let (loaded_settings, palette) =
                super::load_settings_and_palette::<$settings_ty>(visual_manager, kind);
            $($(
                let $field: $ty = {
                    let $source = &loaded_settings;
                    $init
                };
            )*)?
            $(
                let mut palette = palette;
                let $editor = &mut palette;
                $(let $palette_source = &loaded_settings;)?
                $init_body
            )?
            Pane { settings: loaded_settings, palette, $($($field,)*)? }
        }
    };
}

macro_rules! settings_messages {
    ($pane:ident, $settings:ident, $value:ident {
        $($variant:ident($ty:ty) => $handler:expr;)+
    }) => {
        #[derive(Debug, Clone)]
        pub enum Message {
            $($variant($ty),)+
            Palette(crate::ui::widgets::palette_editor::PaletteEvent),
        }

        impl Pane {
            pub(super) fn handle(&mut self, message: Message) -> bool {
                let $pane = self;
                let $settings = &mut $pane.settings;
                match message {
                    $(Message::$variant($value) => $handler,)+
                    Message::Palette($value) => $pane.palette.update($value),
                }
            }
        }
    };
}

use crate::persistence::settings::{
    BUILTIN_THEME, HasPalette, ModuleSettings, PaletteSettings, SettingsConfig, SettingsHandle,
};
use crate::ui::theme::Palette;
use crate::ui::widgets::{SliderRange, palette_editor::PaletteEditor};
use crate::visuals::registry::{VisualKind, VisualManagerHandle};
use iced::{Color, Element};

const FFT_OPTIONS: [usize; 5] = [1024, 2048, 4096, 8192, 16384];
const HOP_DIVISORS: [usize; 7] = [4, 6, 8, 16, 32, 64, 128];

fn set<T: PartialEq>(target: &mut T, value: T) -> bool {
    if *target == value {
        return false;
    }
    *target = value;
    true
}

// Compare bits to avoid spurious writes for identical NaN payloads.
fn set_f32(target: &mut f32, value: f32, range: SliderRange) -> bool {
    let value = range.snap(value);
    if target.to_bits() == value.to_bits() {
        return false;
    }
    *target = value;
    true
}

fn set_usize(target: &mut usize, value: f32, range: SliderRange) -> bool {
    debug_assert!(
        [range.min, range.max, range.step]
            .into_iter()
            .all(|value| value.fract().abs() <= f32::EPSILON),
        "set_usize expects integral slider bounds"
    );
    set(target, range.snap(value).round() as usize)
}

fn get_closest_hop_divisor(fft_size: usize, hop_size: usize) -> usize {
    if fft_size == 0 || hop_size == 0 {
        return 8;
    }
    let ratio = fft_size as f32 / hop_size as f32;
    HOP_DIVISORS
        .into_iter()
        .min_by(|&left, &right| {
            (ratio - left as f32)
                .abs()
                .total_cmp(&(ratio - right as f32).abs())
        })
        .unwrap_or(8)
}

// Preserve the hop:fft ratio when fft_size changes.
fn update_fft_size(fft_size: &mut usize, hop_size: &mut usize, new_size: usize) -> bool {
    let hop_divisor = get_closest_hop_divisor(*fft_size, *hop_size);
    if !set(fft_size, new_size) {
        return false;
    }
    *hop_size = (new_size / hop_divisor).max(1);
    true
}

fn update_hop_divisor(fft_size: usize, hop_size: &mut usize, divisor: usize) -> bool {
    set(hop_size, (fft_size / divisor.max(1)).max(1))
}

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

impl ActiveSettings {
    pub(in crate::ui) fn new(kind: VisualKind, visual_manager: &VisualManagerHandle) -> Self {
        Self {
            kind,
            pane: SettingsPane::new(kind, visual_manager),
        }
    }

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

pub(super) fn persist_with_palette<T: Clone + serde::Serialize + HasPalette>(
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
    settings_handle.update(move |settings| {
        settings
            .data
            .visuals
            .modules
            .entry(kind)
            .or_default()
            .set_config(&stored);
        if palette_settings.is_some() || settings.active_theme() != BUILTIN_THEME {
            settings.update_active_theme(|theme| {
                if let Some(ps) = palette_settings {
                    theme.palettes.insert(kind, ps);
                } else {
                    theme.palettes.remove(&kind);
                }
            });
        }
    });
}
