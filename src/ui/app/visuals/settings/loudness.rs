use super::palette::{PaletteEditor, PaletteEvent};
use super::widgets::{labeled_pick_list, set_if_changed};
use super::{ModuleSettingsPane, SettingsMessage};
use crate::ui::settings::{LoudnessSettings, SettingsHandle};
use crate::ui::theme;
use crate::ui::visualization::loudness::MeterMode;
use crate::ui::visualization::visual_manager::{VisualId, VisualKind, VisualManagerHandle};
use iced::{Element, widget::column};

const PALETTE_LABELS: [&str; 5] = [
    "Background",
    "Left Ch 1",
    "Left Ch 2",
    "Right Fill",
    "Guide Line",
];

#[derive(Debug)]
pub struct LoudnessSettingsPane {
    visual_id: VisualId,
    settings: LoudnessSettings,
    palette: PaletteEditor,
}

#[derive(Debug, Clone)]
pub enum Message {
    LeftMode(MeterMode),
    RightMode(MeterMode),
    Palette(PaletteEvent),
}

pub fn create(visual_id: VisualId, visual_manager: &VisualManagerHandle) -> LoudnessSettingsPane {
    let (settings, palette) = super::load_settings_and_palette(
        visual_manager,
        VisualKind::LOUDNESS,
        &theme::DEFAULT_LOUDNESS_PALETTE,
        &PALETTE_LABELS,
    );

    LoudnessSettingsPane {
        visual_id,
        settings,
        palette,
    }
}

impl ModuleSettingsPane for LoudnessSettingsPane {
    fn visual_id(&self) -> VisualId {
        self.visual_id
    }

    fn view(&self) -> Element<'_, SettingsMessage> {
        column![
            labeled_pick_list(
                "Left meter mode",
                MeterMode::ALL,
                Some(self.settings.left_mode),
                |m| SettingsMessage::Loudness(Message::LeftMode(m))
            ),
            labeled_pick_list(
                "Right meter mode",
                MeterMode::ALL,
                Some(self.settings.right_mode),
                |m| SettingsMessage::Loudness(Message::RightMode(m))
            ),
            super::palette_section(&self.palette, Message::Palette, SettingsMessage::Loudness)
        ]
        .spacing(16)
        .into()
    }

    fn handle(
        &mut self,
        message: &SettingsMessage,
        visual_manager: &VisualManagerHandle,
        settings: &SettingsHandle,
    ) {
        let SettingsMessage::Loudness(msg) = message else {
            return;
        };

        let changed = match msg {
            Message::LeftMode(mode) => set_if_changed(&mut self.settings.left_mode, *mode),
            Message::RightMode(mode) => set_if_changed(&mut self.settings.right_mode, *mode),
            Message::Palette(event) => self.palette.update(*event),
        };

        if changed {
            persist_palette!(
                visual_manager,
                settings,
                VisualKind::LOUDNESS,
                self,
                theme::DEFAULT_LOUDNESS_PALETTE
            );
        }
    }
}
