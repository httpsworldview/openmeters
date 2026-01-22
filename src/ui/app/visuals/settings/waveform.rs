use super::palette::{PaletteEditor, PaletteEvent};
use super::widgets::{
    SliderRange, labeled_pick_list, labeled_slider, set_if_changed, update_f32_range,
};
use super::{CHANNEL_OPTIONS, SettingsMessage};
use crate::dsp::waveform::{MAX_SCROLL_SPEED, MIN_SCROLL_SPEED};
use crate::ui::settings::{ChannelMode, SettingsHandle, WaveformColorMode, WaveformSettings};
use crate::ui::theme;
use crate::ui::visualization::visual_manager::{VisualKind, VisualManagerHandle};
use iced::Element;
use iced::widget::column;

settings_pane!(
    WaveformSettingsPane,
    WaveformSettings,
    VisualKind::Waveform,
    theme::waveform,
    init_palette(settings, palette) {
        configure_palette_for_mode(&mut palette, settings.color_mode);
    }
);

const SCROLL_SPEED_RANGE: SliderRange = SliderRange::new(MIN_SCROLL_SPEED, MAX_SCROLL_SPEED, 1.0);

const COLOR_MODE_OPTIONS: [WaveformColorMode; 3] = [
    WaveformColorMode::Frequency,
    WaveformColorMode::Loudness,
    WaveformColorMode::Static,
];

fn configure_palette_for_mode(palette: &mut PaletteEditor, mode: WaveformColorMode) {
    match mode {
        WaveformColorMode::Static => {
            palette.set_visible_indices(Some(vec![0]));
            palette.set_label_overrides(vec![(0, "Color")]);
        }
        WaveformColorMode::Loudness => {
            palette.set_visible_indices(None);
            palette.set_label_overrides(vec![(0, "Quiet"), (5, "Loud")]);
        }
        WaveformColorMode::Frequency => {
            palette.set_visible_indices(None);
            palette.set_label_overrides(vec![]);
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Message {
    ScrollSpeed(f32),
    ChannelMode(ChannelMode),
    ColorMode(WaveformColorMode),
    Palette(PaletteEvent),
}
impl WaveformSettingsPane {
    fn view(&self) -> Element<'_, SettingsMessage> {
        column![
            labeled_slider(
                "Scroll speed",
                self.settings.scroll_speed,
                format!("{:.0} px/s", self.settings.scroll_speed),
                SCROLL_SPEED_RANGE,
                |v| SettingsMessage::Waveform(Message::ScrollSpeed(v)),
            ),
            labeled_pick_list(
                "Channels",
                &CHANNEL_OPTIONS,
                Some(self.settings.channel_mode),
                |m| SettingsMessage::Waveform(Message::ChannelMode(m))
            ),
            labeled_pick_list(
                "Color mode",
                &COLOR_MODE_OPTIONS,
                Some(self.settings.color_mode),
                |m| SettingsMessage::Waveform(Message::ColorMode(m))
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
            Message::ScrollSpeed(v) => {
                update_f32_range(&mut self.settings.scroll_speed, v, SCROLL_SPEED_RANGE)
            }
            Message::ChannelMode(m) => set_if_changed(&mut self.settings.channel_mode, m),
            Message::ColorMode(m) => {
                let changed = set_if_changed(&mut self.settings.color_mode, m);
                if changed {
                    configure_palette_for_mode(&mut self.palette, m);
                }
                changed
            }
            Message::Palette(e) => self.palette.update(e),
        };
        if changed {
            persist_palette!(
                visual_manager,
                settings_handle,
                VisualKind::Waveform,
                self,
                &theme::waveform::COLORS
            );
        }
    }
}
