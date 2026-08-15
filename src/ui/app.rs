// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

mod message;
mod windowing;

use crate::infra::pipewire::{AudioReader, CaptureControl, audio_wake};
use crate::meter::MeterEngine;
use crate::persistence::settings::{BarAlignment, BarSettings, SettingsHandle, clamp_bar_height};
use crate::ui::config::ConfigPage;
use crate::ui::settings::ActiveSettings;
use crate::ui::theme;
use crate::ui::visuals::VisualsPage;
use crate::ui::widgets::{
    fill,
    frame_clock::{FrameCoordinator, frame_clock, frame_watchdog},
    scroll_glow::ScrollGlow,
};
use crate::visuals::registry::{VisualManager, VisualManagerHandle};
use iced::alignment::{Horizontal, Vertical};
use iced::event;
use iced::widget::{Space, container, mouse_area, row, stack, text};
use iced::{
    Element, Length, Settings as IcedSettings, Size, Subscription, Task, daemon as iced_daemon,
    window,
};
use iced_layershell::settings::{LayerShellSettings, Settings as LayerSettings, StartMode};
use message::{Message, keyboard_shortcut, update, view};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use windowing::{
    APP_ID, BarResizeState, PopoutWindow, layershell_available, main_window_size, open_main_window,
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
        let (shell_broadcast, shell_events) = iced_layershell::shell::channel();
        iced_layershell::daemon(
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
                shell_events
                    .listen()
                    .map(|event| Message::Shell(Box::new(event))),
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
    settings_handle: SettingsHandle,
    config_window: Option<window::Id>,
    bar_resize_state: Option<BarResizeState>,
    rendering_paused: bool,
    next_maintenance: Instant,
    toast_until: Option<Instant>,
    main_window_id: window::Id,
    main_window_size: Size,
    last_base_window_size: Size,
    main_window_is_layer: bool,
    use_layershell: bool,
    settings_window: Option<(window::Id, ActiveSettings)>,
    settings_scroll: ScrollGlow,
    // at most one popout exists for each VisualKind
    popout_windows: HashMap<window::Id, PopoutWindow>,
    exit_warning_until: Option<Instant>,
}

impl UiApp {
    fn new(config: UiConfig, use_layershell: bool) -> (Self, Task<Message>) {
        let UiConfig {
            capture,
            audio,
            settings_handle,
        } = config;
        let visual_frame_rate = settings_handle.borrow().data.visual_frame_rate;
        let (visual_settings, use_decorations, bar_settings, main_window, theme_file) = {
            let guard = settings_handle.borrow();
            let settings = &guard.data;
            (
                settings.visuals.clone(),
                settings.decorations,
                settings.bar.clone(),
                settings.main_window,
                guard.theme_store().load(guard.active_theme()),
            )
        };
        let mut manager = VisualManager::default();
        manager.apply_visual_settings(&visual_settings);
        if let Some(theme_file) = theme_file {
            manager.apply_theme(&theme_file);
        }
        let visuals_active = manager.has_enabled();
        let visual_manager = Rc::new(RefCell::new(manager));
        let reader = audio
            .borrow_mut()
            .take()
            .expect("audio reader already taken");
        let mut meter_engine = MeterEngine::new(reader, visual_manager.clone());
        if !visuals_active {
            meter_engine.set_active(false);
        }

        let config_page = ConfigPage::new(
            capture,
            visual_manager.clone(),
            settings_handle.clone(),
            use_layershell,
        );
        let visuals_page = VisualsPage::new(visual_manager.clone(), settings_handle.clone());
        let base_size = main_window_size(main_window);
        let (main_id, open_task, main_is_layer, main_size) =
            open_main_window(use_layershell, bar_settings, base_size, use_decorations);
        let frames = Rc::new(RefCell::new(FrameCoordinator::new(
            meter_engine,
            visual_frame_rate,
        )));
        let mut app = Self {
            config_page,
            visuals_page,
            visual_manager,
            frames,
            settings_handle,
            config_window: None,
            bar_resize_state: None,
            rendering_paused: false,
            next_maintenance: Instant::now(),
            toast_until: None,
            main_window_id: main_id,
            main_window_size: main_size,
            last_base_window_size: base_size,
            main_window_is_layer: main_is_layer,
            use_layershell,
            settings_window: None,
            settings_scroll: ScrollGlow::default(),
            popout_windows: HashMap::default(),
            exit_warning_until: None,
        };
        let restore_popouts = app.restore_popout_windows(&visual_settings.popouts);
        if !app.popout_windows.is_empty() {
            app.sync_visuals_page();
        }
        (app, Task::batch([open_task, restore_popouts]))
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            event::listen_with(keyboard_shortcut),
            window::close_events().map(Message::WindowClosed),
            window::resize_events().map(|(id, size)| Message::WindowResized(id, size)),
        ];
        if self.bar_resize_state.is_some() {
            subs.push(event::listen_with(message::bar_drag_events));
        }
        if self.visuals_active() && !self.rendering_paused {
            let frames = self.frames.borrow();
            subs.push(
                Subscription::run_with(frames.wake_handle(), audio_wake)
                    .map(|_| Message::AudioWake),
            );
            subs.push(
                Subscription::run_with(frames.heartbeat_handle(), frame_watchdog)
                    .map(Message::Watchdog),
            );
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

    fn tick(&mut self) {
        let now = Instant::now();
        if now >= self.next_maintenance {
            if self.config_window.is_some() {
                self.config_page.refresh_registry();
            }
            self.toast_until.take_if(|deadline| now >= *deadline);
            self.exit_warning_until.take_if(|deadline| now >= *deadline);
            self.next_maintenance = now + MAINTENANCE_INTERVAL;
        }
    }

    fn set_rendering_paused(&mut self, paused: bool) {
        self.rendering_paused = paused;
        self.frames.borrow_mut().set_paused(paused, Instant::now());
    }

    fn toggle_config_window(&mut self) -> Task<Message> {
        if let Some(id) = self.config_window.take() {
            return window::close(id);
        }
        self.config_page.refresh_registry();
        let (id, task) = open_tool_base_window(self.use_layershell);
        self.config_window = Some(id);
        self.toast_until = Some(Instant::now() + TOAST_DISPLAY_DURATION);
        task
    }

    fn begin_bar_resize(&mut self) {
        if !self.main_window_is_layer {
            return;
        }
        let (enabled, height, alignment) = {
            let settings = self.settings_handle.borrow();
            let bar = &settings.data.bar;
            (bar.enabled, clamp_bar_height(bar.height), bar.alignment)
        };
        if !enabled {
            return;
        }
        let start_y = match alignment {
            BarAlignment::Top => height as f32,
            BarAlignment::Bottom => 0.0,
        };
        self.bar_resize_state = Some(BarResizeState {
            start_y,
            start_height: height,
            pending_height: height,
        });
    }

    fn handle_bar_resize(&mut self, position: iced::Point) {
        if let Some(state) = &mut self.bar_resize_state {
            let alignment = self.settings_handle.borrow().data.bar.alignment;
            let delta = match alignment {
                BarAlignment::Top => position.y - state.start_y,
                BarAlignment::Bottom => state.start_y - position.y,
            };
            state.pending_height =
                clamp_bar_height((state.start_height as f32 + delta).round().max(1.0) as u32);
        }
    }

    fn finish_bar_resize(&mut self) -> Task<Message> {
        self.bar_resize_state
            .take()
            .filter(|s| s.pending_height != s.start_height)
            .map_or_else(Task::none, |s| {
                let alignment = self.settings_handle.borrow().data.bar.alignment;
                self.settings_handle
                    .update(|settings| settings.data.bar.height = s.pending_height);
                self.apply_bar_layout(alignment, s.pending_height)
            })
    }

    fn main_window_view(&self) -> Element<'_, Message> {
        let bar = self.settings_handle.borrow().data.bar.clone();
        let content = self.visuals_with_toasts();
        let content = self.wrap_bar_resize(content, &bar);
        self.with_frame_clock(self.main_window_id, content)
    }

    fn with_frame_clock<'a>(
        &self,
        window: window::Id,
        content: Element<'a, Message>,
    ) -> Element<'a, Message> {
        if self.visuals_active() && !self.rendering_paused {
            stack![
                content,
                frame_clock(
                    Rc::clone(&self.frames),
                    window,
                    window == self.main_window_id,
                )
            ]
            .into()
        } else {
            content
        }
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

        let base: Element<'_, Message> = visuals_view;
        if !toast_msgs.iter().any(Option::is_some) {
            return base;
        }
        let toast = container(
            row(toast_msgs
                .into_iter()
                .flatten()
                .map(|m| container(text(m).size(11)).padding([2, 6]).into()))
            .spacing(12),
        )
        .padding([6, 10])
        .style(theme::weak_container);
        let overlay = fill(toast)
            .padding(8)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Bottom);
        stack![base, overlay].into()
    }

    fn wrap_bar_resize<'a>(
        &'a self,
        content: Element<'a, Message>,
        bar: &BarSettings,
    ) -> Element<'a, Message> {
        if !(self.main_window_is_layer && bar.enabled) {
            return content;
        }
        let handle = mouse_area(
            Space::new()
                .width(Length::Fill)
                .height(BAR_RESIZE_HANDLE_THICKNESS),
        )
        .on_press(Message::BarResizeStart)
        .interaction(iced::mouse::Interaction::ResizingVertically);
        let handle_layer = fill(handle).align_y(match bar.alignment {
            BarAlignment::Top => Vertical::Bottom,
            BarAlignment::Bottom => Vertical::Top,
        });

        if let Some(state) = &self.bar_resize_state {
            let (current, pending) = (state.start_height, state.pending_height);
            let overlay: Element<'_, Message> =
                container(text(format!("{current}px -> {pending}px")).size(14))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Horizontal::Center)
                    .align_y(Vertical::Center)
                    .style(theme::resize_overlay)
                    .into();
            stack![content, overlay, handle_layer].into()
        } else {
            stack![content, handle_layer].into()
        }
    }
}
