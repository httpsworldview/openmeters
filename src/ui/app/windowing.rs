// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::message::{self, Message};
use super::{ActiveSettings, UiApp};
use crate::persistence::settings::{
    BarAlignment, BarSettings, MainWindowSettings, PopoutWindowSettings, clamp_bar_height,
};
use crate::ui::config::ConfigMessage;
use crate::ui::theme;
use crate::ui::visuals::VisualsMessage;
use crate::ui::widgets::{fill, scroll_glow::ScrollGlow};
use crate::util::color::with_alpha;
use crate::visuals::registry::{VisualContent, VisualKind, VisualSlotSnapshot};
use iced::widget::{mouse_area, text};
use iced::{Element, Size, Task, exit, window};
use iced_layershell::actions::OutputSnapshotCallback;
use iced_layershell::reexport::{
    Anchor, KeyboardInteractivity, Layer, NewLayerShellSettings, OutputOption,
};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle};

pub(super) const APP_ID: &str = "openmeters-ui";
const WINDOW_MIN_SIZE: Size = Size::new(200.0, 150.0);
const TOOL_WINDOW_SIZE: Size = Size::new(480.0, 600.0);

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

pub(super) fn bar_anchor(alignment: BarAlignment) -> Anchor {
    match alignment {
        BarAlignment::Top => Anchor::Top | Anchor::Left | Anchor::Right,
        BarAlignment::Bottom => Anchor::Bottom | Anchor::Left | Anchor::Right,
    }
}

