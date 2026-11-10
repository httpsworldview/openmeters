// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::message::{self, Message};
use super::{ActiveSettings, UiApp};
use crate::persistence::settings::{
    BarAlignment, BarHeight, MainWindowSettings, PopoutWindowSettings, UiSettings, clamp_bar_height,
};
use crate::ui::widgets::scroll_glow::ScrollGlow;
use crate::visuals::registry::{VisualContent, VisualKind};
use iced::{Size, Task, exit, window};
use iced_exwlshell::reexport::{
    Anchor, KeyboardInteractivity, Layer, LayerSize, NewLayerShellSettings, OutputOption, PixelSize,
};
use std::collections::HashMap;
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

fn persisted_window_size(size: Size) -> (u32, u32) {
    let size = size.max(WINDOW_MIN_SIZE);
    (size.width.round() as u32, size.height.round() as u32)
}

fn main_window_size(settings: MainWindowSettings) -> Size {
    Size::new(settings.width as f32, settings.height as f32).max(WINDOW_MIN_SIZE)
}

fn open_base_window(
    layershell: bool,
    size: Size,
    decorations: bool,
) -> (window::Id, Task<Message>) {
    if layershell {
        let (width, height) = persisted_window_size(size);
        let settings = iced_exwlshell::actions::IcedXdgWindowSettings {
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

#[derive(Clone, Copy)]
pub(super) enum MainWindow {
    Window(window::Id),
    Bar(Option<window::Id>),
}

impl MainWindow {
    pub(super) fn id(self) -> Option<window::Id> {
        match self {
            Self::Window(id) => Some(id),
            Self::Bar(id) => id,
        }
    }

    pub(super) fn is_bar(self) -> bool {
        matches!(self, Self::Bar(_))
    }
}

pub(super) fn open_main_window(
    settings: &UiSettings,
    use_layershell: bool,
    has_bar_outputs: bool,
) -> (MainWindow, Task<Message>) {
    let bar = &settings.bar;
    if use_layershell && bar.enabled {
        if !has_bar_outputs {
            return (MainWindow::Bar(None), Task::none());
        }
        let height = bar.height.get();
        let (id, task) = message::layershell_open(NewLayerShellSettings {
            size: LayerSize::fill_width(height),
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
        });
        return (MainWindow::Bar(Some(id)), task);
    }
    let size = main_window_size(settings.main_window);
    let (id, task) = open_base_window(use_layershell, size, settings.decorations);
    (MainWindow::Window(id), task)
}

fn popout_window_size(settings: PopoutWindowSettings) -> Size {
    let dim = |saved: u32, default| if saved > 0 { saved as f32 } else { default };
    Size::new(dim(settings.width, 400.0), dim(settings.height, 300.0)).max(WINDOW_MIN_SIZE)
}

pub(super) struct BarResizeState {
    // Where the drag began, from its first move: the press carries no position.
    pub start_y: Option<f32>,
    pub start_height: BarHeight,
    pub pending_height: BarHeight,
}

pub(super) enum AppWindow<'a> {
    Main,
    Config,
    Settings(&'a ActiveSettings),
    Popout(&'a VisualContent),
    Unknown,
}

impl UiApp {
    pub(super) fn window(&self, id: window::Id) -> AppWindow<'_> {
        if Some(id) == self.main_window.id() {
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
        let previous = self.take_settings_if(|_| true);
        let panel = ActiveSettings::open(kind, &self.settings_handle);
        if let Some((id, previous_kind)) = previous
            && previous_kind == kind
        {
            self.settings_window = Some((id, panel));
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

    fn open_popout_base_window(&self, kind: VisualKind) -> (window::Id, Task<Message>) {
        let settings = &self.settings_handle.borrow().data;
        let saved = settings.visuals.popouts.get(&kind).copied();
        let size = popout_window_size(saved.unwrap_or_default());
        open_base_window(self.use_layershell, size, settings.decorations)
    }

    /// Takes the settings window if `take` accepts it, committing its unfinished edits.
    pub(super) fn take_settings_if(
        &mut self,
        take: impl FnOnce(&(window::Id, ActiveSettings)) -> bool,
    ) -> Option<(window::Id, VisualKind)> {
        let (id, mut panel) = self.settings_window.take_if(|window| take(window))?;
        panel.commit_edits(&self.visual_manager, &self.settings_handle);
        Some((id, panel.kind()))
    }

    pub(super) fn quit(&mut self) -> Task<Message> {
        self.config_page.commit_edits();
        self.take_settings_if(|_| true);
        exit()
    }

    pub(super) fn on_window_closed(&mut self, id: window::Id) -> Task<Message> {
        if Some(id) == self.main_window.id() {
            if !self.main_window.is_bar() {
                return self.quit();
            }
            self.main_window = MainWindow::Bar(None);
            self.bar_resize_state = None;
            self.config_page.sync_current_bar_output(None);
            // A layer close does not carry a reason. Await output changes rather
            // than quitting or repeatedly recreating a rejected surface.
            tracing::debug!("[ui] bar surface closed; waiting for output changes");
            return Task::none();
        }
        if self.config_window.take_if(|window| *window == id).is_some() {
            self.config_page.commit_edits();
        }
        self.take_settings_if(|(window, _)| *window == id);
        if let Some(popout) = self.popout_windows.remove(&id) {
            self.settings_handle
                .edit(|s| s.visuals.popout_mut(popout.kind()).popped_out = false);
        }
        Task::none()
    }

    pub(super) fn sync_visual_windows(
        &mut self,
        old: &UiSettings,
        new: &UiSettings,
    ) -> Task<Message> {
        let visuals = &new.visuals;
        let wanted = |kind| visuals.is_enabled(kind) && visuals.popped_out(kind);
        let restyle = old.decorations != new.decorations;
        let mut tasks: Vec<_> = self
            .popout_windows
            .extract_if(|_, popout| restyle || !wanted(popout.kind()))
            .map(|(id, _)| window::close(id))
            .collect();
        let has_window = |windows: &HashMap<window::Id, VisualContent>, kind| {
            windows.values().any(|popout| popout.kind() == kind)
        };
        let snapshot = self.visual_manager.borrow().snapshot();
        for slot in &snapshot {
            let kind = slot.kind();
            if wanted(kind) && !has_window(&self.popout_windows, kind) {
                let (id, open) = self.open_popout_base_window(kind);
                self.popout_windows.insert(id, slot.content.clone());
                tasks.push(open);
            }
        }
        self.visuals_page
            .apply_snapshot_excluding(&snapshot, |kind| has_window(&self.popout_windows, kind));
        Task::batch(tasks)
    }

    pub(super) fn title(&self, window_id: window::Id) -> String {
        match self.window(window_id) {
            AppWindow::Config => "Configuration - OpenMeters".into(),
            AppWindow::Settings(panel) => format!("{} settings - OpenMeters", panel.kind().label()),
            AppWindow::Popout(popout) => format!("{} - OpenMeters", popout.kind().label()),
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

    pub(super) fn pop_out_or_dock(&mut self, source_window: window::Id) {
        let (kind, popped_out) = match self.popout_windows.get(&source_window) {
            Some(popout) => (popout.kind(), false),
            None => match self.visuals_page.hovered_visual() {
                Some(kind) => (kind, true),
                None => return,
            },
        };
        self.settings_handle
            .edit(|s| s.visuals.popout_mut(kind).popped_out = popped_out);
    }

    fn apply_bar_layout(&self) -> Task<Message> {
        let MainWindow::Bar(Some(id)) = self.main_window else {
            return Task::none();
        };
        let bar = &self.settings_handle.borrow().data.bar;
        let height = bar.height.get();
        Task::batch([
            Task::done(Message::LayoutChange {
                id,
                anchor: bar_anchor(bar.alignment),
                size: LayerSize::fill_width(height),
            }),
            Task::done(Message::ExclusiveZoneChange {
                id,
                zone_size: height as i32,
            }),
        ])
    }

    pub(super) fn handle_window_resize(
        &mut self,
        window_id: window::Id,
        new_size: Size,
    ) -> Task<Message> {
        if let Some(popout) = self.popout_windows.get(&window_id) {
            let kind = popout.kind();
            let (width, height) = persisted_window_size(new_size);
            self.settings_handle.observe(|s| {
                let popout = s.visuals.popout_mut(kind);
                (popout.width, popout.height) = (width, height);
            });
            return Task::none();
        }
        if Some(window_id) != self.main_window.id() {
            return Task::none();
        }

        if self.main_window.is_bar() {
            let height = clamp_bar_height(new_size.height.round() as u32);
            self.settings_handle.observe(|s| s.bar.height = height);
            return Task::done(Message::ExclusiveZoneChange {
                id: window_id,
                zone_size: height.get() as i32,
            });
        }

        let (width, height) = persisted_window_size(new_size);
        let settings = MainWindowSettings { width, height };
        self.settings_handle.observe(|s| s.main_window = settings);
        Task::none()
    }

    pub(super) fn recreate_main_window(&mut self) -> Task<Message> {
        let old_main_id = self.main_window.id();
        self.config_page.sync_current_bar_output(None);
        self.bar_resize_state = None;
        let has_bar_outputs = self.config_page.has_bar_outputs();
        let (main_window, open_main) = open_main_window(
            &self.settings_handle.borrow().data,
            self.use_layershell,
            has_bar_outputs,
        );
        self.main_window = main_window;
        Task::batch(std::iter::once(open_main).chain(old_main_id.map(window::close)))
    }

    /// Rebuilds the main window at most once per change, or updates a bar in place.
    pub(super) fn sync_main_window(&mut self, old: &UiSettings, new: &UiSettings) -> Task<Message> {
        let bar = self.use_layershell && new.bar.enabled;
        let rebuild = if bar {
            !self.main_window.is_bar() || old.bar.monitor != new.bar.monitor
        } else {
            self.main_window.is_bar() || old.decorations != new.decorations
        };
        if rebuild {
            self.recreate_main_window()
        } else if bar && old.bar != new.bar {
            self.apply_bar_layout()
        } else {
            Task::none()
        }
    }
}
