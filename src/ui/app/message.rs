// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{TOAST_DISPLAY_DURATION, UiApp, windowing::AppWindow};
use crate::ui::config::{BarOutputChange, BarOutputEvent, ConfigMessage};
use crate::ui::settings::SettingsMessage;
use crate::ui::visuals::{VisualsMessage, visual_area};
use crate::ui::widgets::{fill, page, scroll_glow::ScrollEvent};
use iced::event::{self, Event};
use iced::keyboard::{self, Key};
use iced::widget::text;
use iced::{Element, Size, Task, mouse, touch, window};
use iced_exwlshell::actions::IcedXdgWindowSettings;
use iced_exwlshell::reexport::NewLayerShellSettings;
use iced_exwlshell::shell::ShellEvent;
use iced_exwlshell::to_layer_message;
use std::time::{Duration, Instant};

// A held key repeats well within this, so a later press is fresh even if focus took the
// release before it.
const KEY_REPEAT_GAP: Duration = Duration::from_secs(1);

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
pub(super) enum Message {
    Config(ConfigMessage),
    Visuals(VisualsMessage),
    Tick,
    SettingsFilesChanged,
    Watchdog(u64),
    AudioWake,
    BarOutput(u32, Option<String>, BarOutputEvent),
    BarWindowOutput(window::Id, Option<u32>),
    ShellWindowClosed(window::Id),
    // Key repeats can arrive as fresh presses (layer shell; X11 after a focus change), so a
    // shortcut acts on its key's first press and ignores more until that key is released.
    Shortcut(keyboard::key::Physical, Instant, Box<Message>),
    ShortcutReleased(keyboard::key::Physical),
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
    SettingsScrolled(ScrollEvent),
}

pub(super) fn base_window_open(settings: IcedXdgWindowSettings) -> (window::Id, Task<Message>) {
    Message::base_window_open(settings)
}

pub(super) fn layershell_open(settings: NewLayerShellSettings) -> (window::Id, Task<Message>) {
    Message::layershell_open(settings)
}

pub(super) fn shell_event(event: ShellEvent) -> Option<Message> {
    use BarOutputEvent as Change;

    match event {
        ShellEvent::OutputAdded(o) => Some(Message::BarOutput(o.id, o.name, Change::Added)),
        ShellEvent::OutputUpdated(o) => Some(Message::BarOutput(o.id, o.name, Change::Updated)),
        ShellEvent::OutputRemoved(o) => Some(Message::BarOutput(o.id, o.name, Change::Removed)),
        ShellEvent::WindowOutputChanged { window, output } => Some(Message::BarWindowOutput(
            window,
            output.map(|output| output.id),
        )),
        ShellEvent::Closed(window) => Some(Message::ShellWindowClosed(window)),
        _ => None,
    }
}

