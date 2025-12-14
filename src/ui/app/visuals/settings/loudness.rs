use super::palette::{PaletteEditor, PaletteEvent};
use super::widgets::{labeled_pick_list, section_title, set_if_changed};
use super::{ModuleSettingsPane, SettingsMessage};
use crate::ui::settings::{
    HasPalette, LoudnessSettings, ModuleSettings, PaletteSettings, SettingsHandle,
};
use crate::ui::theme;
use crate::ui::visualization::loudness::{LOUDNESS_PALETTE_SIZE, MeterMode};
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

#[inline]
fn loud(message: Message) -> SettingsMessage {
    SettingsMessage::Loudness(message)
}

pub fn create(visual_id: VisualId, visual_manager: &VisualManagerHandle) -> LoudnessSettingsPane {
    let settings = visual_manager
        .borrow()
        .module_settings(VisualKind::LOUDNESS)
        .and_then(|stored| stored.config::<LoudnessSettings>())
        .unwrap_or_default();

    let palette = settings
        .palette_array::<LOUDNESS_PALETTE_SIZE>()
        .unwrap_or(theme::DEFAULT_LOUDNESS_PALETTE);

    LoudnessSettingsPane {
        visual_id,
        settings,
        palette: PaletteEditor::with_labels(
            &palette,
            &theme::DEFAULT_LOUDNESS_PALETTE,
            &PALETTE_LABELS,
        ),
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
                |m| loud(Message::LeftMode(m))
            ),
            labeled_pick_list(
                "Right meter mode",
                MeterMode::ALL,
                Some(self.settings.right_mode),
                |m| loud(Message::RightMode(m))
            ),
            column![
                section_title("Colors"),
                self.palette.view().map(|e| loud(Message::Palette(e)))
            ]
            .spacing(8)
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
            self.persist(visual_manager, settings);
        }
    }
}

impl LoudnessSettingsPane {
    fn persist(&self, vm: &VisualManagerHandle, settings: &SettingsHandle) {
        let mut stored = self.settings.clone();
        stored.palette = PaletteSettings::maybe_from_colors(
            self.palette.colors(),
            &theme::DEFAULT_LOUDNESS_PALETTE,
        );
        if vm
            .borrow_mut()
            .apply_module_settings(VisualKind::LOUDNESS, &ModuleSettings::with_config(&stored))
        {
            settings.update(|m| m.set_module_config(VisualKind::LOUDNESS, &stored));
        }
    }
}
