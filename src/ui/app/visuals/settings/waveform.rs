use super::palette::{PaletteEditor, PaletteEvent};
use super::widgets::{
    SliderRange, labeled_pick_list, labeled_slider, section_title, set_f32, set_if_changed,
};
use super::{ModuleSettingsPane, SettingsMessage};
use crate::dsp::waveform::{DownsampleStrategy, MAX_SCROLL_SPEED, MIN_SCROLL_SPEED};
use crate::ui::settings::{
    ModuleSettings, PaletteSettings, SettingsHandle, WaveformChannelMode, WaveformSettings,
};
use crate::ui::theme;
use crate::ui::visualization::visual_manager::{VisualId, VisualKind, VisualManagerHandle};
use iced::Element;
use iced::widget::{column, pick_list};
use std::fmt;

#[derive(Debug)]
pub struct WaveformSettingsPane {
    visual_id: VisualId,
    settings: WaveformSettings,
    palette: PaletteEditor,
}

impl fmt::Display for DownsampleStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DownsampleStrategy::MinMax => "Min/Max",
            DownsampleStrategy::Average => "Average",
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Message {
    ScrollSpeed(f32),
    Downsample(DownsampleStrategy),
    ChannelMode(WaveformChannelMode),
    Palette(PaletteEvent),
}

pub fn create(visual_id: VisualId, visual_manager: &VisualManagerHandle) -> WaveformSettingsPane {
    let settings = visual_manager
        .borrow()
        .module_settings(VisualKind::WAVEFORM)
        .and_then(|s| s.config::<WaveformSettings>())
        .unwrap_or_default();

    let palette = settings
        .palette_array::<{ theme::DEFAULT_WAVEFORM_PALETTE.len() }>()
        .unwrap_or(theme::DEFAULT_WAVEFORM_PALETTE);

    WaveformSettingsPane {
        visual_id,
        settings,
        palette: PaletteEditor::new(&palette, &theme::DEFAULT_WAVEFORM_PALETTE),
    }
}

impl ModuleSettingsPane for WaveformSettingsPane {
    fn visual_id(&self) -> VisualId {
        self.visual_id
    }

    fn view(&self) -> Element<'_, SettingsMessage> {
        column![
            labeled_slider(
                "Scroll speed",
                self.settings.scroll_speed,
                format!("{:.0} px/s", self.settings.scroll_speed),
                SliderRange::new(MIN_SCROLL_SPEED, MAX_SCROLL_SPEED, 1.0),
                |v| SettingsMessage::Waveform(Message::ScrollSpeed(v)),
            ),
            labeled_pick_list(
                "Channels",
                &[
                    WaveformChannelMode::Both,
                    WaveformChannelMode::Left,
                    WaveformChannelMode::Right,
                    WaveformChannelMode::Mono,
                ],
                Some(self.settings.channel_mode),
                |m| SettingsMessage::Waveform(Message::ChannelMode(m)),
            ),
            column![
                section_title("Downsampling strategy"),
                pick_list(
                    [DownsampleStrategy::MinMax, DownsampleStrategy::Average],
                    Some(self.settings.downsample),
                    |v| SettingsMessage::Waveform(Message::Downsample(v))
                )
                .text_size(14)
            ]
            .spacing(8),
            column![
                section_title("Colors"),
                self.palette
                    .view()
                    .map(|e| SettingsMessage::Waveform(Message::Palette(e)))
            ]
            .spacing(8)
        ]
        .spacing(16)
        .into()
    }

    fn handle(&mut self, message: &SettingsMessage, vm: &VisualManagerHandle, s: &SettingsHandle) {
        let SettingsMessage::Waveform(msg) = message else {
            return;
        };
        let changed = match *msg {
            Message::ScrollSpeed(v) => set_f32(
                &mut self.settings.scroll_speed,
                v.clamp(MIN_SCROLL_SPEED, MAX_SCROLL_SPEED),
            ),
            Message::Downsample(d) => set_if_changed(&mut self.settings.downsample, d),
            Message::ChannelMode(m) => set_if_changed(&mut self.settings.channel_mode, m),
            Message::Palette(e) => self.palette.update(e),
        };
        if changed {
            self.settings.palette = PaletteSettings::maybe_from_colors(
                self.palette.colors(),
                &theme::DEFAULT_WAVEFORM_PALETTE,
            );
            if vm
                .borrow_mut()
                .apply_module_settings(VisualKind::WAVEFORM, &ModuleSettings::with_config(&self.settings))
            {
                s.update(|m| m.set_module_config(VisualKind::WAVEFORM, &self.settings));
            }
        }
    }
}
