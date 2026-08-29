// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::message::{self, Message};
use super::{ActiveSettings, UiApp};
use crate::persistence::settings::{
    BarAlignment, BarSettings, MainWindowSettings, PopoutWindowSettings, clamp_bar_height,
};
use crate::ui::config::BarChange;
use crate::ui::visuals::VisualsMessage;
use crate::ui::widgets::{fill, scroll_glow::ScrollGlow};
use crate::visuals::registry::{VisualContent, VisualKind, VisualSlotSnapshot};
use iced::widget::mouse_area;
use iced::{Element, Size, Task, exit, window};
use iced_layershell::reexport::{
    Anchor, KeyboardInteractivity, Layer, LayerSize, NewLayerShellSettings, OutputOption, PixelSize,
};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle};

pub(super) const APP_ID: &str = "openmeters-ui";
const WINDOW_MIN_SIZE: Size = Size::new(200.0, 150.0);
const TOOL_WINDOW_SIZE: Size = Size::new(480.0, 600.0);

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

pub(super) fn bar_anchor(alignment: BarAlignment) -> Anchor {
    match alignment {
        BarAlignment::Top => Anchor::Top | Anchor::Left | Anchor::Right,
        BarAlignment::Bottom => Anchor::Bottom | Anchor::Left | Anchor::Right,
    }
}

fn clamp_window_size(size: Size) -> Size {
    Size::new(
        size.width.max(WINDOW_MIN_SIZE.width),
        size.height.max(WINDOW_MIN_SIZE.height),
    )
}

fn persisted_window_size(size: Size) -> (u32, u32) {
    let size = clamp_window_size(size);
    (size.width.round() as u32, size.height.round() as u32)
}

pub(super) fn main_window_size(settings: MainWindowSettings) -> Size {
    clamp_window_size(Size::new(settings.width as f32, settings.height as f32))
}

fn open_base_window(
    layershell: bool,
    size: Size,
    decorations: bool,
) -> (window::Id, Task<Message>) {
    if layershell {
        let (width, height) = persisted_window_size(size);
        let settings = iced_layershell::actions::IcedXdgWindowSettings {
            size: Some(PixelSize::px(width, height)),
            client_side_decorations: !decorations,
        };
        message::base_window_open(settings)
    } else {
        let (id, task) = window::open(window::Settings {
            size,
            min_size: Some(WINDOW_MIN_SIZE),
            resizable: true,
            decorations,
            // Keep one alpha mode across base windows; visual windows need it for background opacity.
            transparent: true,
            ..Default::default()
        });
        (id, task.discard())
    }
}

pub(super) fn open_tool_base_window(use_layershell: bool) -> (window::Id, Task<Message>) {
    open_base_window(use_layershell, TOOL_WINDOW_SIZE, true)
}

pub(super) fn open_main_window(
    use_layershell: bool,
    bar_settings: BarSettings,
    base_size: Size,
    with_decorations: bool,
) -> (window::Id, Task<Message>, bool, Size) {
    if use_layershell && bar_settings.enabled {
        let height = clamp_bar_height(bar_settings.height);
        let (id, task) = message::layershell_open(NewLayerShellSettings {
            size: LayerSize::fill_width(height),
            layer: Layer::Top,
            anchor: bar_anchor(bar_settings.alignment),
            exclusive_zone: Some(height as i32),
            keyboard_interactivity: KeyboardInteractivity::OnDemand,
            output_option: bar_settings
                .monitor
                .map(OutputOption::OutputName)
                .unwrap_or_default(),
            ..Default::default()
        });
        let new_size = Size::new(base_size.width, height as f32);
        return (id, task, true, new_size);
    }

    let (id, task) = open_base_window(use_layershell, base_size, with_decorations);
    (id, task, false, base_size)
}

fn popout_window_settings(size: Size, popped_out: bool) -> PopoutWindowSettings {
    let (width, height) = persisted_window_size(size);
    PopoutWindowSettings {
        width,
        height,
        popped_out,
    }
}

pub(super) struct BarResizeState {
    pub start_y: f32,
    pub start_height: u32,
    pub pending_height: u32,
}

