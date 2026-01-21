//! Contains the settings panes for visual modules.

macro_rules! persist_palette {
    ($visual_manager:expr, $settings_handle:expr, $kind:expr, $this:expr, $defaults:expr) => {{
        super::persist_with_palette(
            $visual_manager,
            $settings_handle,
            $kind,
            &$this.settings,
            &$this.palette,
            $defaults,
        );
    }};
}

/// Generates settings pane struct, create function, and trait impl.
/// Use `extra_from_settings` for fields computed from loaded settings during init.
/// Use `init_palette` to run initialization code on the palette after loading.
macro_rules! settings_pane {
    // Branch with extra fields computed from settings
    (
        $pane:ident, $settings_ty:ty, $kind:expr, $palette_mod:path
        , extra_from_settings($s:ident) { $($field:ident : $ty:ty = $init:expr),* $(,)? }
    ) => {
        #[derive(Debug)]
        pub struct $pane {
            visual_id: super::VisualId,
            settings: $settings_ty,
            palette: super::palette::PaletteEditor,
            $($field: $ty,)*
        }

        pub fn create(
            visual_id: super::VisualId,
            visual_manager: &super::VisualManagerHandle,
        ) -> $pane {
            use $palette_mod as pal;
            let ($s, palette) = super::load_settings_and_palette::<$settings_ty>(
                visual_manager, $kind, &pal::COLORS, pal::LABELS,
            );
            $(let $field: $ty = $init;)*
            $pane { visual_id, settings: $s, palette, $($field,)* }
        }

        settings_pane!(@impl $pane);
    };
    // Branch with palette init callback
    (
        $pane:ident, $settings_ty:ty, $kind:expr, $palette_mod:path
        , init_palette($s:ident, $p:ident) $init_body:block
    ) => {
        #[derive(Debug)]
        pub struct $pane {
            visual_id: super::VisualId,
            settings: $settings_ty,
            palette: super::palette::PaletteEditor,
        }

        pub fn create(
            visual_id: super::VisualId,
            visual_manager: &super::VisualManagerHandle,
        ) -> $pane {
            use $palette_mod as pal;
            let ($s, mut $p) = super::load_settings_and_palette::<$settings_ty>(
                visual_manager, $kind, &pal::COLORS, pal::LABELS,
            );
            $init_body
            $pane { visual_id, settings: $s, palette: $p }
        }

        settings_pane!(@impl $pane);
    };
    // Branch without extra fields
    (
        $pane:ident, $settings_ty:ty, $kind:expr, $palette_mod:path
    ) => {
        #[derive(Debug)]
        pub struct $pane {
            visual_id: super::VisualId,
            settings: $settings_ty,
            palette: super::palette::PaletteEditor,
        }

        pub fn create(
            visual_id: super::VisualId,
            visual_manager: &super::VisualManagerHandle,
        ) -> $pane {
            use $palette_mod as pal;
            let (settings, palette) = super::load_settings_and_palette::<$settings_ty>(
                visual_manager, $kind, &pal::COLORS, pal::LABELS,
            );
            $pane { visual_id, settings, palette }
        }

        settings_pane!(@impl $pane);
    };
    (@impl $pane:ident) => {
        impl super::ModuleSettingsPane for $pane {
            fn visual_id(&self) -> super::VisualId { self.visual_id }
            fn view(&self) -> iced::Element<'_, super::SettingsMessage> {
                $pane::view(self)
            }
            fn handle(
                &mut self,
                message: &super::SettingsMessage,
                visual_manager: &super::VisualManagerHandle,
                settings_handle: &crate::ui::settings::SettingsHandle,
            ) {
                $pane::handle(self, message, visual_manager, settings_handle)
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
use serde::Serialize;
use serde::de::DeserializeOwned;

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
    pub fn new(pane: Box<dyn ModuleSettingsPane>) -> Self {
        Self { pane }
    }
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
    let pane: Box<dyn ModuleSettingsPane> = match kind {
        VisualKind::Loudness => Box::new(loudness::create(visual_id, visual_manager)),
        VisualKind::Oscilloscope => Box::new(oscilloscope::create(visual_id, visual_manager)),
        VisualKind::Spectrogram => Box::new(spectrogram::create(visual_id, visual_manager)),
        VisualKind::Spectrum => Box::new(spectrum::create(visual_id, visual_manager)),
        VisualKind::Stereometer => Box::new(stereometer::create(visual_id, visual_manager)),
        VisualKind::Waveform => Box::new(waveform::create(visual_id, visual_manager)),
    };

    ActiveSettings::new(pane)
}

pub(super) fn load_config_or_default<T>(visual_manager: &VisualManagerHandle, kind: VisualKind) -> T
where
    T: DeserializeOwned + Default,
{
    visual_manager
        .borrow()
        .module_settings(kind)
        .and_then(|stored| stored.parse_config::<T>())
        .unwrap_or_default()
}

pub(super) fn persist_module_config<T>(
    visual_manager: &VisualManagerHandle,
    settings_handle: &SettingsHandle,
    kind: VisualKind,
    config: &T,
) -> bool
where
    T: Serialize,
{
    let applied = visual_manager
        .borrow_mut()
        .apply_module_settings(kind, &ModuleSettings::with_config(config));

    if applied {
        settings_handle.update(|s| s.set_module_config(kind, config));
    }

    applied
}

pub(super) fn load_settings_and_palette<T>(
    visual_manager: &VisualManagerHandle,
    kind: VisualKind,
    defaults: &'static [Color],
    labels: &'static [&'static str],
) -> (T, PaletteEditor)
where
    T: DeserializeOwned + Default + HasPalette,
{
    let settings = load_config_or_default::<T>(visual_manager, kind);
    let mut palette = Palette::new(defaults, labels);
    if let Some(stored) = settings.palette() {
        let colors: Vec<Color> = stored.stops.iter().map(|c| (*c).into()).collect();
        palette.set(&colors);
    }
    (settings, PaletteEditor::new(palette))
}

pub(super) fn palette_section<'a, M>(
    palette: &'a PaletteEditor,
    map: fn(PaletteEvent) -> M,
    wrap: fn(M) -> SettingsMessage,
) -> iced::widget::Column<'a, SettingsMessage>
where
    M: 'a,
{
    column![
        widgets::section_title("Colors"),
        palette.view().map(map).map(wrap)
    ]
    .spacing(8)
}

pub(super) fn persist_with_palette<T>(
    visual_manager: &VisualManagerHandle,
    settings_handle: &SettingsHandle,
    kind: VisualKind,
    config: &T,
    palette: &PaletteEditor,
    defaults: &[Color],
) -> bool
where
    T: Clone + Serialize + HasPalette,
{
    let mut stored = config.clone();
    stored.set_palette(PaletteSettings::if_differs_from(palette.colors(), defaults));
    persist_module_config(visual_manager, settings_handle, kind, &stored)
}
