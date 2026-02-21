// Contains the settings panes for visual modules.

macro_rules! settings_pane {
    (
        $pane:ident, $settings_ty:ty, $kind:expr, $palette_mod:path, $variant:ident,
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
            use $palette_mod as pal;
            let ($s, mut _palette) = super::load_settings_and_palette::<$settings_ty>(
                visual_manager, $kind, &pal::COLORS, pal::LABELS,
            );
            $(let $field: $ty = $init;)*
            $(let $p = &mut _palette; $init_body)?
            $pane { visual_id, settings: $s, palette: _palette, $($field,)* }
        }

        settings_pane!(@impl $pane, $variant, $kind, $palette_mod);
    };
    (
        $pane:ident, $settings_ty:ty, $kind:expr, $palette_mod:path, $variant:ident,
        init_palette($s:ident, $p:ident) $init_body:block
    ) => {
        #[derive(Debug)]
        pub struct $pane {
            visual_id: super::VisualId,
            settings: $settings_ty,
            palette: super::palette::PaletteEditor,
        }

        pub fn create(visual_id: super::VisualId, visual_manager: &super::VisualManagerHandle) -> $pane {
            use $palette_mod as pal;
            let ($s, mut $p) = super::load_settings_and_palette::<$settings_ty>(
                visual_manager, $kind, &pal::COLORS, pal::LABELS,
            );
            $init_body
            $pane { visual_id, settings: $s, palette: $p }
        }

        settings_pane!(@impl $pane, $variant, $kind, $palette_mod);
    };
    ($pane:ident, $settings_ty:ty, $kind:expr, $palette_mod:path, $variant:ident) => {
        settings_pane!($pane, $settings_ty, $kind, $palette_mod, $variant, init_palette(_s, _p) {});
    };
    (@impl $pane:ident, $variant:ident, $kind:expr, $palette_mod:path) => {
        impl super::ModuleSettingsPane for $pane {
            fn visual_id(&self) -> super::VisualId { self.visual_id }
            fn view(&self) -> iced::Element<'_, super::SettingsMessage> {
                $pane::view(self).map(super::SettingsMessage::$variant)
            }
            fn handle(
                &mut self,
                message: &super::SettingsMessage,
                visual_manager: &super::VisualManagerHandle,
                settings_handle: &crate::ui::settings::SettingsHandle,
            ) {
                if let super::SettingsMessage::$variant(msg) = message {
                    if $pane::handle(self, msg) {
                        use $palette_mod as pal;
                        super::persist_with_palette(
                            visual_manager, settings_handle, $kind,
                            &self.settings, &self.palette, &pal::COLORS,
                        );
                    }
                }
            }
        }
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
use crate::ui::settings::{
    ChannelMode, HasPalette, ModuleSettings, PaletteSettings, SettingsHandle,
};
use crate::ui::theme::Palette;
use crate::ui::visualization::visual_manager::{VisualId, VisualKind, VisualManagerHandle};
use iced::widget::column;
use iced::{Color, Element};
use serde::{Serialize, de::DeserializeOwned};

pub(super) const CHANNEL_OPTIONS: [ChannelMode; 4] = [
    ChannelMode::Both,
    ChannelMode::Left,
    ChannelMode::Right,
    ChannelMode::Mono,
];

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

#[derive(Debug)]
pub struct ActiveSettings {
    pane: Box<dyn ModuleSettingsPane>,
}

impl ActiveSettings {
    pub fn visual_id(&self) -> VisualId {
        self.pane.visual_id()
    }
    pub fn view(&self) -> Element<'_, SettingsMessage> {
        self.pane.view()
    }
    pub fn handle_message(
        &mut self,
        message: &SettingsMessage,
        visual_manager: &VisualManagerHandle,
        settings_handle: &SettingsHandle,
    ) {
        self.pane.handle(message, visual_manager, settings_handle);
    }
}

pub fn create_panel(
    visual_id: VisualId,
    kind: VisualKind,
    visual_manager: &VisualManagerHandle,
) -> ActiveSettings {
    ActiveSettings {
        pane: match kind {
            VisualKind::Loudness => Box::new(loudness::create(visual_id, visual_manager)),
            VisualKind::Oscilloscope => Box::new(oscilloscope::create(visual_id, visual_manager)),
            VisualKind::Spectrogram => Box::new(spectrogram::create(visual_id, visual_manager)),
            VisualKind::Spectrum => Box::new(spectrum::create(visual_id, visual_manager)),
            VisualKind::Stereometer => Box::new(stereometer::create(visual_id, visual_manager)),
            VisualKind::Waveform => Box::new(waveform::create(visual_id, visual_manager)),
        },
    }
}

pub(super) fn load_settings_and_palette<T: DeserializeOwned + Default + HasPalette>(
    visual_manager: &VisualManagerHandle,
    kind: VisualKind,
    defaults: &'static [Color],
    labels: &'static [&'static str],
) -> (T, PaletteEditor) {
    let settings: T = visual_manager
        .borrow()
        .module_settings(kind)
        .and_then(|stored| stored.parse_config::<T>())
        .unwrap_or_default();
    let mut editor = PaletteEditor::new(Palette::new(defaults, labels));
    if let Some(stored) = settings.palette() {
        editor.set_colors(
            &stored
                .stops
                .iter()
                .map(|c| (*c).into())
                .collect::<Vec<Color>>(),
        );
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
    defaults: &[Color],
) -> bool {
    let mut stored = config.clone();
    let positions = palette.positions();
    let spreads = palette.spreads();
    stored.set_palette(PaletteSettings::from_state(
        palette.colors(),
        defaults,
        positions,
        spreads,
    ));
    let applied = visual_manager
        .borrow_mut()
        .apply_module_settings(kind, &ModuleSettings::with_config(&stored));
    if applied {
        settings_handle.update(|s| s.set_module_config(kind, &stored));
    }
    applied
}
