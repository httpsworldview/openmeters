// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::UiApp;
use super::message::{self, Message};
use crate::persistence::settings::{BarAlignment, BarSettings, clamp_bar_height};
use crate::ui::pages::config::ConfigMessage;
use crate::ui::pages::visuals::{VisualsMessage, create_settings_panel};
use crate::ui::theme;
use crate::visuals::registry::{
    VisualContent, VisualId, VisualKind, VisualMetadata, VisualSnapshot,
};
use iced::widget::{container, mouse_area, text};
use iced::{Element, Length, Size, Task, exit, window};
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer, NewLayerShellSettings};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle};

const WINDOW_MIN_SIZE: Size = Size::new(200.0, 150.0);
pub(super) const SETTINGS_WINDOW_SIZE: Size = Size::new(480.0, 600.0);
pub(super) const MAIN_WINDOW_INITIAL_SIZE: Size = Size::new(420.0, 520.0);

#[derive(Debug, Default)]
struct LayerShellProbe;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for LayerShellProbe {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

pub(super) fn layershell_available() -> bool {
    let Ok(conn) = Connection::connect_to_env() else {
        return false;
    };
    let Ok((globals, _)) = registry_queue_init::<LayerShellProbe>(&conn) else {
        return false;
    };
    globals.contents().with_list(|list| {
        list.iter()
            .any(|global| global.interface == "zwlr_layer_shell_v1")
    })
}

pub(super) fn namespace() -> String {
    "openmeters-ui".into()
}

pub(super) fn bar_anchor(alignment: BarAlignment) -> Anchor {
    match alignment {
        BarAlignment::Top => Anchor::Top | Anchor::Left | Anchor::Right,
        BarAlignment::Bottom => Anchor::Bottom | Anchor::Left | Anchor::Right,
    }
}

fn bar_layershell_settings(alignment: BarAlignment, height: u32) -> NewLayerShellSettings {
    NewLayerShellSettings {
        size: Some((0, height)),
        layer: Layer::Top,
        anchor: bar_anchor(alignment),
        exclusive_zone: Some(height as i32),
        keyboard_interactivity: KeyboardInteractivity::OnDemand,
        ..Default::default()
    }
}

pub(super) fn open_base_window(
    use_layershell: bool,
    size: Size,
    with_decorations: bool,
    transparent: bool,
) -> (window::Id, Task<Message>) {
    if use_layershell {
        let settings = iced_layershell::actions::IcedXdgWindowSettings {
            size: Some((size.width.round() as u32, size.height.round() as u32)),
        };
        message::base_window_open(settings)
    } else {
        let (id, task) = window::open(window::Settings {
            size,
            min_size: Some(WINDOW_MIN_SIZE),
            resizable: true,
            decorations: with_decorations,
            transparent,
            ..Default::default()
        });
        (id, task.map(|_| Message::WindowOpened))
    }
}

pub(super) fn open_main_window(
    use_layershell: bool,
    bar_settings: BarSettings,
    base_size: Size,
    with_decorations: bool,
) -> (window::Id, Task<Message>, bool, Size) {
    if use_layershell && bar_settings.enabled {
        let height = clamp_bar_height(bar_settings.height);
        let settings = bar_layershell_settings(bar_settings.alignment, height);
        let (id, task) = message::layershell_open(settings);
        let new_size = Size::new(base_size.width, height as f32);
        return (id, task, true, new_size);
    }

    let (id, task) = open_base_window(use_layershell, base_size, with_decorations, true);
    (id, task, false, base_size)
}

#[derive(Debug, Clone, Copy)]
pub(super) struct BarResizeState {
    pub start_y: f32,
    pub start_height: u32,
    pub pending_height: u32,
}

#[derive(Debug)]
pub(super) struct PopoutWindow {
    pub visual_id: VisualId,
    pub kind: VisualKind,
    pub original_index: usize,
    pub cached: Option<(VisualMetadata, VisualContent)>,
}

impl PopoutWindow {
    pub fn sync_from_snapshot(&mut self, snapshot: &VisualSnapshot) {
        self.cached = snapshot
            .slots
            .iter()
            .find(|slot| slot.id == self.visual_id && slot.enabled)
            .map(|slot| (slot.metadata, slot.content.clone()));
    }

    pub fn view(&self) -> Element<'_, VisualsMessage> {
        let Some((meta, content)) = &self.cached else {
            return fill!(text("")).into();
        };
        let msg = VisualsMessage::SettingsRequested {
            visual_id: self.visual_id,
            kind: self.kind,
        };
        mouse_area(fill!(content.render(*meta)))
            .on_right_press(msg)
            .into()
    }
}

impl UiApp {
    pub(super) fn refresh_settings_panel(&mut self) {
        let Some((_, panel)) = self.settings_window.as_mut() else {
            return;
        };
        let visual_id = panel.visual_id();
        let snapshot = self.visual_manager.snapshot();
        let Some(kind) = snapshot
            .slots
            .iter()
            .find(|s| s.id == visual_id)
            .map(|s| s.kind)
        else {
            return;
        };
        *panel = create_settings_panel(visual_id, kind, &self.visual_manager);
    }

