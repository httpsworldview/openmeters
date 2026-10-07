// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{TOAST_DISPLAY_DURATION, UiApp, windowing::AppWindow};
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
                return Task::batch([restore, app.sync_all_windows()]);
            }
            Some(ConfigEffect::FrameRateChanged(rate)) => {
                app.frames.borrow_mut().set_rate(rate);
            }
            Some(ConfigEffect::DecorationsChanged) => return app.recreate_visual_windows(),
            Some(ConfigEffect::BarChanged(change)) => {
                return app.handle_bar_config_change(change);
            }
            Some(ConfigEffect::ThemeChanged) => {
                if let Some((_, panel)) = app.settings_window.as_mut() {
                    *panel = super::ActiveSettings::new(panel.kind(), &app.visual_manager);
                }
            }
            None => {}
        },
        Message::Visuals(VisualsMessage::SettingsRequested(kind)) => {
            return app.open_settings_window(kind);
        }
        Message::Visuals(visuals_msg) => {
            return app.visuals_page.update(visuals_msg).map(Message::Visuals);
        }
        Message::ToggleConfig => return app.toggle_config_window(),
        Message::TogglePause => app.set_rendering_paused(!app.rendering_paused),
        Message::PopOutOrDock(window_id) => return app.handle_popout_or_dock(window_id),
        Message::BarResizeStart => app.begin_bar_resize(),
        Message::BarResizeMove(pos) => app.handle_bar_resize(pos),
        Message::BarResizeEnd => return app.finish_bar_resize(),
        Message::Quit => {
            if app.exit_warning_until.is_some_and(|d| Instant::now() < d) {
                return exit();
            }
            app.exit_warning_until = Some(Instant::now() + TOAST_DISPLAY_DURATION);
        }
        Message::Tick => app.tick(),
        Message::Watchdog(generation) => {
            app.frames.borrow_mut().watchdog(generation, Instant::now())
        }
        Message::BarOutput(id, name, event) => {
            let change = app.config_page.sync_bar_output(id, name, event);
            if app.main_window.is_bar()
                && change != BarOutputChange::Unchanged
                && (app.main_window.id().is_none()
                    || change == BarOutputChange::Retarget
                    || event == BarOutputEvent::Removed)
            {
                return app.recreate_main_window();
            }
        }
        Message::BarWindowOutput(window, output)
            if app.main_window.is_bar()
                && Some(window) == app.main_window.id()
                && app.config_page.sync_current_bar_output(output) =>
        {
            return app.recreate_main_window();
        }
        Message::ShellWindowClosed(window)
            if app.main_window.is_bar() && Some(window) == app.main_window.id() =>
        {
            return app.on_window_closed(window);
        }
        Message::WindowClosed(window) => return app.on_window_closed(window),
        Message::Settings(window_id, settings_msg) => {
            if let Some((wid, panel)) = app.settings_window.as_mut()
                && *wid == window_id
            {
                panel.handle(settings_msg, &app.visual_manager, &app.settings_handle);
                app.config_page.refresh_theme_choices_if_needed();
            }
        }
        Message::SettingsScrolled(g) => app.settings_scroll = g,
        Message::WindowResized(id, size) => return app.handle_window_resize(id, size),
        _ => {}
    }
    Task::none()
}

