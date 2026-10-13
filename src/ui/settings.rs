// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

mod loudness;
mod oscilloscope;
mod spectrogram;
mod spectrum;
mod stereometer;
mod waveform;

use crate::persistence::settings::{
    BUILTIN_THEME, ChangeOrigin::User, PaletteSettings, SettingsHandle, VisualConfig,
};
use crate::ui::theme::Palette;
use crate::ui::widgets::palette_editor::{PaletteChange, PaletteEditor, PaletteEvent};
use crate::util::set_if_changed as set;
use crate::visuals::registry::{VisualKind, VisualManagerHandle};
use iced::Element;

const FFT_OPTIONS: [usize; 5] = [1024, 2048, 4096, 8192, 16384];
const HOP_DIVISORS: [usize; 7] = [4, 6, 8, 16, 32, 64, 128];

macro_rules! settings_modules {
    ($($module:ident => $variant:ident),+ $(,)?) => {
        #[derive(Debug, Clone)]
        pub(in crate::ui) enum SettingsMessage { $($variant($module::Message),)+ }

        pub(in crate::ui) enum ActiveSettings { $($variant($module::Pane),)+ }

        impl ActiveSettings {
            pub(in crate::ui) fn new(kind: VisualKind, manager: &VisualManagerHandle) -> Self {
                let config = manager.borrow().config(kind);
                let palette = load_palette(&config);
                match config {
                    $(VisualConfig::$variant(settings) => Self::$variant($module::create(settings, palette)),)+
                }
            }

            pub(in crate::ui) fn kind(&self) -> VisualKind {
                match self { $(Self::$variant(_) => VisualKind::$variant,)+ }
            }

            pub(in crate::ui) fn view(&self) -> Element<'_, SettingsMessage> {
                match self {
                    $(Self::$variant(pane) => pane.view().map(SettingsMessage::$variant),)+
                }
            }

            /// Commits unfinished palette edits, e.g. before the pane closes.
            pub(in crate::ui) fn commit_edits(
                &mut self,
                manager: &VisualManagerHandle,
                settings: &SettingsHandle,
            ) {
                let message = match self {
                    $(Self::$variant(_) => {
                        SettingsMessage::$variant($module::Message::Palette(PaletteEvent::Commit))
                    })+
                };
                self.handle(message, manager, settings);
            }

            pub(in crate::ui) fn handle(
                &mut self,
                message: SettingsMessage,
                manager: &VisualManagerHandle,
                settings: &SettingsHandle,
            ) {
                match (self, message) {
                    $((Self::$variant(pane), SettingsMessage::$variant(message)) => {
                        let (changed, change) = pane.handle(message);
                        match change {
                            Some(PaletteChange::Preview) => manager
                                .borrow_mut()
                                .apply_palette(VisualKind::$variant, pane.palette.settings().as_ref()),
                            _ if changed || change.is_some() => {
                                let config = VisualConfig::$variant(pane.settings.clone());
                                save(manager, settings, config, pane.palette.settings(), change.is_some());
                            }
                            _ => {}
                        }
                    })+
                    _ => {}
                }
            }
        }
    };
}

settings_modules! {
    loudness => Loudness,
    oscilloscope => Oscilloscope,
    spectrogram => Spectrogram,
    spectrum => Spectrum,
    stereometer => Stereometer,
    waveform => Waveform,
}

fn load_palette(config: &VisualConfig) -> PaletteEditor {
    let mut editor = PaletteEditor::new(Palette::for_kind(config.kind()));
    if let Some(stored) = config.palette() {
        editor.load(stored);
    }
    editor
}

fn save(
    visual_manager: &VisualManagerHandle,
    settings_handle: &SettingsHandle,
    mut config: VisualConfig,
    palette: Option<PaletteSettings>,
    palette_saved: bool,
) {
    let kind = config.kind();
    *config.palette_mut() = palette.clone();
    visual_manager.borrow_mut().apply_config(config.clone());
    settings_handle.update(User, move |settings| {
        settings.data.visuals.set_config(config);
        if palette_saved && (palette.is_some() || settings.active_theme() != BUILTIN_THEME) {
            settings.update_active_theme(|theme| {
                match palette {
                    Some(palette) => theme.palettes.insert(kind, palette),
                    None => theme.palettes.remove(&kind),
                };
            });
        }
    });
}

