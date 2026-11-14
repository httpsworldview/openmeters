// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

mod loudness;
mod oscilloscope;
mod spectrogram;
mod spectrum;
mod stereometer;
mod waveform;

use crate::persistence::settings::{PaletteSettings, SettingsHandle, VisualConfig};
use crate::ui::theme::Palette;
use crate::ui::widgets::SliderRange;
use crate::ui::widgets::palette_editor::{PaletteChange, PaletteEditor, PaletteEvent};
use crate::util::audio::{DB_FLOOR, FftSize, NegativeDb};
use crate::util::set_if_changed as set;
use crate::visuals::registry::{VisualKind, VisualManagerHandle};
use iced::Element;
use std::num::NonZeroUsize;

const FFT_OPTIONS: [usize; 5] = [1024, 2048, 4096, 8192, 16384];
const HOP_DIVISORS: [usize; 7] = [4, 6, 8, 16, 32, 64, 128];
const QUARTER_TURNS: SliderRange = SliderRange::new(-1.0, 2.0, 1.0);
const DB_FLOOR_RANGE: SliderRange = SliderRange::new(DB_FLOOR, NegativeDb::MAX, 1.0);

fn quarter_turn_label(rotation: i8) -> String {
    format!("{}\u{00b0}", i32::from(rotation) * 90)
}

fn quarter_turn_slider<'a, M: Clone + 'a>(
    rotation: i8,
    on_change: impl Fn(i8) -> M + 'a,
) -> iced::widget::Column<'a, M> {
    slider!(
        "Rotation",
        f32::from(rotation),
        QUARTER_TURNS,
        move |value| on_change(value.round() as i8),
        quarter_turn_label(rotation)
    )
}

fn fmt_ms(seconds: f32, decimals: usize) -> String {
    format!("{:.decimals$} ms", seconds * 1000.0)
}