pub(super) fn view(app: &UiApp, window_id: window::Id) -> Element<'_, Message> {
    match app.window(window_id) {
        AppWindow::Main => app.main_window_view(window_id),
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

#[cfg(test)]
mod tests {
    use super::{Message::*, *};
    use crate::infra::pipewire::{CaptureControl, test_audio_reader};
    use crate::persistence::settings::{BarAlignment, SettingsHandle};
    use crate::ui::app::UiConfig;
    use crate::ui::app::windowing::{MainWindow, bar_anchor};
    use crate::ui::config::BarChange;
    use BarOutputEvent::{Added, Removed};
    use iced::futures::{StreamExt, executor::block_on};
    use iced::window::Action::Close;
    use iced_exwlshell::reexport::{KeyboardInteractivity, Layer, LayerSize, OutputOption};
    use iced_runtime::Action::{Exit, Output, Window};
    use iced_runtime::{Action, task};
    use std::{cell::RefCell, rc::Rc};

    fn app(settings: &str) -> (tempfile::TempDir, UiApp, Task<Message>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), settings).unwrap();
        let (app, task) = UiApp::new(
            UiConfig {
                capture: CaptureControl::for_test(),
                audio: Rc::new(RefCell::new(Some(test_audio_reader()))),
                settings_handle: SettingsHandle::for_test(dir.path()),
            },
            true,
        );
        (dir, app, task)
    }

    fn actions(task: Task<Message>) -> Vec<Action<Message>> {
        task::into_stream(task).map_or_else(Vec::new, |stream| block_on(stream.collect()))
    }

    macro_rules! assert_actions {
        ($task:expr, $pattern:pat $(if $guard:expr)?) => {{
            let actions = actions($task);
            assert!(matches!(actions.as_slice(), $pattern $(if $guard)?), "{actions:?}");
        }};
    }

    fn base(task: Task<Message>) -> window::Id {
        let actions = actions(task);
        let [Output(NewBaseWindow { id, .. })] = actions.as_slice() else {
            panic!("expected one base window: {actions:?}");
        };
        *id
    }

    fn layer(task: Task<Message>, closed: Option<window::Id>, output: OutputOption) -> window::Id {
        let mut actions = actions(task);
        if let Some(old) = closed {
            let index = actions
                .iter()
                .position(|action| matches!(action, Window(Close(id)) if *id == old));
            actions
                .remove(index.unwrap_or_else(|| panic!("missing close for {old:?}: {actions:?}")));
        }
        let [Output(NewLayerShell { id, settings })] = actions.as_slice() else {
            panic!("expected one layer creation: {actions:?}");
        };
        assert_eq!(
            settings,
            &NewLayerShellSettings {
                size: LayerSize::fill_width(100),
                layer: Layer::Top,
                anchor: bar_anchor(BarAlignment::Bottom),
                exclusive_zone: Some(100),
                keyboard_interactivity: KeyboardInteractivity::OnDemand,
                output_option: output,
                ..Default::default()
            }
        );
        *id
    }

    fn output(
        app: &mut UiApp,
        id: u32,
        name: Option<&str>,
        event: BarOutputEvent,
    ) -> Task<Message> {
        update(app, Message::BarOutput(id, name.map(str::to_owned), event))
    }

    #[test]
    fn bar_waits_for_outputs_and_ignores_retired_window_events() {
        let (_dir, mut app, initial) = app("{}");
        assert_actions!(initial, []);
        for output_id in 1..=3 {
            let id = layer(
                output(&mut app, output_id, None, Added),
                None,
                OutputOption::Active,
            );
            assert_actions!(output(&mut app, output_id, None, Added), []);
            if output_id % 2 == 0 {
                assert_actions!(update(&mut app, BarWindowOutput(id, Some(output_id))), []);
            }
            assert_actions!(output(&mut app, output_id, None, Removed), [Window(Close(old))] if *old == id);
            assert_actions!(update(&mut app, ShellWindowClosed(id)), []);
            assert_actions!(update(&mut app, WindowClosed(id)), []);
            assert_actions!(update(&mut app, BarWindowOutput(id, Some(output_id))), []);
            assert_actions!(update(&mut app, Tick), []);
            assert_actions!(output(&mut app, output_id, None, Removed), []);
        }
    }

    #[test]
    fn bar_closure_before_removal_waits_then_recovers_and_restores_selected_monitor() {
        let (_dir, mut app, initial) = app(r#"{"bar":{"monitor":"HDMI"}}"#);
        assert_actions!(initial, []);
        let selected = || OutputOption::OutputName("HDMI".into());
        let first = layer(output(&mut app, 1, Some("HDMI"), Added), None, selected());
        assert_actions!(output(&mut app, 2, Some("DP"), Added), []);
        assert_actions!(update(&mut app, BarWindowOutput(first, Some(1))), []);
        // A shell close is not an application quit, even before output removal arrives.
        assert_actions!(update(&mut app, ShellWindowClosed(first)), []);
        assert_actions!(update(&mut app, WindowClosed(first)), []);
        assert_actions!(output(&mut app, 2, Some("DP"), Added), []);
        assert_actions!(update(&mut app, Tick), []);
        let fallback = layer(output(&mut app, 1, Some("HDMI"), Removed), None, selected());
        assert_actions!(update(&mut app, BarWindowOutput(fallback, Some(2))), []);
        let restored = layer(
            output(&mut app, 3, Some("HDMI"), Added),
            Some(fallback),
            selected(),
        );
        assert_ne!(restored, fallback);
        assert_actions!(update(&mut app, ShellWindowClosed(fallback)), []);
        assert_actions!(update(&mut app, WindowClosed(fallback)), []);
        assert_eq!(
            app.settings_handle.borrow().data.bar.monitor.as_deref(),
            Some("HDMI")
        );
        // Rejection before the first configure must not start a retry loop either.
        assert_actions!(update(&mut app, WindowClosed(restored)), []);
        assert_actions!(update(&mut app, ShellWindowClosed(restored)), []);
        assert_actions!(update(&mut app, BarWindowOutput(restored, Some(3))), []);
        assert_actions!(update(&mut app, Tick), []);
        assert_actions!(update(&mut app, Quit), []);
        assert_actions!(update(&mut app, Quit), [Exit]);
    }

    #[test]
    fn switching_to_bar_without_outputs_retires_only_the_previous_window() {
        let (_dir, mut app, initial) = app("{}");
        assert_actions!(initial, []);
        // The mode setting has changed, but the previous normal window still exists.
        let old = window::Id::unique();
        app.main_window = MainWindow::Window(old);
        assert_actions!(app.handle_bar_config_change(BarChange::Mode), [Window(Close(id))] if *id == old);
        assert_actions!(update(&mut app, WindowClosed(old)), []);
        assert_actions!(app.handle_bar_config_change(BarChange::Layout), []);
        assert_actions!(app.recreate_visual_windows(), []);
        assert_actions!(update(&mut app, BarResizeStart), []);
        assert!(app.bar_resize_state.is_none());
        let id = base(update(&mut app, ToggleConfig));
        assert_actions!(update(&mut app, WindowClosed(id)), []);
        assert!(app.config_window.is_none());
        layer(output(&mut app, 1, None, Added), None, OutputOption::Active);
    }

    #[test]
    fn normal_window_closing_still_quits_without_outputs() {
        let (_dir, mut app, initial) = app(r#"{"bar":{"enabled":false}}"#);
        let id = base(initial);
        assert_actions!(update(&mut app, WindowClosed(id)), [Exit]);

        // Switching out of bar mode must work even when its surface is absent.
        app.main_window = MainWindow::Bar(None);
        let new_id = base(app.handle_bar_config_change(BarChange::Mode));
        assert_ne!(new_id, id);
        assert_actions!(update(&mut app, WindowClosed(id)), []);
        assert_actions!(update(&mut app, WindowClosed(new_id)), [Exit]);
    }
}