// Compare bits to avoid spurious writes for identical NaN payloads.
fn set_f32(target: &mut f32, value: f32) -> bool {
    if target.to_bits() == value.to_bits() {
        return false;
    }
    *target = value;
    true
}

fn set_usize(target: &mut usize, value: f32) -> bool {
    set(target, value.round() as usize)
}

fn get_closest_hop_divisor(fft_size: usize, hop_size: usize) -> usize {
    let ratio = fft_size as f32 / hop_size as f32;
    HOP_DIVISORS
        .into_iter()
        .min_by(|&left, &right| {
            (ratio - left as f32)
                .abs()
                .total_cmp(&(ratio - right as f32).abs())
        })
        .unwrap()
}

// Preserve the nearest supported hop divisor.
fn update_fft_size(fft_size: &mut usize, hop_size: &mut usize, new_size: usize) -> bool {
    let hop_divisor = get_closest_hop_divisor(*fft_size, *hop_size);
    if !set(fft_size, new_size) {
        return false;
    }
    *hop_size = new_size / hop_divisor;
    true
}

fn update_hop_divisor(fft_size: usize, hop_size: &mut usize, divisor: usize) -> bool {
    set(hop_size, (fft_size / divisor).max(1))
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
        pub(in crate::ui) struct Pane {
            pub(super) settings: $settings_ty,
            pub(super) palette: $crate::ui::widgets::palette_editor::PaletteEditor,
            $($($field: $ty,)*)?
        }

        pub(super) fn create(
            loaded_settings: $settings_ty,
            palette: $crate::ui::widgets::palette_editor::PaletteEditor,
        ) -> Pane {
            $(
                let $source = &loaded_settings;
                $(let $field: $ty = $init;)*
            )?
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
            Palette($crate::ui::widgets::palette_editor::PaletteEvent),
        }

        impl Pane {
            /// Whether settings changed, and how the palette did.
            pub(super) fn handle(&mut self, message: Message) -> (bool, Option<super::PaletteChange>) {
                let $pane = self;
                let $settings = &mut $pane.settings;
                let changed = match message {
                    $(Message::$variant($value) => $handler,)+
                    Message::Palette($value) => return (false, $pane.palette.update($value)),
                };
                // Saving settings also saves a drag whose release was lost.
                (changed, changed.then(|| $pane.palette.save_changes()).flatten())
            }
        }
    };
}

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

use {settings_messages, settings_pane, settings_view};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::widgets::palette_editor::{HsvEdit::Alpha, PaletteEvent::*};
    use waveform::Message::{Palette, ScrollSpeed};

    #[test]
    fn palette_drags_save_with_settings_and_only_when_changed() {
        let dir = tempfile::tempdir().unwrap();
        let settings = SettingsHandle::for_test(dir.path());
        let manager = VisualManagerHandle::default();
        let mut pane = ActiveSettings::new(VisualKind::Waveform, &manager);
        let mut send =
            |message| pane.handle(SettingsMessage::Waveform(message), &manager, &settings);
        let live = || {
            manager
                .borrow()
                .config(VisualKind::Waveform)
                .palette()
                .cloned()
        };
        let theme = || {
            let settings = settings.borrow();
            let theme = settings.theme_store().load(settings.active_theme());
            theme.unwrap().palettes.get(&VisualKind::Waveform).cloned()
        };
        send(Palette(Select(Some(0))));
        for alpha in [0.5, 1.0] {
            send(Palette(Adjust(Alpha(alpha))));
            assert_eq!(live().is_some(), alpha < 1.0);
        }
        send(Palette(Commit));
        assert_eq!(settings.borrow().active_theme(), BUILTIN_THEME);
        send(Palette(Adjust(Alpha(0.5))));
        send(Palette(EditHex(Some("#123".into()))));
        assert!(theme().is_none());
        send(ScrollSpeed(123.0));
        assert!(iced::Color::from(theme().unwrap().stops[0]).a < 1.0);
        SettingsHandle::flush();
    }
}