pub(super) fn bar_drag_events(evt: Event, _: event::Status, _: window::Id) -> Option<Message> {
    match evt {
        Event::Mouse(mouse::Event::CursorMoved { position })
        | Event::Touch(touch::Event::FingerMoved { position, .. }) => {
            Some(Message::BarResizeMove(position))
        }
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
        | Event::Touch(touch::Event::FingerLifted { .. } | touch::Event::FingerLost { .. }) => {
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
    let Event::Keyboard(keyboard::Event::KeyPressed {
        key,
        modifiers,
        physical_key,
        ..
    }) = event
    else {
        return match event {
            Event::Keyboard(keyboard::Event::KeyReleased { physical_key, .. }) => {
                Some(Message::ShortcutReleased(physical_key))
            }
            Event::Window(window::Event::Closed) => Some(Message::WindowClosed(window_id)),
            Event::Window(window::Event::Resized(size)) => {
                Some(Message::WindowResized(window_id, size))
            }
            _ => None,
        };
    };
    let (ctrl, shift, no_modifiers) =
        (modifiers.control(), modifiers.shift(), modifiers.is_empty());
    let action = match key {
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
    };
    action.map(|action| Message::Shortcut(physical_key, Instant::now(), Box::new(action)))
}

pub(super) fn update(app: &mut UiApp, msg: Message) -> Task<Message> {
    // A shortcut wakes through the action it runs.
    let quiet = matches!(
        &msg,
        Message::Tick | Message::Watchdog(_) | Message::Shortcut(..) | Message::ShortcutReleased(_)
    );
    if !quiet {
        app.frames.borrow_mut().wake();
    }
    let task = handle(app, msg);
    match app.apply_settings_changes() {
        Some(applied) => Task::batch([task, applied]),
        None => task,
    }
}

fn handle(app: &mut UiApp, msg: Message) -> Task<Message> {
    match msg {
        Message::Config(config_msg) => app.config_page.update(config_msg),
        Message::Visuals(VisualsMessage::SettingsRequested(kind)) => {
            return app.open_settings_window(kind);
        }
        Message::Visuals(visuals_msg) => app.visuals_page.update(visuals_msg),
        Message::Shortcut(key, at, action) => {
            let previous = app.held_shortcut.replace((key, at));
            let repeat = previous.is_some_and(|(held, last)| {
                held == key && at.saturating_duration_since(last) < KEY_REPEAT_GAP
            });
            if !repeat {
                return update(app, *action);
            }
        }
        Message::ShortcutReleased(key) => {
            app.held_shortcut = app.held_shortcut.filter(|&(held, _)| held != key);
        }
        Message::ToggleConfig => return app.toggle_config_window(),
        Message::TogglePause => app.set_rendering_paused(!app.rendering_paused),
        Message::PopOutOrDock(window_id) => app.pop_out_or_dock(window_id),
        Message::BarResizeStart => app.begin_bar_resize(),
        Message::BarResizeMove(pos) => app.handle_bar_resize(pos),
        Message::BarResizeEnd => app.finish_bar_resize(),
        Message::Quit => {
            if app.exit_warning_until.is_some_and(|d| Instant::now() < d) {
                return app.quit();
            }
            app.exit_warning_until = Some(Instant::now() + TOAST_DISPLAY_DURATION);
        }
        Message::Tick => app.tick(),
        Message::SettingsFilesChanged => {
            app.settings_handle.reload();
            app.config_page.refresh_theme_choices();
        }
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
            }
        }
        Message::SettingsScrolled(event) => app.settings_scroll.update(event),
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
            let visual = Element::from(fill(visual_area(popout))).map(Message::Visuals);
            app.with_frame_clock(window_id, visual)
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
    use crate::visuals::registry::VisualKind;
    use BarOutputEvent::{Added, Removed};
    use iced::futures::{StreamExt, executor::block_on};
    use iced::window::Action::Close;
    use iced_exwlshell::reexport::{
        KeyboardInteractivity, Layer, LayerSize, OutputOption, PixelSize,
    };
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
                settings_handle: SettingsHandle::load_from(dir.path()),
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
        update(app, BarOutput(id, name.map(str::to_owned), event))
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
        let key_event = |code, pressed| {
            let event = Event::Keyboard(if pressed {
                keyboard::Event::KeyPressed {
                    key: Key::Character("q".into()),
                    modified_key: Key::Character("q".into()),
                    physical_key: keyboard::key::Physical::Code(code),
                    location: keyboard::Location::Standard,
                    modifiers: keyboard::Modifiers::empty(),
                    text: None,
                    repeat: false,
                }
            } else {
                keyboard::Event::KeyReleased {
                    key: Key::Character("q".into()),
                    modified_key: Key::Character("q".into()),
                    physical_key: keyboard::key::Physical::Code(code),
                    location: keyboard::Location::Standard,
                    modifiers: keyboard::Modifiers::empty(),
                }
            });
            app_event(event, event::Status::Ignored, window::Id::unique()).unwrap()
        };
        let q = keyboard::key::Code::KeyQ;
        assert_actions!(update(&mut app, key_event(q, true)), []);
        // Repeats of the held key, and other keys' releases, are not the confirming keystroke.
        assert_actions!(update(&mut app, key_event(q, true)), []);
        let ctrl = key_event(keyboard::key::Code::ControlLeft, false);
        assert_actions!(update(&mut app, ctrl), []);
        assert_actions!(update(&mut app, key_event(q, false)), []);
        assert_actions!(update(&mut app, key_event(q, true)), [Exit]);
        // Focus can take a release elsewhere; a press after the repeat gap is still fresh.
        let later = Instant::now() + KEY_REPEAT_GAP;
        let quit = |at| Shortcut(keyboard::key::Physical::Code(q), at, Box::new(Quit));
        assert_actions!(update(&mut app, quit(later)), [Exit]);
        assert_actions!(update(&mut app, quit(later + KEY_REPEAT_GAP / 2)), []);
    }

    #[test]
    fn switching_to_bar_without_outputs_retires_only_the_previous_window() {
        let (_dir, mut app, initial) = app("{}");
        assert_actions!(initial, []);
        // The mode setting has changed, but the previous normal window still exists.
        let old = window::Id::unique();
        app.main_window = MainWindow::Window(old);
        let settings = app.settings_handle.borrow().data.clone();
        let rebuilt = app.sync_main_window(&settings, &settings);
        assert_actions!(rebuilt, [Window(Close(id))] if *id == old);
        assert_actions!(update(&mut app, WindowClosed(old)), []);
        assert_actions!(app.sync_main_window(&settings, &settings), []);
        assert_actions!(app.sync_visual_windows(&settings, &settings), []);
        assert_actions!(update(&mut app, BarResizeStart), []);
        assert!(app.bar_resize_state.is_none());
        let id = base(update(&mut app, ToggleConfig));
        assert_actions!(update(&mut app, WindowClosed(id)), []);
        assert!(app.config_window.is_none());
        let bar = layer(output(&mut app, 1, None, Added), None, OutputOption::Active);
        // A compositor resize is recorded, and not sent back by later changes.
        let resized = update(&mut app, WindowResized(bar, Size::new(1920.0, 120.0)));
        assert_actions!(
            resized,
            [Output(ExclusiveZoneChange { zone_size: 120, .. })]
        );
        assert_eq!(app.settings_handle.borrow().data.bar.height.get(), 120);
        let decorations = Config(ConfigMessage::DecorationsToggled(true));
        assert_actions!(update(&mut app, decorations), []);
    }

    #[test]
    fn background_drags_save_on_release_or_window_close_and_follow_external_edits() {
        use crate::ui::widgets::palette_editor::{HsvEdit::SatVal, PaletteEvent::*};
        let (dir, mut app, initial) = app(r#"{"bar":{"enabled":false}}"#);
        let main = base(initial);
        let saved = |app: &UiApp| {
            let background = app.settings_handle.borrow().theme().background;
            background.map(iced::Color::from)
        };
        let bg = |app: &mut UiApp, event| update(app, Config(ConfigMessage::BgPalette(event)));
        let edit_theme = |app: &UiApp, edit: &dyn Fn(&mut serde_json::Value)| {
            let name = app.settings_handle.borrow().active_theme().to_owned();
            let path = dir.path().join(format!("themes/{name}.json"));
            let mut theme = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            edit(&mut theme);
            std::fs::write(path, theme.to_string()).unwrap();
        };
        let id = base(update(&mut app, ToggleConfig));
        assert_actions!(bg(&mut app, Select(Some(0))), []);
        for value in [0.5, 0.25] {
            assert_actions!(bg(&mut app, Adjust(SatVal(0.0, value))), []);
            assert_eq!(saved(&app), None);
        }
        assert_actions!(bg(&mut app, Commit), []);
        let released = saved(&app);
        assert!(released.is_some());
        assert_actions!(bg(&mut app, Adjust(SatVal(0.0, 0.75))), []);
        assert_eq!(saved(&app), released);
        // Reloading the app's own write keeps the unsaved drag.
        assert_actions!(update(&mut app, SettingsFilesChanged), []);
        assert_actions!(update(&mut app, WindowClosed(id)), []);
        let closed = saved(&app);
        assert_ne!(closed, released);
        // Outside edits that keep the saved background keep the finer color shown.
        let shown = app.theme(main).palette().background;
        edit_theme(&app, &|theme| theme["author"] = "Elsewhere".into());
        assert_actions!(update(&mut app, SettingsFilesChanged), []);
        assert_eq!(app.theme(main).palette().background, shown);
        edit_theme(&app, &|theme| theme["background"] = "#123456".into());
        assert_actions!(update(&mut app, SettingsFilesChanged), []);
        assert_eq!(Some(app.theme(main).palette().background), saved(&app));
        assert_ne!(saved(&app), closed);
        let closed = saved(&app);
        assert_actions!(bg(&mut app, Adjust(SatVal(0.0, 0.5))), []);
        assert_actions!(update(&mut app, Quit), []);
        assert_actions!(update(&mut app, Quit), [Exit]);
        assert_ne!(saved(&app), closed);
    }

    #[test]
    fn outside_settings_edits_reach_the_running_app() {
        let (dir, mut app, initial) = app(r#"{"bar":{"enabled":false}}"#);
        let main = base(initial);
        let path = dir.path().join("settings.json");
        let handle = app.settings_handle.clone();
        let edit_file = |edit: &dyn Fn(&mut serde_json::Value)| {
            handle.flush();
            let mut value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            edit(&mut value);
            let written = value.to_string();
            std::fs::write(&path, &written).unwrap();
            written
        };
        let spectrum = |app: &UiApp| {
            let snapshot = app.visual_manager.borrow().snapshot();
            snapshot
                .iter()
                .any(|slot| slot.kind() == VisualKind::Spectrum && slot.enabled)
        };

        edit_file(&|value| {
            value["visuals"]["popouts"]["spectrum"] =
                serde_json::json!({"width": 300, "height": 200});
        });
        let opened = actions(update(&mut app, SettingsFilesChanged));
        let [Output(NewBaseWindow { id, settings })] = opened.as_slice() else {
            panic!("expected one base window: {opened:?}");
        };
        assert_eq!(settings.size, Some(PixelSize::px(300, 200)));
        let popout = *id;

        edit_file(&|value| value["decorations"] = true.into());
        let actions = actions(update(&mut app, SettingsFilesChanged));
        let closed = |id| {
            actions
                .iter()
                .any(|action| matches!(action, Window(Close(c)) if *c == id))
        };
        let opened = actions
            .iter()
            .filter(|action| matches!(action, Output(NewBaseWindow { .. })));
        assert!(
            closed(main) && closed(popout) && opened.count() == 2,
            "{actions:?}"
        );

        assert!(spectrum(&app));
        let written = edit_file(&|value| {
            value["visuals"]["modules"]["spectrum"]["enabled"] = false.into();
        });
        let [(&popout, _)] = Vec::from_iter(&app.popout_windows)[..] else {
            panic!("expected one popout");
        };
        assert_actions!(update(&mut app, SettingsFilesChanged), [Window(Close(id))] if *id == popout);
        assert!(!spectrum(&app));
        assert!(
            app.settings_handle
                .borrow()
                .data
                .visuals
                .popped_out(VisualKind::Spectrum)
        );
        handle.flush();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), written);
    }

    #[test]
    fn hidden_visuals_keep_their_place_in_the_settings_order() {
        use VisualKind::*;
        let mut settings = serde_json::json!({
            "bar": {"enabled": false},
            "visuals": {"popouts": {"spectrum": {}}},
        });
        let (dir, mut app, _) = app(&settings.to_string());
        let [&popout] = Vec::from_iter(app.popout_windows.keys())[..] else {
            panic!("expected one popout");
        };
        let order = [
            Waveform,
            Spectrum,
            Spectrogram,
            Oscilloscope,
            Stereometer,
            Loudness,
        ];
        settings["visuals"]["order"] = serde_json::to_value(order).unwrap();
        std::fs::write(dir.path().join("settings.json"), settings.to_string()).unwrap();
        assert_actions!(update(&mut app, SettingsFilesChanged), []);
        let move_pane = Visuals(VisualsMessage::PaneMoved(Loudness, 0));
        assert_actions!(update(&mut app, move_pane), []);
        assert_actions!(update(&mut app, PopOutOrDock(popout)), [Window(Close(id))] if *id == popout);
        let order = [
            Loudness,
            Spectrum,
            Waveform,
            Spectrogram,
            Oscilloscope,
            Stereometer,
        ];
        assert_eq!(app.settings_handle.borrow().data.visuals.order, order);
        assert_eq!(app.visual_manager.borrow().order(), order);
    }

    #[test]
    fn one_outside_edit_of_mode_and_decorations_rebuilds_the_main_window_once() {
        let (dir, mut app, initial) = app("{}");
        assert_actions!(initial, []);
        let bar = layer(output(&mut app, 1, None, Added), None, OutputOption::Active);
        let edited = r#"{"bar":{"enabled":false},"decorations":true}"#;
        std::fs::write(dir.path().join("settings.json"), edited).unwrap();
        let actions = actions(update(&mut app, SettingsFilesChanged));
        let opened = actions.iter().filter(|action| match action {
            Output(NewBaseWindow { settings, .. }) => !settings.client_side_decorations,
            _ => false,
        });
        let closed = actions
            .iter()
            .filter(|action| matches!(action, Window(Close(id)) if *id == bar));
        assert!(
            actions.len() == 2 && opened.count() == 1 && closed.count() == 1,
            "{actions:?}"
        );
    }

    #[test]
    fn normal_window_closing_still_quits_without_outputs() {
        let (_dir, mut app, initial) = app(r#"{"bar":{"enabled":false}}"#);
        let id = base(initial);
        assert_actions!(update(&mut app, WindowClosed(id)), [Exit]);

        // Switching out of bar mode must work even when its surface is absent.
        app.main_window = MainWindow::Bar(None);
        let settings = app.settings_handle.borrow().data.clone();
        let new_id = base(app.sync_main_window(&settings, &settings));
        assert_ne!(new_id, id);
        assert_actions!(update(&mut app, WindowClosed(id)), []);
        assert_actions!(update(&mut app, WindowClosed(new_id)), [Exit]);
    }
}
