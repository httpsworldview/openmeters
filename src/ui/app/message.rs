// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{TOAST_DISPLAY_DURATION, UiApp, windowing::AppWindow};
use crate::ui::config::ConfigMessage;
use crate::ui::settings::SettingsMessage;
use crate::ui::visuals::VisualsMessage;
use crate::ui::widgets::{fill, page, scroll_glow::ScrollGlow};
use iced::event::{self, Event};
use iced::keyboard::{self, Key};
use iced::widget::text;
use iced::{Element, Size, Task, exit, mouse, window};
use iced_layershell::actions::IcedXdgWindowSettings;
use iced_layershell::reexport::NewLayerShellSettings;
use iced_layershell::shell::ShellEvent;
use iced_layershell::to_layer_message;
use std::time::Instant;

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
pub(super) enum Message {
    Config(ConfigMessage),
    Visuals(VisualsMessage),
    Tick,
    Watchdog(u64),
    AudioWake,
    BarOutput(u32, Option<String>),
    BarWindowOutput(window::Id, Option<String>),
    ToggleConfig,
    TogglePause,
    PopOutOrDock(window::Id),
    BarResizeStart,
    BarResizeMove(iced::Point),
    BarResizeEnd,
    Quit,
    WindowClosed(window::Id),
    WindowResized(window::Id, Size),
    Settings(window::Id, SettingsMessage),
    SettingsScrolled(ScrollGlow),
}

pub(super) fn base_window_open(settings: IcedXdgWindowSettings) -> (window::Id, Task<Message>) {
    Message::base_window_open(settings)
}

pub(super) fn layershell_open(settings: NewLayerShellSettings) -> (window::Id, Task<Message>) {
    Message::layershell_open(settings)
}

pub(super) fn shell_event(event: ShellEvent) -> Option<Message> {
    Some(match event {
        ShellEvent::OutputAdded(output) | ShellEvent::OutputUpdated(output) => {
            Message::BarOutput(output.id, output.name)
        }
        ShellEvent::OutputRemoved(output) => Message::BarOutput(output.id, None),
        ShellEvent::WindowOutputChanged { window, output } => {
            Message::BarWindowOutput(window, output.and_then(|output| output.name))
        }
        _ => return None,
    })
}

pub(super) fn bar_drag_events(evt: Event, _: event::Status, _: window::Id) -> Option<Message> {
    match evt {
        Event::Mouse(mouse::Event::CursorMoved { position }) => {
            Some(Message::BarResizeMove(position))
        }
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
            Some(Message::BarResizeEnd)
        }
        _ => None,
    }
}

pub(super) fn keyboard_shortcut(
    event: Event,
    status: event::Status,
    window_id: window::Id,
) -> Option<Message> {
    let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else {
        return None;
    };
    let (ctrl, shift, no_modifiers) =
        (modifiers.control(), modifiers.shift(), modifiers.is_empty());
    match key {
        Key::Character(ch) if ctrl && shift && ch.eq_ignore_ascii_case("h") => {
            Some(Message::ToggleConfig)
        }
        Key::Named(keyboard::key::Named::Space) if ctrl => Some(Message::PopOutOrDock(window_id)),
        Key::Character(ch) if no_modifiers && status != event::Status::Captured => {
            if ch.eq_ignore_ascii_case("p") {
                Some(Message::TogglePause)
            } else {
                ch.eq_ignore_ascii_case("q").then_some(Message::Quit)
            }
        }
        _ => None,
    }
}

pub(super) fn update(app: &mut UiApp, msg: Message) -> Task<Message> {
    if !app.rendering_paused && !matches!(&msg, Message::Tick | Message::Watchdog(_)) {
        app.frames.borrow_mut().wake();
    }
    match msg {
        Message::Config(config_msg) => {
            let decoration_task = match &config_msg {
                ConfigMessage::DecorationsToggled(enabled) if app.main_window_is_layer => {
                    app.recreate_popout_windows(*enabled)
                }
                ConfigMessage::DecorationsToggled(enabled) => app.recreate_windows(*enabled),
                _ => Task::none(),
            };
            let toggled = match &config_msg {
                ConfigMessage::VisualToggled { kind, enabled } => Some((*kind, *enabled)),
                _ => None,
            };
            let bar_task = app.handle_bar_config_message(&config_msg);
            let theme_changed = matches!(&config_msg, ConfigMessage::ThemeChanged(_));
            if let ConfigMessage::VisualFrameRateChanged(rate) = &config_msg {
                app.frames.borrow_mut().set_rate(*rate);
            }
            app.config_page.update(config_msg);
            let topology_task = toggled.map_or_else(Task::none, |(kind, enabled)| {
                let active = app.visuals_active();
                app.frames.borrow_mut().set_active(active);
                let restore = if enabled {
                    app.restore_popout_window(kind)
                } else {
                    Task::none()
                };
                Task::batch([restore, app.sync_all_windows()])
            });
            if theme_changed && let Some((_, panel)) = app.settings_window.as_mut() {
                *panel = super::ActiveSettings::new(panel.kind(), &app.visual_manager);
            }
            Task::batch([decoration_task, bar_task, topology_task])
        }
        Message::Visuals(VisualsMessage::SettingsRequested(kind)) => {
            app.open_settings_window(kind, false)
        }
        Message::Visuals(visuals_msg) => app.visuals_page.update(visuals_msg).map(Message::Visuals),
        Message::ToggleConfig => app.toggle_config_window(),
        Message::TogglePause => {
            app.set_rendering_paused(!app.rendering_paused);
            Task::none()
        }
        Message::PopOutOrDock(window_id) => app.handle_popout_or_dock(window_id),
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
        Message::Tick => {
            app.tick();
            Task::none()
        }
        Message::Watchdog(generation) => {
            app.frames.borrow_mut().watchdog(generation, Instant::now());
            Task::none()
        }
        Message::BarOutput(id, name) => {
            app.config_page.sync_bar_output(id, name);
            Task::none()
        }
        Message::BarWindowOutput(window, name) => {
            if app.main_window_is_layer && window == app.main_window_id {
                app.config_page.sync_current_bar_output(name);
            }
            Task::none()
        }
        Message::WindowClosed(window_id) => app.on_window_closed(window_id),
        Message::Settings(window_id, settings_msg) => {
            if let Some((wid, panel)) = app.settings_window.as_mut()
                && *wid == window_id
            {
                panel.handle(settings_msg, &app.visual_manager, &app.settings_handle);
                app.config_page.refresh_theme_choices_if_needed();
            }
            Task::none()
        }
        Message::SettingsScrolled(g) => {
            app.settings_scroll = g;
            Task::none()
        }
        Message::WindowResized(id, size) => app.handle_window_resize(id, size),
        _ => Task::none(),
    }
}

pub(super) fn view(app: &UiApp, window_id: window::Id) -> Element<'_, Message> {
    match app.window(window_id) {
        AppWindow::Main => app.main_window_view(),
        AppWindow::Config => page(app.config_page.view().map(Message::Config)).into(),
        AppWindow::Settings(panel) => {
            let content = panel
                .view()
                .map(move |message| Message::Settings(window_id, message));
            page(
                app.settings_scroll
                    .vertical(content, Message::SettingsScrolled),
            )
            .into()
        }
        AppWindow::Popout(popout) => {
            app.with_frame_clock(window_id, popout.view().map(Message::Visuals))
        }
        AppWindow::Unknown => fill(text("")).into(),
    }
}
