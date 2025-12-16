use super::palette::{PaletteEditor, PaletteEvent};
use super::widgets::{
    SliderRange, labeled_pick_list, labeled_slider, section_title, set_f32, set_if_changed,
};
use super::{ModuleSettingsPane, SettingsMessage};
use crate::dsp::waveform::{DownsampleStrategy, MAX_SCROLL_SPEED, MIN_SCROLL_SPEED};
use crate::ui::settings::{ChannelMode, SettingsHandle, WaveformSettings};
use crate::ui::theme;
use crate::ui::visualization::visual_manager::{VisualId, VisualKind, VisualManagerHandle};
use iced::Element;
use iced::widget::{column, pick_list};

const CHANNEL_OPTIONS: [ChannelMode; 4] = [
    ChannelMode::Both,
    ChannelMode::Left,
    ChannelMode::Right,
    ChannelMode::Mono,
];

#[derive(Debug)]
pub struct WaveformSettingsPane {
    visual_id: VisualId,
    settings: WaveformSettings,
    palette: PaletteEditor,
}

#[derive(Debug, Clone, Copy)]
pub enum Message {
    ScrollSpeed(f32),
    Downsample(DownsampleStrategy),
    ChannelMode(ChannelMode),
    Palette(PaletteEvent),
}

pub fn create(visual_id: VisualId, visual_manager: &VisualManagerHandle) -> WaveformSettingsPane {
    let (settings, palette) = super::load_settings_and_palette(
        visual_manager,
        VisualKind::Waveform,
        &theme::DEFAULT_WAVEFORM_PALETTE,
        &[],
    );

    WaveformSettingsPane {
        visual_id,
        settings,
        palette,
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
                &CHANNEL_OPTIONS,
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
            super::palette_section(&self.palette, Message::Palette, SettingsMessage::Waveform)
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
            persist_palette!(
                vm,
                s,
                VisualKind::Waveform,
                self,
                theme::DEFAULT_WAVEFORM_PALETTE
            );
        }
    }
}
