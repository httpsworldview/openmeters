// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{BAR_RETRY_WINDOW, TOAST_DISPLAY_DURATION, UiApp, windowing::AppWindow};
use crate::ui::config::{BarOutputChange, BarOutputEvent, ConfigEffect, ConfigMessage};
use crate::ui::settings::SettingsMessage;
use crate::ui::visuals::VisualsMessage;
use crate::ui::widgets::{fill, page, scroll_glow::ScrollGlow};
use iced::event::{self, Event};
use iced::keyboard::{self, Key};
use iced::widget::text;
use iced::{Element, Size, Task, exit, mouse, window};
use iced_exwlshell::actions::IcedXdgWindowSettings;
use iced_exwlshell::reexport::NewLayerShellSettings;
use iced_exwlshell::shell::ShellEvent;
use iced_exwlshell::to_layer_message;
use std::time::Instant;

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
pub(super) enum Message {
    Config(ConfigMessage),
    Visuals(VisualsMessage),
    Tick,
    Watchdog(u64),
    AudioWake,
    BarOutput(u32, Option<String>, BarOutputEvent),
    BarWindowOutput(window::Id, Option<u32>),
    ShellWindowClosed(window::Id),
    ToggleConfig,
    TogglePause,
    PopOutOrDock(window::Id),
    BarResizeStart,
    BarResizeMove(iced::Point),
    BarResizeEnd,
    Quit,
    WindowOpened(window::Id),
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
    use BarOutputEvent as Change;

    Some(match event {
        ShellEvent::OutputAdded(o) => Message::BarOutput(o.id, o.name, Change::Added),
        ShellEvent::OutputUpdated(o) => Message::BarOutput(o.id, o.name, Change::Updated),
        ShellEvent::OutputRemoved(o) => Message::BarOutput(o.id, o.name, Change::Removed),
        ShellEvent::WindowOutputChanged { window, output } => {
            Message::BarWindowOutput(window, output.map(|output| output.id))
        }
        ShellEvent::Closed(window) => Message::ShellWindowClosed(window),
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

pub(super) fn app_event(
    event: Event,
    status: event::Status,
    window_id: window::Id,
) -> Option<Message> {
    let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else {
        return match event {
            Event::Window(window::Event::Opened { .. }) => Some(Message::WindowOpened(window_id)),
            Event::Window(window::Event::Closed) => Some(Message::WindowClosed(window_id)),
            Event::Window(window::Event::Resized(size)) => {
                Some(Message::WindowResized(window_id, size))
            }
            _ => None,
        };
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

fn retry_bar(app: &mut UiApp, closed: bool) -> Task<Message> {
    if app
        .last_bar_retry
        .is_some_and(|retry| retry.elapsed() < BAR_RETRY_WINDOW)
    {
        return if closed { exit() } else { Task::none() };
    }
    app.last_bar_retry = Some(Instant::now());
    app.recreate_main_window(!closed)
}

fn close_main_layer(app: &mut UiApp, window: window::Id) -> Task<Message> {
    if app.main_layer_ready {
        app.on_window_closed(window)
    } else {
        retry_bar(app, true)
    }
}

pub(super) fn update(app: &mut UiApp, msg: Message) -> Task<Message> {
    if !app.rendering_paused && !matches!(&msg, Message::Tick | Message::Watchdog(_)) {
        app.frames.borrow_mut().wake();
    }
    match msg {
        Message::Config(config_msg) => match app.config_page.update(config_msg) {
            Some(ConfigEffect::VisualToggled { kind, enabled }) => {
                let active = app.visuals_active();
                app.frames.borrow_mut().set_active(active);
                let restore = if enabled {
                    app.restore_popout_window(kind)
                } else {
                    Task::none()
                };
                Task::batch([restore, app.sync_all_windows()])
            }
            Some(ConfigEffect::FrameRateChanged(rate)) => {
                app.frames.borrow_mut().set_rate(rate);
                Task::none()
            }
            Some(ConfigEffect::DecorationsChanged) => app.recreate_visual_windows(),
            Some(ConfigEffect::BarChanged(change)) => {
                app.last_bar_retry = None;
                app.handle_bar_config_change(change)
            }
            Some(ConfigEffect::ThemeChanged) => {
                if let Some((_, panel)) = app.settings_window.as_mut() {
                    *panel = super::ActiveSettings::new(panel.kind(), &app.visual_manager);
                }
                Task::none()
            }
            None => Task::none(),
        },
        Message::Visuals(VisualsMessage::SettingsRequested(kind)) => app.open_settings_window(kind),
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
        Message::BarOutput(id, name, event) => {
            let change = app.config_page.sync_bar_output(id, name, event);
            if app.main_window_is_layer {
                if change != BarOutputChange::Unchanged {
                    app.last_bar_retry = None;
                }
                if change == BarOutputChange::CurrentRemoved {
                    app.main_layer_ready = false;
                }
                if change == BarOutputChange::Retarget {
                    return retry_bar(app, false);
                }
            }
            Task::none()
        }
        Message::BarWindowOutput(window, output)
            if app.main_window_is_layer && window == app.main_window_id =>
        {
            app.main_layer_ready = true;
            if app.config_page.sync_current_bar_output(output) {
                retry_bar(app, false)
            } else {
                Task::none()
            }
        }
        Message::BarWindowOutput(_, _) => Task::none(),
        // Output changes and shell closes share one ordered event stream.
        Message::ShellWindowClosed(window)
            if app.main_window_is_layer && window == app.main_window_id =>
        {
            close_main_layer(app, window)
        }
        Message::ShellWindowClosed(_) => Task::none(),
        Message::WindowOpened(window) => {
            if app.main_window_is_layer && window == app.main_window_id {
                app.main_layer_opened = true;
            }
            Task::none()
        }
        // After Opened, the ordered shell close is authoritative for the main layer.
        Message::WindowClosed(window)
            if app.main_window_is_layer && window == app.main_window_id =>
        {
            if app.main_layer_opened {
                Task::none()
            } else {
                close_main_layer(app, window)
            }
        }
        Message::WindowClosed(window) => app.on_window_closed(window),
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
