// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

macro_rules! fill {
    ($e:expr) => {
        container($e).width(Length::Fill).height(Length::Fill)
    };
}

mod message;
mod windowing;

use crate::domain::routing::RoutingCommand;
use crate::infra::pipewire::{meter_tap::AudioBatch, registry::RegistrySnapshot};
use crate::persistence::settings::{BarAlignment, BarSettings, SettingsHandle, clamp_bar_height};
use crate::ui::config::ConfigPage;
use crate::ui::settings::ActiveSettings;
use crate::ui::subscription::channel_subscription;
use crate::ui::theme;
use crate::ui::visuals::VisualsPage;
use crate::ui::widgets::scroll_glow::ScrollGlow;
use crate::visuals::registry::{VisualManager, VisualManagerHandle};
use async_channel::Receiver as AsyncReceiver;
use iced::alignment::{Horizontal, Vertical};
use iced::event::{self, Event};
use iced::widget::{column, container, mouse_area, row, stack, text};
use iced::{
    Element, Length, Settings as IcedSettings, Size, Subscription, Task, daemon as iced_daemon,
    window,
};
use iced_layershell::settings::{LayerShellSettings, Settings as LayerSettings, StartMode};
use message::{Message, keyboard_shortcut, update, view};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use windowing::{
    APP_ID, BarResizeState, PopoutWindow, layershell_available, main_window_size,
    open_config_base_window, open_main_window,
};

const TOAST_DISPLAY_DURATION: Duration = Duration::from_secs(2);
const BAR_RESIZE_HANDLE_THICKNESS: f32 = 6.0;

#[derive(Clone)]
pub(crate) struct UiConfig {
    pub(crate) routing_sender: mpsc::Sender<RoutingCommand>,
    pub(crate) registry_updates: Option<Arc<AsyncReceiver<RegistrySnapshot>>>,
    pub(crate) audio_frames: Arc<AsyncReceiver<AudioBatch>>,
    pub(crate) settings_handle: SettingsHandle,
}

