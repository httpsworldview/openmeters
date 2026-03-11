// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{TOAST_DISPLAY_DURATION, UiApp};
use crate::ui::pages::config::ConfigMessage;
use crate::ui::pages::visuals::{SettingsMessage, VisualsMessage};
use crate::ui::theme;
use iced::event::{self, Event};
use iced::keyboard::{self, Key};
use iced::widget::{container, scrollable, text};
use iced::{Element, Length, Size, Task, exit, window};
use iced_layershell::actions::IcedXdgWindowSettings;
use iced_layershell::reexport::NewLayerShellSettings;
use iced_layershell::to_layer_message;
use std::time::Instant;

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
pub(super) enum Message {
    Config(ConfigMessage),
    Visuals(VisualsMessage),
    AudioFrame(Vec<f32>),
    ToggleDrawer,
    TogglePause,
    PopOutOrDock(window::Id),
    DrawerResizeStart,
    DrawerResizeMove(iced::Point),
    DrawerResizeEnd,
    BarResizeStart,
    BarResizeMove(iced::Point),
    BarResizeEnd,
    Quit,
    Resize,
    WindowOpened,
    WindowClosed(window::Id),
    WindowResized(window::Id, Size),
    WindowFocused(window::Id),
    Settings(window::Id, SettingsMessage),
}

// Forwarding functions for macro-generated private methods on Message,
// so sibling modules can access them.
pub(super) fn base_window_open(settings: IcedXdgWindowSettings) -> (window::Id, Task<Message>) {
    Message::base_window_open(settings)
}

pub(super) fn layershell_open(settings: NewLayerShellSettings) -> (window::Id, Task<Message>) {
    Message::layershell_open(settings)
}

fn drag_events(
    evt: Event,
    on_move: fn(iced::Point) -> Message,
    on_release: Message,
) -> Option<Message> {
    match evt {
        Event::Mouse(iced::mouse::Event::CursorMoved { position }) => Some(on_move(position)),
        Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
            Some(on_release)
        }
        _ => None,
    }
}

pub(super) fn drawer_drag_events(evt: Event, _: event::Status, _: window::Id) -> Option<Message> {
    drag_events(evt, Message::DrawerResizeMove, Message::DrawerResizeEnd)
}

pub(super) fn bar_drag_events(evt: Event, _: event::Status, _: window::Id) -> Option<Message> {
    drag_events(evt, Message::BarResizeMove, Message::BarResizeEnd)
}

pub(super) fn keyboard_shortcut(
    event: Event,
    _status: event::Status,
    window_id: window::Id,
) -> Option<Message> {
    let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else {
        return None;
    };
    let (ctrl, shift, no_modifiers) =
        (modifiers.control(), modifiers.shift(), modifiers.is_empty());
    match key {
        Key::Character(ch) if ctrl && shift && ch.eq_ignore_ascii_case("h") => {
            Some(Message::ToggleDrawer)
        }
        Key::Named(keyboard::key::Named::Space) if ctrl => Some(Message::PopOutOrDock(window_id)),
        Key::Character(ch) if no_modifiers && ch.eq_ignore_ascii_case("p") => {
            Some(Message::TogglePause)
        }
        Key::Character(ch) if no_modifiers && ch.eq_ignore_ascii_case("q") => Some(Message::Quit),
        _ => None,
    }
}

pub(super) fn update(app: &mut UiApp, msg: Message) -> Task<Message> {
    match msg {
        Message::Config(config_msg) => {
            let decoration_task = if let ConfigMessage::DecorationsToggled(enabled) = &config_msg
                && !app.main_window_is_layer
            {
                app.recreate_windows(*enabled)
            } else {
                Task::none()
            };
            let bar_task = app.handle_bar_config_message(&config_msg);
            let theme_changed = matches!(config_msg, ConfigMessage::ThemeChanged(_));
            let config_task = app.config_page.update(config_msg).map(Message::Config);
            if theme_changed {
                app.refresh_settings_panel();
            }
            Task::batch([
                config_task,
                decoration_task,
                bar_task,
                app.sync_all_windows(),
            ])
        }
        Message::Visuals(VisualsMessage::SettingsRequested { visual_id, kind }) => {
            app.open_settings_window(visual_id, kind)
        }
        Message::Visuals(VisualsMessage::WindowDragRequested) if !app.main_window_is_layer => {
            window::drag(app.main_window_id)
        }
        Message::Visuals(visuals_msg) => app.visuals_page.update(visuals_msg).map(Message::Visuals),
        Message::ToggleDrawer => {
            app.toggle_drawer();
            Task::none()
        }
        Message::TogglePause => {
            app.rendering_paused = !app.rendering_paused;
            Task::none()
        }
        Message::PopOutOrDock(window_id) => app.handle_popout_or_dock(window_id),
        Message::DrawerResizeStart => {
            if app.drawer_open {
                app.drawer_resizing = true;
                app.drawer_resize_offset = None;
            }
            Task::none()
        }
        Message::DrawerResizeMove(pos) => {
            app.handle_drawer_resize(pos);
            Task::none()
        }
        Message::DrawerResizeEnd => {
            app.end_drawer_resize();
            Task::none()
        }
        Message::BarResizeStart => {
            app.begin_bar_resize();
            Task::none()
        }
        Message::BarResizeMove(pos) => {
            app.handle_bar_resize(pos);
            Task::none()
        }
        Message::BarResizeEnd => app.finish_bar_resize(),
        Message::Quit => {
            if app.exit_warning_until.is_some_and(|d| Instant::now() < d) {
                return exit();
            }
            app.exit_warning_until = Some(Instant::now() + TOAST_DISPLAY_DURATION);
            Task::none()
        }
        Message::Resize if !app.main_window_is_layer => {
            window::drag_resize(app.main_window_id, window::Direction::SouthEast)
        }
        Message::AudioFrame(samples) if !app.rendering_paused => {
            app.visual_manager.borrow_mut().ingest_samples(&samples);
            app.sync_all_windows()
        }
        Message::AudioFrame(_) | Message::WindowOpened => Task::none(),
        Message::WindowClosed(window_id) => app.on_window_closed(window_id),
        Message::WindowFocused(id) => {
            app.focused_window = Some(id);
            Task::none()
        }
        Message::Settings(window_id, settings_msg) => {
            if let Some((wid, panel)) = app.settings_window.as_mut()
                && *wid == window_id
            {
                panel.handle(&settings_msg, &app.visual_manager, &app.settings_handle)
            }
            Task::none()
        }
        Message::WindowResized(id, size) => app.handle_main_window_resize(id, size),
        Message::SizeChange { id, size } => {
            app.handle_main_window_resize(id, Size::new(size.0 as f32, size.1 as f32))
        }
        // Layer shell infrastructure messages - handled internally by iced_layershell
        _ => Task::none(),
    }
}

pub(super) fn view(app: &UiApp, window_id: window::Id) -> Element<'_, Message> {
    if window_id == app.main_window_id {
        return app.main_window_view();
    }
    if let Some((_, panel)) = app
        .settings_window
        .as_ref()
        .filter(|(id, _)| *id == window_id)
    {
        let content: Element<'_, SettingsMessage> = fill!(
            scrollable(panel.view())
                .width(Length::Fill)
                .height(Length::Fill)
        )
        .padding(16)
        .style(theme::weak_container)
        .into();
        return content.map(move |msg| Message::Settings(window_id, msg));
    }
    app.popout_windows
        .get(&window_id)
        .map(|popout| popout.view().map(Message::Visuals))
        .unwrap_or_else(|| fill!(text("")).into())
}