pub(super) struct PopoutWindow {
    pub kind: VisualKind,
    pub original_index: usize,
    pub size: Size,
    pub content: VisualContent,
}

impl PopoutWindow {
    pub fn view(&self) -> Element<'_, VisualsMessage> {
        let msg = VisualsMessage::SettingsRequested(self.kind);
        mouse_area(fill(self.content.render()))
            .on_right_press(msg)
            .into()
    }
}

pub(super) enum AppWindow<'a> {
    Main,
    Config,
    Settings(&'a ActiveSettings),
    Popout(&'a PopoutWindow),
    Unknown,
}

impl UiApp {
    pub(super) fn window(&self, id: window::Id) -> AppWindow<'_> {
        if id == self.main_window_id {
            AppWindow::Main
        } else if self.config_window == Some(id) {
            AppWindow::Config
        } else if let Some((_, panel)) = self.settings_window.as_ref().filter(|(wid, _)| *wid == id)
        {
            AppWindow::Settings(panel)
        } else {
            self.popout_windows
                .get(&id)
                .map_or(AppWindow::Unknown, AppWindow::Popout)
        }
    }

    pub(super) fn open_settings_window(&mut self, kind: VisualKind) -> Task<Message> {
        let panel = ActiveSettings::new(kind, &self.visual_manager);
        let previous = self.settings_window.take();
        if previous
            .as_ref()
            .is_some_and(|(_, current)| current.kind() == kind)
        {
            self.settings_window = previous.map(|(id, _)| (id, panel));
            return Task::none();
        }

        let (id, open) = open_tool_base_window(self.use_layershell);
        self.settings_scroll = ScrollGlow::default();
        self.settings_window = Some((id, panel));
        match previous {
            Some((old, _)) => Task::batch([window::close(old), open]),
            None => open,
        }
    }

    fn saved_popout(&self, kind: VisualKind) -> Option<PopoutWindowSettings> {
        self.settings_handle
            .borrow()
            .data
            .visuals
            .popouts
            .get(&kind)
            .copied()
    }

    fn create_popout_window(
        &mut self,
        kind: VisualKind,
        saved_size: Option<PopoutWindowSettings>,
    ) -> Option<(PopoutWindowSettings, Task<Message>)> {
        if self
            .popout_windows
            .values()
            .any(|popout| popout.kind == kind)
        {
            return None;
        }
        let snapshot = self.visual_manager.borrow().snapshot();
        let (index, slot) = snapshot
            .iter()
            .enumerate()
            .find(|(_, slot)| slot.kind == kind && slot.enabled)?;
        let saved = saved_size.unwrap_or_default();
        let dim = |saved: u32, default| if saved > 0 { saved as f32 } else { default };
        let window_size =
            clamp_window_size(Size::new(dim(saved.width, 400.0), dim(saved.height, 300.0)));
        let use_decorations = self.settings_handle.borrow().data.decorations;
        let (new_id, open_task) =
            open_base_window(self.use_layershell, window_size, use_decorations);
        let popout = PopoutWindow {
            kind,
            original_index: index,
            size: window_size,
            content: slot.content.clone(),
        };
        self.popout_windows.insert(new_id, popout);
        Some((popout_window_settings(window_size, true), open_task))
    }

    pub(super) fn restore_popout_windows(
        &mut self,
        saved: &std::collections::BTreeMap<VisualKind, PopoutWindowSettings>,
    ) -> Task<Message> {
        let order = self.visual_manager.borrow().order();
        Task::batch(order.into_iter().filter_map(|kind| {
            let settings = saved.get(&kind).copied().filter(|s| s.popped_out)?;
            self.create_popout_window(kind, Some(settings))
                .map(|(_, task)| task)
        }))
    }

    pub(super) fn restore_popout_window(&mut self, kind: VisualKind) -> Task<Message> {
        let saved = self
            .saved_popout(kind)
            .filter(|settings| settings.popped_out);
        let Some(settings) = saved else {
            return Task::none();
        };
        self.create_popout_window(kind, Some(settings))
            .map_or_else(Task::none, |(_, task)| task)
    }

    fn open_popout_window(&mut self, kind: VisualKind) -> Task<Message> {
        let saved_size = self.saved_popout(kind);
        let Some((settings, task)) = self.create_popout_window(kind, saved_size) else {
            return Task::none();
        };
        self.settings_handle.update(|s| {
            s.data.visuals.popouts.insert(kind, settings);
        });
        task
    }

    fn dock_popout(&mut self, popout: PopoutWindow) {
        let order = {
            let mut manager = self.visual_manager.borrow_mut();
            manager.move_to(popout.kind, popout.original_index);
            manager.order()
        };
        let popout_settings = popout_window_settings(popout.size, false);
        self.sync_visuals_page();
        self.settings_handle.update(|settings| {
            settings
                .data
                .visuals
                .popouts
                .insert(popout.kind, popout_settings);
            settings.data.visuals.order = order;
        });
    }

    pub(super) fn on_window_closed(&mut self, id: window::Id) -> Task<Message> {
        if id == self.main_window_id {
            return exit();
        }
        let _ = self.config_window.take_if(|window| *window == id);
        let _ = self.settings_window.take_if(|(window, _)| *window == id);
        if let Some(popout) = self.popout_windows.remove(&id) {
            self.dock_popout(popout);
        }
        Task::none()
    }

    pub(super) fn sync_all_windows(&mut self) -> Task<Message> {
        let snapshot = self.visual_manager.borrow().snapshot();
        let close_settings_task = self
            .settings_window
            .take_if(|(_, panel)| {
                !snapshot
                    .iter()
                    .any(|slot| slot.kind == panel.kind() && slot.enabled)
            })
            .map(|(id, _)| window::close::<Message>(id));
        let stale_windows: Vec<_> = self
            .popout_windows
            .extract_if(|_, popout| {
                !snapshot
                    .iter()
                    .any(|slot| slot.kind == popout.kind && slot.enabled)
            })
            .map(|(id, popout)| (id, popout.kind, popout.size))
            .collect();
        // keep disabled popouts restorable when re-enabled.
        if !stale_windows.is_empty() {
            self.settings_handle.update(|settings| {
                for (_, kind, size) in &stale_windows {
                    settings
                        .data
                        .visuals
                        .popouts
                        .insert(*kind, popout_window_settings(*size, true));
                }
            });
        }
        self.apply_visual_snapshot(&snapshot);
        Task::batch(
            close_settings_task.into_iter().chain(
                stale_windows
                    .into_iter()
                    .map(|(id, _, _)| window::close(id)),
            ),
        )
    }

    pub(super) fn title(&self, window_id: window::Id) -> String {
        match self.window(window_id) {
            AppWindow::Config => "Configuration - OpenMeters".into(),
            AppWindow::Settings(panel) => format!("{} settings - OpenMeters", panel.kind().label()),
            AppWindow::Popout(popout) => format!("{} - OpenMeters", popout.kind.label()),
            AppWindow::Main | AppWindow::Unknown => "OpenMeters".into(),
        }
    }

    pub(super) fn theme(&self, window_id: window::Id) -> iced::Theme {
        let [fallback, visual, tool] = &self.config_page.window_themes;
        match self.window(window_id) {
            AppWindow::Config | AppWindow::Settings(_) => tool,
            AppWindow::Main | AppWindow::Popout(_) => visual,
            AppWindow::Unknown => fallback,
        }
        .clone()
    }

    pub(super) fn handle_popout_or_dock(&mut self, source_window: window::Id) -> Task<Message> {
        if let Some(popout) = self.popout_windows.remove(&source_window) {
            self.dock_popout(popout);
            return window::close(source_window);
        }
        let Some(kind) = self.visuals_page.hovered_visual() else {
            return Task::none();
        };
        let task = self.open_popout_window(kind);
        self.sync_visuals_page();
        task
    }

    fn apply_visual_snapshot(&mut self, snapshot: &[VisualSlotSnapshot]) {
        self.visuals_page
            .apply_snapshot_excluding(snapshot, |kind| {
                self.popout_windows
                    .values()
                    .any(|window| window.kind == kind)
            });
    }

    pub(super) fn sync_visuals_page(&mut self) {
        let snapshot = self.visual_manager.borrow().snapshot();
        self.apply_visual_snapshot(&snapshot);
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
            Task::done(Message::LayoutChange {
                id: self.main_window_id,
                anchor: bar_anchor(alignment),
                size: LayerSize::fill_width(height),
            }),
            Task::done(Message::ExclusiveZoneChange {
                id: self.main_window_id,
                zone_size: height as i32,
            }),
        ])
    }

    pub(super) fn handle_window_resize(
        &mut self,
        window_id: window::Id,
        new_size: Size,
    ) -> Task<Message> {
        if let Some(popout) = self.popout_windows.get_mut(&window_id) {
            let settings = popout_window_settings(new_size, true);
            if popout_window_settings(popout.size, true) != settings {
                popout.size = Size::new(settings.width as f32, settings.height as f32);
                let kind = popout.kind;
                self.settings_handle.update(|s| {
                    s.data.visuals.popouts.insert(kind, settings);
                });
            }
            return Task::none();
        }
        if window_id != self.main_window_id {
            return Task::none();
        }

        if self.main_window_is_layer {
            self.main_window_size = new_size;
            let height = clamp_bar_height(new_size.height.round().max(1.0) as u32);
            let current_height = self.settings_handle.borrow().data.bar.height;
            if current_height != height {
                self.settings_handle.update(|s| s.data.bar.height = height);
            }
            return Task::done(Message::ExclusiveZoneChange {
                id: self.main_window_id,
                zone_size: height as i32,
            });
        }

        let (width, height) = persisted_window_size(new_size);
        let settings = MainWindowSettings { width, height };
        let size = main_window_size(settings);
        self.main_window_size = size;
        self.last_base_window_size = size;
        let current_settings = self.settings_handle.borrow().data.main_window;
        if current_settings != settings {
            self.settings_handle
                .update(|s| s.data.main_window = settings);
        }
        Task::none()
    }

    pub(super) fn recreate_main_window(&mut self, close_old: bool) -> Task<Message> {
        let old_main_id = self.main_window_id;
        let (bar, decorations) = {
            let settings = &self.settings_handle.borrow().data;
            (settings.bar.clone(), settings.decorations)
        };
        self.config_page.sync_current_bar_output(None);
        self.main_layer_opened = false;
        self.main_layer_ready = false;
        let (new_main_id, open_main, main_is_layer, main_size) = open_main_window(
            self.use_layershell,
            bar,
            self.last_base_window_size,
            decorations,
        );
        self.main_window_id = new_main_id;
        self.main_window_size = main_size;
        self.main_window_is_layer = main_is_layer;
        if close_old {
            Task::batch([open_main, window::close(old_main_id)])
        } else {
            open_main
        }
    }

    pub(super) fn handle_bar_config_change(&mut self, change: BarChange) -> Task<Message> {
        if !self.use_layershell {
            return Task::none();
        }
        let bar = self.settings_handle.borrow().data.bar.clone();
        match change {
            BarChange::Mode if bar.enabled != self.main_window_is_layer => {
                self.recreate_main_window(true)
            }
            BarChange::Monitor if self.main_window_is_layer => self.recreate_main_window(true),
            BarChange::Mode | BarChange::Layout if self.main_window_is_layer => {
                self.apply_bar_layout(bar.alignment, bar.height)
            }
            BarChange::Mode | BarChange::Layout | BarChange::Monitor => Task::none(),
        }
    }

    pub(super) fn recreate_visual_windows(&mut self) -> Task<Message> {
        let decorations = self.settings_handle.borrow().data.decorations;
        let old_popouts = std::mem::take(&mut self.popout_windows);
        let mut tasks = Vec::with_capacity(old_popouts.len() * 2 + 2);

        if !self.main_window_is_layer {
            let old = self.main_window_id;
            let (id, open) =
                open_base_window(self.use_layershell, self.main_window_size, decorations);
            self.main_window_id = id;
            tasks.extend([open, window::close(old)]);
        }
        for (old, popout) in old_popouts {
            let (id, open) = open_base_window(self.use_layershell, popout.size, decorations);
            self.popout_windows.insert(id, popout);
            tasks.extend([open, window::close(old)]);
        }
        Task::batch(tasks)
    }
}