pub(crate) fn run(config: UiConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if layershell_available() {
        let layer_settings = LayerShellSettings {
            start_mode: StartMode::Background,
            size: None,
            ..Default::default()
        };
        iced_layershell::daemon(
            move || UiApp::new(config.clone(), true),
            || APP_ID.to_string(),
            update,
            view,
        )
        .settings(LayerSettings {
            id: Some(APP_ID.into()),
            layer_settings,
            ..Default::default()
        })
        .subscription(UiApp::subscription)
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
    settings_handle: SettingsHandle,
    audio_frames: Arc<AsyncReceiver<AudioBatch>>,
    config_window: Option<window::Id>,
    bar_resize_state: Option<BarResizeState>,
    rendering_paused: bool,
    toast_until: Option<Instant>,
    main_window_id: window::Id,
    main_window_size: Size,
    last_base_window_size: Size,
    main_window_is_layer: bool,
    use_layershell: bool,
    settings_window: Option<(window::Id, ActiveSettings)>,
    settings_scroll: ScrollGlow,
    popout_windows: HashMap<window::Id, PopoutWindow>,
    exit_warning_until: Option<Instant>,
}

impl UiApp {
    fn new(config: UiConfig, use_layershell: bool) -> (Self, Task<Message>) {
        let UiConfig {
            routing_sender,
            registry_updates,
            audio_frames,
            settings_handle,
        } = config;
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
        let visual_manager = Rc::new(RefCell::new(manager));
        let config_page = ConfigPage::new(
            routing_sender,
            registry_updates,
            visual_manager.clone(),
            settings_handle.clone(),
            use_layershell,
        );
        let visuals_page = VisualsPage::new(visual_manager.clone(), settings_handle.clone());
        let base_size = main_window_size(main_window);
        let (main_id, open_task, main_is_layer, main_size) =
            open_main_window(use_layershell, bar_settings, base_size, use_decorations);
        let mut app = Self {
            config_page,
            visuals_page,
            visual_manager,
            settings_handle,
            audio_frames,
            config_window: None,
            bar_resize_state: None,
            rendering_paused: false,
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
            self.config_page.subscription().map(Message::Config),
            event::listen_with(keyboard_shortcut),
            window::close_events().map(Message::WindowClosed),
            window::resize_events().map(|(id, size)| Message::WindowResized(id, size)),
            event::listen_with(|evt, _, wid| match evt {
                Event::Window(window::Event::Opened { size, .. }) => {
                    Some(Message::WindowResized(wid, size))
                }
                _ => None,
            }),
        ];
        subs.push(channel_subscription(Arc::clone(&self.audio_frames)).map(Message::AudioFrame));
        if self.bar_resize_state.is_some() {
            subs.push(event::listen_with(message::bar_drag_events));
        }
        Subscription::batch(subs)
    }

    fn toggle_config_window(&mut self) -> Task<Message> {
        if let Some(id) = self.config_window.take() {
            return window::close(id);
        }
        let (id, task) = open_config_base_window(self.use_layershell);
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

    fn pending_bar_resize(&self) -> Option<(u32, u32)> {
        self.bar_resize_state
            .map(|s| (s.start_height, s.pending_height))
    }

    fn main_window_view(&self) -> Element<'_, Message> {
        let (use_decorations, bar) = {
            let guard = self.settings_handle.borrow();
            let settings = &guard.data;
            (settings.decorations, settings.bar.clone())
        };

        let content = self.visuals_with_toasts();
        let content = self.wrap_bar_resize(content, &bar);
        Self::wrap_window_resize(content, use_decorations, &bar)
    }

    fn visuals_with_toasts(&self) -> Element<'_, Message> {
        let config_open = self.config_window.is_some();
        let visuals_view = self.visuals_page.view(config_open).map(Message::Visuals);

        let now = Instant::now();
        let is_active = |deadline: Option<Instant>| deadline.is_some_and(|expires| now < expires);
        let toast_msgs: Vec<&str> = [
            (config_open && is_active(self.toast_until))
                .then_some("drag visuals to rearrange | ctrl+shift+h to close config"),
            self.rendering_paused.then_some("paused (p to resume)"),
            is_active(self.exit_warning_until).then_some("q again to exit"),
        ]
        .into_iter()
        .flatten()
        .collect();

        let mut layer = column![fill!(visuals_view)]
            .width(Length::Fill)
            .height(Length::Fill);
        if !toast_msgs.is_empty() {
            layer = layer.push(
                container(
                    row(toast_msgs
                        .iter()
                        .map(|m| container(text(*m).size(11)).padding([2, 6]).into()))
                    .spacing(12),
                )
                .width(Length::Fill)
                .align_x(Horizontal::Center),
            );
        }
        layer.into()
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
            container(text(" "))
                .width(Length::Fill)
                .height(BAR_RESIZE_HANDLE_THICKNESS),
        )
        .on_press(Message::BarResizeStart)
        .interaction(iced::mouse::Interaction::ResizingVertically);
        let handle_layer = fill!(handle).align_y(match bar.alignment {
            BarAlignment::Top => Vertical::Bottom,
            BarAlignment::Bottom => Vertical::Top,
        });

        if let Some((current, pending)) = self.pending_bar_resize() {
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

    fn wrap_window_resize<'a>(
        content: Element<'a, Message>,
        use_decorations: bool,
        bar: &BarSettings,
    ) -> Element<'a, Message> {
        if use_decorations || bar.enabled {
            return content;
        }
        let handle = mouse_area(container(text(" ")).width(20).height(20))
            .on_press(Message::Resize)
            .interaction(iced::mouse::Interaction::ResizingDiagonallyDown);
        stack![
            content,
            fill!(handle)
                .align_x(Horizontal::Right)
                .align_y(Vertical::Bottom)
                .padding(4)
        ]
        .into()
    }
}
