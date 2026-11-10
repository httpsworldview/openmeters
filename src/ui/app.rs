// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

mod message;
mod sync;
mod windowing;

use crate::infra::pipewire::{AudioReader, AudioWake, CaptureControl, audio_wake};
use crate::meter::MeterEngine;
use crate::persistence::settings::{
    BarAlignment, SettingsHandle, clamp_bar_height, settings_changes,
};
use crate::ui::config::ConfigPage;
use crate::ui::settings::ActiveSettings;
use crate::ui::theme;
use crate::ui::visuals::VisualsPage;
use crate::ui::widgets::{
    fill,
    frame_clock::{FrameCoordinator, frame_clock, frame_watchdog},
    scroll_glow::ScrollGlow,
};
use crate::visuals::registry::{VisualContent, VisualManager, VisualManagerHandle};
use iced::alignment::{Horizontal, Vertical};
use iced::event;
use iced::widget::{Space, container, mouse_area, row, stack, text};
use iced::{
    Element, Length, Settings as IcedSettings, Subscription, Task, daemon as iced_daemon, window,
};
use iced_exwlshell::settings::{LayerShellSettings, Settings as LayerSettings, StartMode};
use message::{Message, app_event, shell_event, update, view};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use windowing::{
    APP_ID, BarResizeState, MainWindow, layershell_available, open_main_window,
    open_tool_base_window,
};

const TOAST_DISPLAY_DURATION: Duration = Duration::from_secs(2);
const MAINTENANCE_INTERVAL: Duration = Duration::from_millis(100);
const BAR_RESIZE_HANDLE_THICKNESS: f32 = 6.0;

fn clock(period: &Duration) -> async_channel::Receiver<()> {
    let (sender, receiver) = async_channel::bounded(1);
    let period = *period;
    if let Err(err) = std::thread::Builder::new()
        .name("openmeters-ui-maintenance".into())
        .spawn(move || {
            while let Ok(()) | Err(async_channel::TrySendError::Full(_)) = sender.try_send(()) {
                std::thread::sleep(period);
            }
        })
    {
        tracing::error!("[ui] failed to start maintenance clock: {err}");
    }
    receiver
}

#[derive(Clone)]
pub(crate) struct UiConfig {
    pub(crate) capture: CaptureControl,
    pub(crate) audio: Rc<RefCell<Option<AudioReader>>>,
    pub(crate) settings_handle: SettingsHandle,
}

pub(crate) fn run(config: UiConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if layershell_available() {
        let (shell_broadcast, shell_events) = iced_exwlshell::shell::channel();
        iced_exwlshell::daemon(
            move || UiApp::new(config.clone(), true),
            || APP_ID.to_string(),
            update,
            view,
        )
        .settings(LayerSettings {
            id: Some(APP_ID.into()),
            layer_settings: LayerShellSettings {
                start_mode: StartMode::Background,
                ..Default::default()
            },
            shell_broadcast,
            ..Default::default()
        })
        .subscription(move |app| {
            Subscription::batch([
                app.subscription(),
                shell_events.listen().filter_map(shell_event),
            ])
        })
        .title(|app, window_id| Some(app.title(window_id)))
        .theme(|app: &UiApp, window_id| Some(app.theme(window_id)))
        .run()?;
    } else {
        iced_daemon(move || UiApp::new(config.clone(), false), update, view)
            .settings(IcedSettings {
                id: Some(APP_ID.into()),
                ..Default::default()
            })
            .subscription(UiApp::subscription)
            .title(UiApp::title)
            .theme(UiApp::theme)
            .run()?;
    }
    Ok(())
}

struct UiApp {
    config_page: ConfigPage,
    visuals_page: VisualsPage,
    visual_manager: VisualManagerHandle,
    frames: Rc<RefCell<FrameCoordinator>>,
    audio_wake: AudioWake,
    settings_handle: SettingsHandle,
    config_window: Option<window::Id>,
    bar_resize_state: Option<BarResizeState>,
    rendering_paused: bool,
    toast_until: Option<Instant>,
    main_window: MainWindow,
    use_layershell: bool,
    settings_window: Option<(window::Id, ActiveSettings)>,
    settings_scroll: ScrollGlow,
    // at most one popout exists for each VisualKind
    popout_windows: HashMap<window::Id, VisualContent>,
    exit_warning_until: Option<Instant>,
    held_shortcut: Option<(iced::keyboard::key::Physical, Instant)>,
}

