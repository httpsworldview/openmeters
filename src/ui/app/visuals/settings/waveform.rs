use super::palette::PaletteEvent;
use super::widgets::{SliderRange, labeled_pick_list, labeled_slider, set_f32, set_if_changed};
use super::{CHANNEL_OPTIONS, SettingsMessage};
use crate::dsp::waveform::{MAX_SCROLL_SPEED, MIN_SCROLL_SPEED};
use crate::ui::settings::{ChannelMode, SettingsHandle, WaveformSettings};
use crate::ui::theme;
use crate::ui::visualization::visual_manager::{VisualKind, VisualManagerHandle};
use iced::Element;
use iced::widget::column;

settings_pane!(
    WaveformSettingsPane,
    WaveformSettings,
    VisualKind::Waveform,
    theme::DEFAULT_WAVEFORM_PALETTE
);

#[derive(Debug, Clone, Copy)]
pub enum Message {
    ScrollSpeed(f32),
    ChannelMode(ChannelMode),
    Palette(PaletteEvent),
}

impl WaveformSettingsPane {
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
                |m| SettingsMessage::Waveform(Message::ChannelMode(m))
            ),
            super::palette_section(&self.palette, Message::Palette, SettingsMessage::Waveform)
        ]
        .spacing(16)
        .into()
    }

    fn handle(
        &mut self,
        message: &SettingsMessage,
        visual_manager: &VisualManagerHandle,
        settings_handle: &SettingsHandle,
    ) {
        let SettingsMessage::Waveform(msg) = message else {
            return;
        };
        let changed = match *msg {
            Message::ScrollSpeed(v) => set_f32(
                &mut self.settings.scroll_speed,
                v.clamp(MIN_SCROLL_SPEED, MAX_SCROLL_SPEED),
            ),
            Message::ChannelMode(m) => set_if_changed(&mut self.settings.channel_mode, m),
            Message::Palette(e) => self.palette.update(e),
        };
        if changed {
            persist_palette!(
                visual_manager,
                settings_handle,
                VisualKind::Waveform,
                self,
                theme::DEFAULT_WAVEFORM_PALETTE
            );
        }
    }
}
