// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{Message, UiApp};
use crate::domain::visuals::VisualKind;
use crate::persistence::settings::Snapshot;
use crate::ui::settings::ActiveSettings;
use iced::{Task, window};

impl UiApp {
    pub(super) fn apply_settings_changes(&mut self) -> Option<Task<Message>> {
        let mut tasks = Vec::new();
        // Committing a closing panel's palette can start a theme: a change applied in turn.
        while let Some((old, new)) = self.settings_handle.take_change() {
            tasks.push(self.apply_settings(&old, &new));
        }
        (!tasks.is_empty()).then(|| Task::batch(tasks))
    }

    fn apply_settings(&mut self, old: &Snapshot, new: &Snapshot) -> Task<Message> {
        let (was, now) = (&old.settings, &new.settings);
        if was.capture_config() != now.capture_config() {
            self.config_page.dispatch_capture_config();
        }
        if was.visual_frame_rate != now.visual_frame_rate {
            self.frames.borrow_mut().set_rate(now.visual_frame_rate);
        }
        if was.theme != now.theme {
            self.config_page.refresh_theme_choices();
        }
        if old.theme.background != new.theme.background {
            self.config_page.show_background();
        }
        let mut manager = self.visual_manager.borrow_mut();
        manager.apply_visual_settings(&now.visuals, Some(&was.visuals));
        manager.apply_palettes(&new.theme, Some(&old.theme));
        drop(manager);
        let toggled =
            |&kind: &VisualKind| was.visuals.is_enabled(kind) != now.visuals.is_enabled(kind);
        if VisualKind::ALL.iter().any(toggled) {
            self.frames.borrow_mut().set_active(self.visuals_active());
        }
        Task::batch([
            self.sync_main_window(was, now),
            self.sync_visual_windows(was, now),
            self.sync_settings_panel(old, new),
        ])
    }

    fn sync_settings_panel(&mut self, old: &Snapshot, new: &Snapshot) -> Task<Message> {
        let visuals = &new.settings.visuals;
        let closed = self.take_settings_if(|(_, panel)| !visuals.is_enabled(panel.kind()));
        if let Some((id, _)) = closed {
            return window::close(id);
        }
        let Some((_, panel)) = &mut self.settings_window else {
            return Task::none();
        };
        let kind = panel.kind();
        // Outside palette changes win over unfinished panel edits
        if old.theme.palettes.get(&kind) == new.theme.palettes.get(&kind) {
            if panel.config() == visuals.config(kind) {
                return Task::none();
            }
            panel.commit_edits(&self.visual_manager, &self.settings_handle);
        }
        *panel = ActiveSettings::open(kind, &self.settings_handle);
        Task::none()
    }
}