impl UiApp {
    fn new(config: UiConfig, use_layershell: bool) -> (Self, Task<Message>) {
        let UiConfig {
            capture,
            audio,
            settings_handle,
        } = config;
        let settings = settings_handle.borrow().data.clone();
        let mut manager = VisualManager::default();
        manager.apply_visual_settings(&settings.visuals, None);
        manager.apply_palettes(settings_handle.borrow().theme(), None);
        let visual_manager = Rc::new(RefCell::new(manager));
        let reader = audio
            .borrow_mut()
            .take()
            .expect("audio reader already taken");
        let audio_wake = reader.wake_handle();
        let mut meter_engine = MeterEngine::new(reader, Rc::clone(&visual_manager));
        meter_engine.set_active(visual_manager.borrow().has_enabled());

        let config_page = ConfigPage::new(
            capture,
            Rc::clone(&visual_manager),
            settings_handle.clone(),
            use_layershell,
        );
        let visuals_page = VisualsPage::new(Rc::clone(&visual_manager), settings_handle.clone());
        let (main_window, open_task) =
            open_main_window(&settings, use_layershell, config_page.has_bar_outputs());
        let frames = Rc::new(RefCell::new(FrameCoordinator::new(
            meter_engine,
            settings.visual_frame_rate,
        )));
        let mut app = Self {
            config_page,
            visuals_page,
            visual_manager,
            frames,
            audio_wake,
            settings_handle,
            config_window: None,
            bar_resize_state: None,
            rendering_paused: false,
            toast_until: None,
            main_window,
            use_layershell,
            settings_window: None,
            settings_scroll: ScrollGlow::default(),
            popout_windows: HashMap::default(),
            exit_warning_until: None,
            held_shortcut: None,
        };
        let restore_popouts = app.sync_visual_windows(&settings, &settings);
        (app, Task::batch([open_task, restore_popouts]))
    }

    fn subscription(&self) -> Subscription<Message> {
        let watcher = self.settings_handle.watcher();
        let mut subs = vec![
            event::listen_with(app_event),
            Subscription::run_with(watcher, settings_changes)
                .map(|()| Message::SettingsFilesChanged),
        ];
        if self.bar_resize_state.is_some() {
            subs.push(event::listen_with(message::bar_drag_events));
        }
        if self.rendering() {
            let heartbeat = self.frames.borrow().heartbeat_handle();
            subs.extend([
                Subscription::run_with(self.audio_wake.clone(), audio_wake)
                    .map(|_| Message::AudioWake),
                Subscription::run_with(heartbeat, frame_watchdog).map(Message::Watchdog),
            ]);
        }
        if self.config_window.is_some()
            || self.toast_until.is_some()
            || self.exit_warning_until.is_some()
        {
            subs.push(Subscription::run_with(MAINTENANCE_INTERVAL, clock).map(|_| Message::Tick));
        }
        Subscription::batch(subs)
    }

    fn visuals_active(&self) -> bool {
        self.visual_manager.borrow().has_enabled()
    }

    fn rendering(&self) -> bool {
        self.visuals_active() && !self.rendering_paused
    }

    fn tick(&mut self) {
        let now = Instant::now();
        if self.config_window.is_some() {
            self.config_page.refresh_registry();
        }
        self.toast_until.take_if(|deadline| now >= *deadline);
        self.exit_warning_until.take_if(|deadline| now >= *deadline);
    }

    fn set_rendering_paused(&mut self, paused: bool) {
        self.rendering_paused = paused;
        self.frames.borrow_mut().set_paused(paused);
    }

    fn toggle_config_window(&mut self) -> Task<Message> {
        if let Some(id) = self.config_window.take() {
            self.config_page.commit_edits();
            return window::close(id);
        }
        self.config_page.refresh_registry();
        let (id, task) = open_tool_base_window(self.use_layershell);
        self.config_window = Some(id);
        self.toast_until = Some(Instant::now() + TOAST_DISPLAY_DURATION);
        task
    }