fn bar_layershell_settings(bar: &BarSettings, height: u32) -> NewLayerShellSettings {
    NewLayerShellSettings {
        size: Some((0, height)),
        layer: Layer::Top,
        anchor: bar_anchor(bar.alignment),
        exclusive_zone: Some(height as i32),
        keyboard_interactivity: KeyboardInteractivity::OnDemand,
        output_option: bar
            .monitor
            .clone()
            .map(OutputOption::OutputName)
            .unwrap_or_default(),
        ..Default::default()
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

fn main_window_settings(size: Size) -> MainWindowSettings {
    let (width, height) = persisted_window_size(size);
    MainWindowSettings { width, height }
}

fn base_window_settings(size: Size, decorations: bool) -> window::Settings {
    window::Settings {
        size,
        min_size: Some(WINDOW_MIN_SIZE),
        resizable: true,
        decorations,
        // Keep one alpha mode across base windows; visual windows need it for background opacity.
        transparent: true,
        ..Default::default()
    }
}

fn open_base_window(
    layershell: bool,
    size: Size,
    decorations: bool,
) -> (window::Id, Task<Message>) {
    if layershell {
        let settings = iced_layershell::actions::IcedXdgWindowSettings {
            size: Some((size.width.round() as u32, size.height.round() as u32)),
            client_side_decorations: !decorations,
        };
        message::base_window_open(settings)
    } else {
        let (id, task) = window::open(base_window_settings(size, decorations));
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
        let settings = bar_layershell_settings(&bar_settings, height);
        let (id, task) = message::layershell_open(settings);
        let new_size = Size::new(base_size.width, height as f32);
        return (id, task, true, new_size);
    }

    let (id, task) = open_base_window(use_layershell, base_size, with_decorations);
    (id, task, false, base_size)
}

fn popout_window_size(saved: Option<PopoutWindowSettings>) -> Size {
    let saved = saved.unwrap_or_default();
    let dim = |saved: u32, default| if saved > 0 { saved as f32 } else { default };
    clamp_window_size(Size::new(dim(saved.width, 400.0), dim(saved.height, 300.0)))
}

fn popout_window_settings(size: Size, popped_out: bool) -> PopoutWindowSettings {
    let (width, height) = persisted_window_size(size);
    PopoutWindowSettings {
        width,
        height,
        popped_out,
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct BarResizeState {
    pub start_y: f32,
    pub start_height: u32,
    pub pending_height: u32,
}

pub(super) struct PopoutWindow {
    pub kind: VisualKind,
    pub original_index: usize,
    pub size: Size,
    pub cached: Option<VisualContent>,
}

impl PopoutWindow {
    pub fn sync_from_snapshot(&mut self, snapshot: &[VisualSlotSnapshot]) {
        self.cached = snapshot
            .iter()
            .find(|slot| slot.kind == self.kind && slot.enabled)
            .map(|slot| slot.content.clone());
    }

    pub fn view(&self) -> Element<'_, VisualsMessage> {
        let Some(content) = &self.cached else {
            return fill(text("")).into();
        };
        let msg = VisualsMessage::SettingsRequested(self.kind);
        mouse_area(fill(content.render()))
            .on_right_press(msg)
            .into()
    }
}

impl UiApp {
    pub(super) fn refresh_settings_panel(&mut self) {
        let Some((_, panel)) = self.settings_window.as_mut() else {
            return;
        };
        *panel = ActiveSettings::new(panel.kind, &self.visual_manager);
    }

    pub(super) fn open_settings_window(&mut self, kind: VisualKind) -> Task<Message> {
        let new_panel = ActiveSettings::new(kind, &self.visual_manager);
        let previous = self.settings_window.take();
        if previous
            .as_ref()
            .is_some_and(|(_, panel)| panel.kind == kind)
        {
            self.settings_window = previous.map(|(id, _)| (id, new_panel));
            return Task::none();
        }
        let (new_id, open_task) = open_tool_base_window(self.use_layershell);
        self.settings_scroll = ScrollGlow::default();
        self.settings_window = Some((new_id, new_panel));
        match previous {
            Some((old_id, _)) => Task::batch([window::close(old_id), open_task]),
            None => open_task,
        }
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
        let (index, _) = snapshot
            .iter()
            .enumerate()
            .find(|(_, s)| s.kind == kind && s.enabled)?;
        let window_size = popout_window_size(saved_size);
        let use_decorations = self.settings_handle.borrow().data.decorations;
        let (new_id, open_task) =
            open_base_window(self.use_layershell, window_size, use_decorations);
        let mut popout = PopoutWindow {
            kind,
            original_index: index,
            size: window_size,
            cached: None,
        };
        popout.sync_from_snapshot(&snapshot);
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
        let saved = {
            self.settings_handle
                .borrow()
                .data
                .visuals
                .popouts
                .get(&kind)
                .copied()
                .filter(|s| s.popped_out)
        };
        let Some(settings) = saved else {
            return Task::none();
        };
        self.create_popout_window(kind, Some(settings))
            .map_or_else(Task::none, |(_, task)| task)
    }

    fn open_popout_window(&mut self, kind: VisualKind) -> Task<Message> {
        let saved_size = self
            .settings_handle
            .borrow()
            .data
            .visuals
            .popouts
            .get(&kind)
            .copied();
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
        if self.config_window == Some(id) {
            self.config_window = None;
        }
        if self.settings_window.as_ref().is_some_and(|(w, _)| *w == id) {
            self.settings_window = None;
        }
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
                    .any(|slot| slot.kind == panel.kind && slot.enabled)
            })
            .map(|(id, _)| window::close::<Message>(id));
        self.popout_windows
            .values_mut()
            .for_each(|popout| popout.sync_from_snapshot(&snapshot));
        let stale_windows: Vec<_> = self
            .popout_windows
            .extract_if(|_, popout| popout.cached.is_none())
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
        self.visuals_page
            .apply_snapshot_excluding(&snapshot, |kind| {
                self.popout_windows.values().any(|w| w.kind == kind)
            });
        Task::batch(
            close_settings_task.into_iter().chain(
                stale_windows
                    .into_iter()
                    .map(|(id, _, _)| window::close(id)),
            ),
        )
    }

    pub(super) fn title(&self, window_id: window::Id) -> String {
        if window_id == self.main_window_id {
            return "OpenMeters".into();
        }

        if self.config_window == Some(window_id) {
            return "Configuration - OpenMeters".into();
        }

        let (kind, suffix) = if let Some((_, panel)) = self
            .settings_window
            .as_ref()
            .filter(|(id, _)| *id == window_id)
        {
            (panel.kind, " settings")
        } else if let Some(popout) = self.popout_windows.get(&window_id) {
            (popout.kind, "")
        } else {
            return "OpenMeters".into();
        };

        format!("{}{} - OpenMeters", kind.label(), suffix)
    }

    pub(super) fn theme(&self, window_id: window::Id) -> iced::Theme {
        let is_config = self.config_window == Some(window_id);
        let is_settings = matches!(&self.settings_window, Some((w, _)) if *w == window_id);
        let is_tool = is_config || is_settings;
        // Tool windows force opaque alpha: they have no wgpu visual backdrop, so a
        // translucent user background would let the desktop bleed through the chrome.
        let custom_bg = if is_tool
            || window_id == self.main_window_id
            || self.popout_windows.contains_key(&window_id)
        {
            self.settings_handle.borrow().data.background_color
        } else {
            None
        }
        .map(|c| {
            let c: iced::Color = c.into();
            if is_tool { with_alpha(c, 1.0) } else { c }
        });
        theme::theme(custom_bg)
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

    pub(super) fn sync_visuals_page(&mut self) {
        let snapshot = self.visual_manager.borrow().snapshot();
        self.visuals_page
            .apply_snapshot_excluding(&snapshot, |kind| {
                self.popout_windows.values().any(|w| w.kind == kind)
            });
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
            return Task::batch([
                Task::done(Message::ExclusiveZoneChange {
                    id: self.main_window_id,
                    zone_size: height as i32,
                }),
                self.request_main_output_snapshot(),
            ]);
        }

        let settings = main_window_settings(new_size);
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
        Task::batch([open_main, window::close(old_main_id)])
    }

    pub(super) fn request_main_output_snapshot(&self) -> Task<Message> {
        if !self.main_window_is_layer {
            return Task::none();
        }
        let id = self.main_window_id;
        let (sender, receiver) = async_channel::bounded(1);
        Task::batch([
            Task::done(Message::OutputSnapshotRequest {
                id,
                callback: OutputSnapshotCallback::new(move |snapshot| {
                    let _ = sender.try_send(snapshot);
                }),
            }),
            Task::perform(async move { receiver.recv().await.ok() }, move |snapshot| {
                Message::BarOutputResolved(id, snapshot)
            }),
        ])
    }

    pub(super) fn handle_bar_config_message(
        &mut self,
        config_msg: &ConfigMessage,
    ) -> Task<Message> {
        if !self.use_layershell
            || !matches!(
                config_msg,
                ConfigMessage::BarModeToggled(_)
                    | ConfigMessage::BarAlignmentChanged(_)
                    | ConfigMessage::BarHeightChanged(_)
                    | ConfigMessage::BarMonitorChanged(_)
            )
        {
            return Task::none();
        }
        let (bar, decorations) = {
            let guard = self.settings_handle.borrow();
            let settings = &guard.data;
            (settings.bar.clone(), settings.decorations)
        };
        match config_msg {
            ConfigMessage::BarModeToggled(true) if self.main_window_is_layer => {
                self.apply_bar_layout(bar.alignment, bar.height)
            }
            ConfigMessage::BarModeToggled(enabled) if *enabled == self.main_window_is_layer => {
                Task::none()
            }
            ConfigMessage::BarModeToggled(enabled) => self.recreate_main_window(
                BarSettings {
                    enabled: *enabled,
                    ..bar
                },
                decorations,
            ),
            ConfigMessage::BarAlignmentChanged(alignment) if self.main_window_is_layer => {
                self.apply_bar_layout(*alignment, bar.height)
            }
            ConfigMessage::BarHeightChanged(height) if self.main_window_is_layer => {
                self.apply_bar_layout(bar.alignment, *height)
            }
            ConfigMessage::BarMonitorChanged(monitor) if self.main_window_is_layer => {
                if bar.monitor.as_deref() == Some(monitor.as_str()) {
                    Task::none()
                } else {
                    self.recreate_main_window(
                        BarSettings {
                            monitor: Some(monitor.clone()),
                            ..bar
                        },
                        decorations,
                    )
                }
            }
            _ => Task::none(),
        }
    }

    pub(super) fn recreate_settings_window(&mut self) -> Task<Message> {
        let Some((old_id, panel)) = self.settings_window.take() else {
            return Task::none();
        };
        let (new_id, open_task) = open_tool_base_window(self.use_layershell);
        self.settings_window = Some((
            new_id,
            ActiveSettings::new(panel.kind, &self.visual_manager),
        ));
        Task::batch([open_task, window::close(old_id)])
    }

    pub(super) fn recreate_popout_windows(&mut self, use_decorations: bool) -> Task<Message> {
        let old_popouts = std::mem::take(&mut self.popout_windows);
        let mut tasks = Vec::with_capacity(old_popouts.len() * 2);
        for (old_id, popout) in old_popouts {
            let (new_id, open_task) =
                open_base_window(self.use_layershell, popout.size, use_decorations);
            self.popout_windows.insert(new_id, popout);
            tasks.push(open_task);
            tasks.push(window::close(old_id));
        }
        Task::batch(tasks)
    }

    pub(super) fn recreate_windows(&mut self, use_decorations: bool) -> Task<Message> {
        let old_main_id = self.main_window_id;
        let (new_main_id, open_main) =
            open_base_window(self.use_layershell, self.main_window_size, use_decorations);
        self.main_window_id = new_main_id;
        self.main_window_is_layer = false;
        let settings_task = self.recreate_settings_window();
        Task::batch([
            open_main,
            window::close(old_main_id),
            settings_task,
            self.recreate_popout_windows(use_decorations),
        ])
    }
}