    pub(super) fn open_settings_window(
        &mut self,
        visual_id: VisualId,
        kind: VisualKind,
    ) -> Task<Message> {
        let new_panel = create_settings_panel(visual_id, kind, &self.visual_manager);
        let previous = self.settings_window.take();
        if previous
            .as_ref()
            .is_some_and(|(_, panel)| panel.visual_id() == visual_id)
        {
            self.settings_window = previous.map(|(id, _)| (id, new_panel));
            return Task::none();
        }
        let (new_id, open_task) =
            open_base_window(self.use_layershell, SETTINGS_WINDOW_SIZE, true, false);
        self.settings_scroll = Default::default();
        self.settings_window = Some((new_id, new_panel));
        match previous {
            Some((old_id, _)) => Task::batch([window::close(old_id), open_task]),
            None => open_task,
        }
    }

    pub(super) fn open_popout_window(
        &mut self,
        visual_id: VisualId,
        kind: VisualKind,
    ) -> Task<Message> {
        if self
            .popout_windows
            .values()
            .any(|popout| popout.visual_id == visual_id)
        {
            return Task::none();
        }
        let snapshot = self.visual_manager.snapshot();
        let Some((index, slot)) = snapshot
            .slots
            .iter()
            .enumerate()
            .find(|(_, s)| s.id == visual_id)
        else {
            return Task::none();
        };
        let window_size = Size::new(
            slot.metadata.preferred_width.max(400.0),
            slot.metadata.preferred_height.max(300.0),
        );
        let (new_id, open_task) = open_base_window(self.use_layershell, window_size, true, true);
        let mut popout = PopoutWindow {
            visual_id,
            kind,
            original_index: index,
            cached: None,
        };
        popout.sync_from_snapshot(&snapshot);
        self.popout_windows.insert(new_id, popout);
        open_task
    }

    pub(super) fn on_window_closed(&mut self, id: window::Id) -> Task<Message> {
        if id == self.main_window_id {
            return exit();
        }
        if self.settings_window.as_ref().is_some_and(|(w, _)| *w == id) {
            self.settings_window = None;
        }
        self.popout_windows.remove(&id);
        Task::none()
    }

    pub(super) fn popped_out_ids(&self) -> Vec<VisualId> {
        self.popout_windows.values().map(|w| w.visual_id).collect()
    }

    pub(super) fn sync_all_windows(&mut self) -> Task<Message> {
        let snapshot = self.visual_manager.snapshot();
        let close_settings_task = self
            .settings_window
            .take_if(|(_, panel)| {
                !snapshot
                    .slots
                    .iter()
                    .any(|slot| slot.id == panel.visual_id() && slot.enabled)
            })
            .map(|(id, _)| window::close::<Message>(id));
        self.popout_windows
            .values_mut()
            .for_each(|popout| popout.sync_from_snapshot(&snapshot));
        let stale_windows: Vec<_> = self
            .popout_windows
            .iter()
            .filter_map(|(id, popout)| popout.cached.is_none().then_some(*id))
            .collect();
        self.popout_windows
            .retain(|_, popout| popout.cached.is_some());
        self.visuals_page
            .apply_snapshot_excluding(snapshot, &self.popped_out_ids());
        Task::batch(
            close_settings_task
                .into_iter()
                .chain(stale_windows.into_iter().map(window::close)),
        )
    }

    pub(super) fn title(&self, window_id: window::Id) -> String {
        if window_id == self.main_window_id {
            return "OpenMeters".into();
        }

        let (visual_id, suffix) = if let Some((_, panel)) = self
            .settings_window
            .as_ref()
            .filter(|(id, _)| *id == window_id)
        {
            (panel.visual_id(), " settings")
        } else if let Some(popout) = self.popout_windows.get(&window_id) {
            (popout.visual_id, "")
        } else {
            return "OpenMeters".into();
        };

        self.visual_manager
            .snapshot()
            .slots
            .iter()
            .find(|s| s.id == visual_id)
            .map_or_else(
                || "OpenMeters".into(),
                |s| format!("{}{} - OpenMeters", s.metadata.display_name, suffix),
            )
    }

    pub(super) fn theme(&self, window_id: window::Id) -> iced::Theme {
        let is_settings = matches!(&self.settings_window, Some((w, _)) if *w == window_id);
        // Settings window forces opaque alpha: it has no wgpu visual backdrop, so a
        // translucent user background would let the desktop bleed through the chrome.
        let custom_bg = (is_settings
            || window_id == self.main_window_id
            || self.popout_windows.contains_key(&window_id))
        .then(|| self.settings_handle.borrow().settings().background_color)
        .flatten()
        .map(|c| {
            let c: iced::Color = c.into();
            if is_settings {
                iced::Color { a: 1.0, ..c }
            } else {
                c
            }
        });
        theme::theme(custom_bg)
    }