    fn begin_bar_resize(&mut self) {
        if !matches!(self.main_window, MainWindow::Bar(Some(_))) {
            return;
        }
        let height = self.settings_handle.borrow().data.bar.height;
        self.bar_resize_state = Some(BarResizeState {
            start_y: None,
            start_height: height,
            pending_height: height,
        });
    }

    fn handle_bar_resize(&mut self, position: iced::Point) {
        if let Some(state) = &mut self.bar_resize_state {
            let alignment = self.settings_handle.borrow().data.bar.alignment;
            let start_y = *state.start_y.get_or_insert(position.y);
            let delta = match alignment {
                BarAlignment::Top => position.y - start_y,
                BarAlignment::Bottom => start_y - position.y,
            };
            state.pending_height =
                clamp_bar_height((state.start_height.get() as f32 + delta).round() as u32);
        }
    }

    fn finish_bar_resize(&mut self) {
        if let Some(state) = self.bar_resize_state.take() {
            self.settings_handle
                .edit(|s| s.bar.height = state.pending_height);
        }
    }

    fn main_window_view(&self, window_id: window::Id) -> Element<'_, Message> {
        self.with_frame_clock(window_id, self.wrap_bar_resize(self.visuals_with_toasts()))
    }

    fn with_frame_clock<'a>(
        &self,
        window: window::Id,
        content: Element<'a, Message>,
    ) -> Element<'a, Message> {
        let is_main = Some(window) == self.main_window.id();
        let clock =
            (self.rendering()).then(|| frame_clock(Rc::clone(&self.frames), window, is_main));
        // The optional layer comes last, so pausing or a toast keeps the content's widget state.
        stack![content, clock].into()
    }

    fn visuals_with_toasts(&self) -> Element<'_, Message> {
        let config_open = self.config_window.is_some();
        let visuals_view = self.visuals_page.view(config_open).map(Message::Visuals);

        let now = Instant::now();
        let is_active = |deadline: Option<Instant>| deadline.is_some_and(|expires| now < expires);
        let toast_msgs = [
            (config_open && is_active(self.toast_until))
                .then_some("drag visuals to rearrange | ctrl+shift+h to close config"),
            self.rendering_paused.then_some("paused (p to resume)"),
            is_active(self.exit_warning_until).then_some("q again to exit"),
        ];

        let overlay = toast_msgs.iter().any(Option::is_some).then(|| {
            let toast = container(
                row(toast_msgs
                    .into_iter()
                    .flatten()
                    .map(|m| container(text(m).size(11)).padding([2, 6]).into()))
                .spacing(12),
            )
            .padding([6, 10])
            .style(theme::weak_container);
            fill(toast)
                .padding(8)
                .align_x(Horizontal::Center)
                .align_y(Vertical::Bottom)
        });
        stack![visuals_view, overlay].into()
    }

    fn wrap_bar_resize<'a>(&'a self, content: Element<'a, Message>) -> Element<'a, Message> {
        let bar = &self.settings_handle.borrow().data.bar;
        if !self.main_window.is_bar() {
            return content;
        }
        let handle = mouse_area(
            Space::new()
                .width(Length::Fill)
                .height(BAR_RESIZE_HANDLE_THICKNESS),
        )
        .on_press(Message::BarResizeStart)
        // Drag events only flow once the press is handled, so a release batched with it ends here.
        .on_release(Message::BarResizeEnd)
        .interaction(iced::mouse::Interaction::ResizingVertically);
        let handle_layer = fill(handle).align_y(match bar.alignment {
            BarAlignment::Top => Vertical::Bottom,
            BarAlignment::Bottom => Vertical::Top,
        });
        let overlay = self.bar_resize_state.as_ref().map(|state| {
            let (current, pending) = (state.start_height.get(), state.pending_height.get());
            container(text(format!("{current}px -> {pending}px")).size(14))
                .center(Length::Fill)
                .style(theme::resize_overlay)
        });
        stack![content, overlay, handle_layer].into()
    }
}