macro_rules! settings_modules {
    ($($module:ident => $variant:ident),+ $(,)?) => {
        #[derive(Debug, Clone)]
        pub(in crate::ui) enum SettingsMessage { $($variant($module::Message),)+ }

        pub(in crate::ui) enum ActiveSettings { $($variant($module::Pane),)+ }

        impl ActiveSettings {
            pub(in crate::ui) fn open(kind: VisualKind, settings: &SettingsHandle) -> Self {
                let settings = settings.borrow();
                let palette = load_palette(kind, settings.theme().palettes.get(&kind));
                match settings.data.visuals.config(kind) {
                    $(VisualConfig::$variant(config) => Self::$variant($module::create(config, palette)),)+
                }
            }

            pub(in crate::ui) fn kind(&self) -> VisualKind {
                match self { $(Self::$variant(_) => VisualKind::$variant,)+ }
            }

            pub(in crate::ui) fn config(&self) -> VisualConfig {
                match self { $(Self::$variant(pane) => VisualConfig::$variant(pane.settings),)+ }
            }

            pub(in crate::ui) fn view(&self) -> Element<'_, SettingsMessage> {
                match self {
                    $(Self::$variant(pane) => pane.view().map(SettingsMessage::$variant),)+
                }
            }

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
                        if changed {
                            let config = VisualConfig::$variant(pane.settings);
                            settings.edit(|s| s.visuals.set_config(config));
                        }
                        if let Some(change) = change {
                            let palette = pane.palette.settings();
                            let palette = palette.as_ref();
                            show_palette(VisualKind::$variant, palette, change, manager, settings);
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

fn load_palette(kind: VisualKind, stored: Option<&PaletteSettings>) -> PaletteEditor {
    let mut editor = PaletteEditor::new(Palette::for_kind(kind));
    if let Some(stored) = stored {
        editor.load(stored);
    }
    editor
}

fn show_palette(
    kind: VisualKind,
    palette: Option<&PaletteSettings>,
    change: PaletteChange,
    manager: &VisualManagerHandle,
    settings: &SettingsHandle,
) {
    manager.borrow_mut().apply_palette(kind, palette);
    if change == PaletteChange::Save {
        settings.edit_theme(|theme| {
            match palette {
                Some(palette) => theme.palettes.insert(kind, palette.clone()),
                None => theme.palettes.remove(&kind),
            };
        });
    }
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
fn update_fft_size(fft_size: &mut FftSize, hop_size: &mut NonZeroUsize, new_size: usize) -> bool {
    let hop_divisor = get_closest_hop_divisor(fft_size.get(), hop_size.get());
    if !fft_size.set(new_size) {
        return false;
    }
    update_hop_divisor(*fft_size, hop_size, hop_divisor);
    true
}

fn update_hop_divisor(fft_size: FftSize, hop_size: &mut NonZeroUsize, divisor: usize) -> bool {
    let hop = NonZeroUsize::new(fft_size.get() / divisor).unwrap_or(NonZeroUsize::MIN);
    set(hop_size, hop)
}

macro_rules! settings_pane {
    (
        $settings_ty:ty
        $(, extra_from_settings($source:ident) {
            $($field:ident: $ty:ty = $init:expr),* $(,)?
        })?
        $(, init_palette($editor:ident, $palette_source:ident) $init_body:block)?
        $(,)?
    ) => {
        pub(in crate::ui) struct Pane {
            pub(super) settings: $settings_ty,
            pub(super) palette: $crate::ui::widgets::palette_editor::PaletteEditor,
            disclosure: $crate::ui::widgets::Disclosure,
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
                let $palette_source = &loaded_settings;
                $init_body
            )?
            Pane {
                settings: loaded_settings,
                palette,
                disclosure: Default::default(),
                $($($field,)*)?
            }
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
            Toggle(&'static str),
        }

        impl Pane {
            /// Whether settings changed, and how the palette did.
            pub(super) fn handle(&mut self, message: Message) -> (bool, Option<super::PaletteChange>) {
                let $pane = self;
                let $settings = &mut $pane.settings;
                let changed = match message {
                    $(Message::$variant($value) => $handler,)+
                    Message::Palette($value) => return (false, $pane.palette.update($value)),
                    Message::Toggle(title) => {
                        $pane.disclosure.toggle(title);
                        return (false, None);
                    }
                };
                // Saving settings also saves a drag whose release was lost.
                (changed, changed.then(|| $pane.palette.save_changes()).flatten())
            }
        }
    };
}

/// Cards are `"Title" => content;`, or collapsible as
/// `"Title" [summary] => content;`. Colors always come last, collapsible.
macro_rules! settings_view {
    (
        $pane:ident as $settings:ident { $($body:tt)* }
        $($label:literal $([$summary:expr])? => $content:expr;)*
    ) => {
        impl Pane {
            pub(super) fn view(&self) -> iced::Element<'_, Message> {
                use Message::*;
                let $pane = self;
                let $settings = &$pane.settings;
                $($body)*
                iced::widget::Column::new()
                    .spacing($crate::ui::theme::SECTION_GAP)
                    $(.push(settings_view!(@card $pane, $label $(, $summary)?, $content)))*
                    .push($pane.disclosure.card(
                        "Colors",
                        [$pane.palette.preview()],
                        Toggle,
                        $pane.palette.view().map(Palette),
                    ))
                    .into()
            }
        }
    };
    (@card $pane:ident, $label:literal, $content:expr) => {
        $crate::ui::widgets::card($label, $content)
    };
    (@card $pane:ident, $label:literal, $summary:expr, $content:expr) => {
        $pane.disclosure.card($label, $summary, Toggle, $content)
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
        let settings = SettingsHandle::load_from(dir.path());
        let manager = VisualManagerHandle::default();
        let mut pane = ActiveSettings::open(VisualKind::Waveform, &settings);
        let mut send =
            |message| pane.handle(SettingsMessage::Waveform(message), &manager, &settings);
        let theme = || {
            let settings = settings.borrow();
            let palette = settings.theme().palettes.get(&VisualKind::Waveform);
            palette.cloned()
        };
        send(Palette(Select(Some(0))));
        for alpha in [0.5, 1.0] {
            send(Palette(Adjust(Alpha(alpha))));
        }
        send(Palette(Commit));
        assert_eq!(settings.borrow().active_theme(), None);
        send(Palette(Adjust(Alpha(0.5))));
        send(Palette(EditHex(Some("#123".into()))));
        assert!(theme().is_none());
        send(ScrollSpeed(123.0));
        assert!(theme().unwrap().stops().next().unwrap().a < 1.0);
    }

    #[test]
    fn toggling_sections_and_typing_hex_neither_apply_nor_save() {
        use spectrum::Message;
        let dir = tempfile::tempdir().unwrap();
        let settings = SettingsHandle::load_from(dir.path());
        let manager = VisualManagerHandle::default();
        let config = || {
            let settings = settings.borrow();
            let kind = VisualKind::Spectrum;
            let palette = settings.theme().palettes.get(&kind).cloned();
            (settings.data.visuals.config(kind), palette)
        };
        let before = config();
        let mut pane = ActiveSettings::open(VisualKind::Spectrum, &settings);
        let toggles = ["Analysis", "Details", "Colors"]
            .into_iter()
            .flat_map(|title| [Message::Toggle(title), Message::Toggle(title)]);
        let draft = [Select(Some(0)), EditHex(Some("#123".into()))].map(Message::Palette);
        for message in toggles.chain(draft) {
            pane.handle(SettingsMessage::Spectrum(message), &manager, &settings);
        }
        assert_eq!(config(), before);
        settings.flush();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
