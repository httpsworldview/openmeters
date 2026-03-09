// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::message::{self, Message};
use crate::persistence::settings::{BarAlignment, BarSettings, clamp_bar_height};
use crate::ui::pages::visuals::VisualsMessage;
use crate::visuals::registry::{
    VisualContent, VisualId, VisualKind, VisualMetadata, VisualSnapshot,
};
use iced::widget::{container, mouse_area, text};
use iced::{Element, Length, Size, Task, window};
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
