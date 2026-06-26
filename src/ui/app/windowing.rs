// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::UiApp;
use super::message::{self, Message};
use crate::persistence::settings::{
    BarAlignment, BarSettings, MainWindowSettings, clamp_bar_height,
};
use crate::ui::config::ConfigMessage;
use crate::ui::settings::create_panel as create_settings_panel;
use crate::ui::theme;
use crate::ui::visuals::VisualsMessage;
use crate::ui::widgets::scroll_glow::ScrollGlow;
use crate::util::color::with_alpha;
use crate::visuals::registry::{VisualContent, VisualKind, VisualMetadata, VisualSlotSnapshot};
use iced::widget::{container, mouse_area, text};
use iced::{Element, Length, Size, Task, exit, window};
use iced_layershell::actions::OutputSnapshotCallback;
use iced_layershell::reexport::{
    Anchor, KeyboardInteractivity, Layer, NewLayerShellSettings, OutputOption,
};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle};

pub(super) const APP_ID: &str = "openmeters-ui";
const WINDOW_MIN_SIZE: Size = Size::new(200.0, 150.0);
pub(super) const SETTINGS_WINDOW_SIZE: Size = Size::new(480.0, 600.0);

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

pub(super) fn main_window_size(settings: MainWindowSettings) -> Size {
    Size::new(
        (settings.width as f32).max(WINDOW_MIN_SIZE.width),
        (settings.height as f32).max(WINDOW_MIN_SIZE.height),
    )
}

fn main_window_settings(size: Size) -> MainWindowSettings {
    MainWindowSettings {
        width: size.width.round().max(WINDOW_MIN_SIZE.width) as u32,
        height: size.height.round().max(WINDOW_MIN_SIZE.height) as u32,
    }
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

fn open_settings_base_window(use_layershell: bool) -> (window::Id, Task<Message>) {
    open_base_window(use_layershell, SETTINGS_WINDOW_SIZE, true)
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

fn popout_window_size(metadata: &VisualMetadata) -> Size {
    Size::new(
        metadata.preferred_width.max(400.0),
        metadata.preferred_height.max(300.0),
    )
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
    pub cached: Option<(VisualMetadata, VisualContent)>,
}

impl PopoutWindow {
    pub fn sync_from_snapshot(&mut self, snapshot: &[VisualSlotSnapshot]) {
        self.cached = snapshot
            .iter()
            .find(|slot| slot.kind == self.kind && slot.enabled)
            .map(|slot| (slot.metadata, slot.content.clone()));
    }

    pub fn view(&self) -> Element<'_, VisualsMessage> {
        let Some((_, content)) = &self.cached else {
            return fill!(text("")).into();
        };
        let msg = VisualsMessage::SettingsRequested(self.kind);
        mouse_area(fill!(content.render()))
            .on_right_press(msg)
            .into()
    }
}

impl UiApp {
    pub(super) fn refresh_settings_panel(&mut self) {
        let Some((_, panel)) = self.settings_window.as_mut() else {
            return;
        };
        *panel = create_settings_panel(panel.kind, &self.visual_manager);
    }

    pub(super) fn open_settings_window(&mut self, kind: VisualKind) -> Task<Message> {
        let new_panel = create_settings_panel(kind, &self.visual_manager);
        let previous = self.settings_window.take();
        if previous
            .as_ref()
            .is_some_and(|(_, panel)| panel.kind == kind)
        {
            self.settings_window = previous.map(|(id, _)| (id, new_panel));
            return Task::none();
        }
        let (new_id, open_task) = open_settings_base_window(self.use_layershell);
        self.settings_scroll = ScrollGlow::default();
        self.settings_window = Some((new_id, new_panel));
        match previous {
            Some((old_id, _)) => Task::batch([window::close(old_id), open_task]),
            None => open_task,
        }
    }

    pub(super) fn open_popout_window(&mut self, kind: VisualKind) -> Task<Message> {
        if self
            .popout_windows
            .values()
            .any(|popout| popout.kind == kind)
        {
            return Task::none();
        }
        let snapshot = self.visual_manager.borrow().snapshot();
        let Some((index, slot)) = snapshot.iter().enumerate().find(|(_, s)| s.kind == kind) else {
            return Task::none();
        };
        let window_size = popout_window_size(&slot.metadata);
        let use_decorations = self.settings_handle.borrow().data.decorations;
        let (new_id, open_task) =
            open_base_window(self.use_layershell, window_size, use_decorations);
        let mut popout = PopoutWindow {
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

    pub(super) fn popped_out_kinds(&self) -> Vec<VisualKind> {
        self.popout_windows.values().map(|w| w.kind).collect()
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
            .map(|(id, _)| id)
            .collect();
        self.visuals_page
            .apply_snapshot_excluding(&snapshot, &self.popped_out_kinds());
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
        let is_settings = matches!(&self.settings_window, Some((w, _)) if *w == window_id);
        // Settings window forces opaque alpha: it has no wgpu visual backdrop, so a
        // translucent user background would let the desktop bleed through the chrome.
        let custom_bg = if is_settings
            || window_id == self.main_window_id
            || self.popout_windows.contains_key(&window_id)
        {
            self.settings_handle.borrow().data.background_color
        } else {
            None
        }
        .map(|c| {
            let c: iced::Color = c.into();
            if is_settings { with_alpha(c, 1.0) } else { c }
        });
        theme::theme(custom_bg)
    }

    pub(super) fn handle_popout_or_dock(&mut self, source_window: window::Id) -> Task<Message> {
        if let Some(popout) = self.popout_windows.remove(&source_window) {
            self.visual_manager
                .borrow_mut()
                .move_to(popout.kind, popout.original_index);
            self.sync_visuals_page();
            self.settings_handle.update(|settings| {
                settings.data.visuals.order = self.visual_manager.borrow().order();
            });
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
            .apply_snapshot_excluding(&snapshot, &self.popped_out_kinds());
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
        let (new_id, open_task) = open_settings_base_window(self.use_layershell);
        self.settings_window = Some((
            new_id,
            create_settings_panel(panel.kind, &self.visual_manager),
        ));
        Task::batch([open_task, window::close(old_id)])
    }

    pub(super) fn recreate_popout_windows(&mut self, use_decorations: bool) -> Task<Message> {
        let old_popouts = std::mem::take(&mut self.popout_windows);
        let mut tasks = Vec::with_capacity(old_popouts.len() * 2);
        for (old_id, popout) in old_popouts {
            let window_size = popout
                .cached
                .as_ref()
                .map_or(Size::new(400.0, 300.0), |(meta, _)| {
                    popout_window_size(meta)
                });
            let (new_id, open_task) =
                open_base_window(self.use_layershell, window_size, use_decorations);
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
