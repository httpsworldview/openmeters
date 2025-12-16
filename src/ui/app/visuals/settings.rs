//! Contains the settings panes for visual modules.

macro_rules! persist_palette {
    ($vm:expr, $settings:expr, $kind:expr, $this:expr, $defaults:expr) => {{
        super::persist_with_palette(
            $vm,
            $settings,
            $kind,
            &$this.settings,
            &$this.palette,
            &$defaults,
        );
    }};
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
        settings: &SettingsHandle,
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
        settings: &SettingsHandle,
    ) {
        self.pane.handle(message, visual_manager, settings);
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
    settings: &SettingsHandle,
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
        settings.update(|s| s.set_module_config(kind, config));
    }

    applied
}

pub(super) fn load_settings_and_palette<T, const N: usize>(
    visual_manager: &VisualManagerHandle,
    kind: VisualKind,
    defaults: &[Color; N],
    labels: &[&'static str],
) -> (T, PaletteEditor)
where
    T: DeserializeOwned + Default + HasPalette,
{
    let settings = load_config_or_default::<T>(visual_manager, kind);
    let current = settings.palette_as_array::<N>().unwrap_or(*defaults);
    let palette = if labels.is_empty() {
        PaletteEditor::new(&current, defaults)
    } else {
        PaletteEditor::with_labels(&current, defaults, labels)
    };
    (settings, palette)
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
    settings: &SettingsHandle,
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
    persist_module_config(visual_manager, settings, kind, &stored)
}