    pub(super) fn handle_popout_or_dock(&mut self, source_window: window::Id) -> Task<Message> {
        if let Some(popout) = self.popout_windows.remove(&source_window) {
            self.visual_manager
                .borrow_mut()
                .restore_position(popout.visual_id, popout.original_index);
            self.sync_visuals_page();
            self.settings_handle.update(|settings| {
                settings
                    .set_visual_order(self.visual_manager.snapshot().slots.iter().map(|s| s.kind));
            });
            return window::close(source_window);
        }
        let Some((id, kind)) = self.visuals_page.hovered_visual() else {
            return Task::none();
        };
        let task = self.open_popout_window(id, kind);
        self.sync_visuals_page();
        task
    }

    pub(super) fn sync_visuals_page(&mut self) {
        self.visuals_page
            .apply_snapshot_excluding(self.visual_manager.snapshot(), &self.popped_out_ids());
    }

    pub(super) fn apply_bar_layout(
        &mut self,
        alignment: BarAlignment,
        height: u32,
    ) -> Task<Message> {
        if !self.main_window_is_layer {
            return Task::none();
        }
        let height = clamp_bar_height(height);
        self.main_window_size.height = height as f32;
        Task::batch([
            Task::done(Message::AnchorSizeChange {
                id: self.main_window_id,
                anchor: bar_anchor(alignment),
                size: (0, height),
            }),
            Task::done(Message::ExclusiveZoneChange {
                id: self.main_window_id,
                zone_size: height as i32,
            }),
        ])
    }

    pub(super) fn handle_main_window_resize(
        &mut self,
        window_id: window::Id,
        new_size: Size,
    ) -> Task<Message> {
        if window_id != self.main_window_id {
            return Task::none();
        }

        self.main_window_size = new_size;
        if self.main_window_is_layer {
            let height = clamp_bar_height(new_size.height.round().max(1.0) as u32);
            let current_height = self.settings_handle.borrow().settings().bar.height;
            if current_height != height {
                self.settings_handle.update(|s| s.set_bar_height(height));
            }
            return Task::done(Message::ExclusiveZoneChange {
                id: self.main_window_id,
                zone_size: height as i32,
            });
        }

        self.last_base_window_size = new_size;
        Task::none()
    }

    pub(super) fn recreate_main_window(
        &mut self,
        bar_settings: BarSettings,
        use_decorations: bool,
    ) -> Task<Message> {
        let old_main_id = self.main_window_id;
        let (new_main_id, open_main, main_is_layer, main_size) = open_main_window(
            self.use_layershell,
            bar_settings,
            self.last_base_window_size,
            use_decorations,
        );
        self.main_window_id = new_main_id;
        self.main_window_size = main_size;
        self.main_window_is_layer = main_is_layer;
        self.focused_window = Some(new_main_id);
        Task::batch([open_main, window::close(old_main_id)])
    }

    pub(super) fn handle_bar_config_message(
        &mut self,
        config_msg: &ConfigMessage,
    ) -> Task<Message> {
        if !self.use_layershell {
            return Task::none();
        }
        let bar = self.settings_handle.borrow().settings().bar.clone();
        match config_msg {
            ConfigMessage::BarModeToggled(true) if self.main_window_is_layer => {
                self.apply_bar_layout(bar.alignment, bar.height)
            }
            // Already in the requested state (false/false); no-op.
            ConfigMessage::BarModeToggled(enabled) if *enabled == self.main_window_is_layer => {
                Task::none()
            }
            ConfigMessage::BarModeToggled(enabled) => {
                let decorations = self.settings_handle.borrow().settings().decorations;
                self.recreate_main_window(
                    BarSettings {
                        enabled: *enabled,
                        ..bar
                    },
                    decorations,
                )
            }
            ConfigMessage::BarAlignmentChanged(alignment) if self.main_window_is_layer => {
                self.apply_bar_layout(*alignment, bar.height)
            }
            ConfigMessage::BarHeightChanged(height) if self.main_window_is_layer => {
                self.apply_bar_layout(bar.alignment, *height as u32)
            }
            _ => Task::none(),
        }
    }

    pub(super) fn recreate_settings_window(&mut self) -> Task<Message> {
        let Some((old_id, panel)) = self.settings_window.take() else {
            return Task::none();
        };
        let visual_id = panel.visual_id();
        let snapshot = self.visual_manager.snapshot();
        let Some(slot) = snapshot.slots.iter().find(|s| s.id == visual_id) else {
            return window::close(old_id);
        };
        let (new_id, open_task) =
            open_base_window(self.use_layershell, SETTINGS_WINDOW_SIZE, true, false);
        self.settings_window = Some((
            new_id,
            create_settings_panel(visual_id, slot.kind, &self.visual_manager),
        ));
        Task::batch([open_task, window::close(old_id)])
    }

    pub(super) fn recreate_windows(&mut self, use_decorations: bool) -> Task<Message> {
        let old_main_id = self.main_window_id;
        let (new_main_id, open_main) = open_base_window(
            self.use_layershell,
            self.main_window_size,
            use_decorations,
            true,
        );
        self.main_window_id = new_main_id;
        self.main_window_is_layer = false;
        let settings_task = self.recreate_settings_window();
        Task::batch([open_main, window::close(old_main_id), settings_task])
    }
}
