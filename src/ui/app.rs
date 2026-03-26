// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

// Main application logic.

// Wraps content in a container that expands to fill available space.
macro_rules! fill {
    ($e:expr) => {
        container($e).width(Length::Fill).height(Length::Fill)
    };
}

mod message;
mod windowing;

use crate::domain::routing::RoutingCommand;
use crate::infra::pipewire::registry::RegistrySnapshot;
use crate::persistence::settings::{BarAlignment, BarSettings, SettingsHandle, clamp_bar_height};
use crate::ui::pages::config::ConfigPage;
use crate::ui::pages::visuals::{ActiveSettings, VisualsPage};
use crate::ui::subscription::channel_subscription;
use crate::ui::theme;
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
use std::collections::HashMap;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use windowing::{
    BarResizeState, MAIN_WINDOW_INITIAL_SIZE, PopoutWindow, layershell_available, namespace,
    open_main_window,
};

const TOAST_DISPLAY_DURATION: Duration = Duration::from_secs(2);
const DEFAULT_DRAWER_RATIO: f32 = 0.20;
const MIN_DRAWER_RATIO: f32 = 0.10;
const MAX_DRAWER_RATIO: f32 = 0.50;
const BAR_RESIZE_HANDLE_THICKNESS: f32 = 6.0;
const DRAWER_RESIZE_HANDLE_WIDTH: f32 = 6.0;

pub type UiResult = std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone)]
pub struct UiConfig {
    routing_sender: mpsc::Sender<RoutingCommand>,
    registry_updates: Option<Arc<AsyncReceiver<RegistrySnapshot>>>,
    audio_frames: Option<Arc<AsyncReceiver<Vec<f32>>>>,
}

impl UiConfig {
    pub fn new(
        routing_sender: mpsc::Sender<RoutingCommand>,
        registry_updates: Option<Arc<AsyncReceiver<RegistrySnapshot>>>,
    ) -> Self {
        Self {
            routing_sender,
            registry_updates,
            audio_frames: None,
        }
    }

    pub fn with_audio_stream(mut self, rx: Arc<AsyncReceiver<Vec<f32>>>) -> Self {
        self.audio_frames = Some(rx);
        self
    }
}

pub fn run(config: UiConfig) -> UiResult {
    if layershell_available() {
        let layer_settings = LayerShellSettings {
            start_mode: StartMode::Background,
            size: None,
            ..Default::default()
        };
        iced_layershell::daemon(
            move || UiApp::new(config.clone(), true),
            namespace,
            update,
            view,
        )
        .settings(LayerSettings {
            id: Some("openmeters-ui".into()),
            layer_settings,
            ..Default::default()
        })
        .subscription(UiApp::subscription)
        .title(|app, window_id| Some(app.title(window_id)))
        .theme(|app: &UiApp, window_id| Some(app.theme(window_id)))
        .run()
        .map_err(|e| Box::new(e) as _)
    } else {
        iced_daemon(move || UiApp::new(config.clone(), false), update, view)
            .settings(IcedSettings {
                id: Some("openmeters-ui".into()),
                ..Default::default()
            })
            .subscription(UiApp::subscription)
            .title(UiApp::title)
            .theme(UiApp::theme)
            .run()
            .map_err(|e| Box::new(e) as _)
    }
}

#[derive(Debug)]
struct UiApp {
    config_page: ConfigPage,
    visuals_page: VisualsPage,
    visual_manager: VisualManagerHandle,
    settings_handle: SettingsHandle,
    audio_frames: Option<Arc<AsyncReceiver<Vec<f32>>>>,
    drawer_open: bool,
    drawer_width_ratio: f32,
    drawer_resizing: bool,
    drawer_resize_offset: Option<f32>,
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
    focused_window: Option<window::Id>,
    exit_warning_until: Option<Instant>,
}

impl UiApp {
    fn new(config: UiConfig, use_layershell: bool) -> (Self, Task<Message>) {
        let UiConfig {
            routing_sender,
            registry_updates,
            audio_frames,
        } = config;
        let settings_handle = SettingsHandle::load_or_default();
        let (visual_settings, use_decorations, bar_settings) = {
            let guard = settings_handle.borrow();
            (
                guard.settings().visuals.clone(),
                guard.settings().decorations,
                guard.settings().bar.clone(),
            )
        };
        let mut manager = VisualManager::new();
        manager.apply_visual_settings(&visual_settings);
        // Theme palettes override whatever settings.json had
        {
            let guard = settings_handle.borrow();
            let theme_name = guard.active_theme();
            if let Some(theme_file) = guard.theme_store().load(theme_name) {
                manager.apply_theme(&theme_file);
            }
        }
        let visual_manager = VisualManagerHandle::new(manager);
        let config_page = ConfigPage::new(
            routing_sender,
            registry_updates,
            visual_manager.clone(),
            settings_handle.clone(),
            use_layershell,
        );
        let visuals_page = VisualsPage::new(visual_manager.clone(), settings_handle.clone());
        let base_size = MAIN_WINDOW_INITIAL_SIZE;
        let (main_id, open_task, main_is_layer, main_size) =
            open_main_window(use_layershell, bar_settings, base_size, use_decorations);
        (
            Self {
                config_page,
                visuals_page,
                visual_manager,
                settings_handle,
                audio_frames,
                drawer_open: false,
                drawer_width_ratio: DEFAULT_DRAWER_RATIO,
                drawer_resizing: false,
                drawer_resize_offset: None,
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
                focused_window: Some(main_id),
                exit_warning_until: None,
            },
            open_task,
        )
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            self.config_page.subscription().map(Message::Config),
            event::listen_with(keyboard_shortcut),
            window::close_events().map(Message::WindowClosed),
            window::resize_events().map(|(id, size)| Message::WindowResized(id, size)),
            event::listen_with(|evt, _, wid| {
                matches!(evt, Event::Window(window::Event::Focused))
                    .then_some(Message::WindowFocused(wid))
            }),
        ];
        if let Some(rx) = &self.audio_frames {
            subs.push(channel_subscription(Arc::clone(rx)).map(Message::AudioFrame));
        }
        if self.drawer_resizing && self.drawer_open {
            subs.push(event::listen_with(message::drawer_drag_events));
        }
        if self.bar_resize_state.is_some() {
            subs.push(event::listen_with(message::bar_drag_events));
        }
        Subscription::batch(subs)
    }

    fn toggle_drawer(&mut self) {
        self.drawer_open = !self.drawer_open;
        self.end_drawer_resize();
        self.toast_until = self
            .drawer_open
            .then(|| Instant::now() + TOAST_DISPLAY_DURATION);
    }

    fn end_drawer_resize(&mut self) {
        self.drawer_resizing = false;
        self.drawer_resize_offset = None;
    }

    fn handle_drawer_resize(&mut self, position: iced::Point) {
        if self.drawer_resizing && self.drawer_width_ratio > 0.0 {
            let estimated_width = self.drawer_resize_offset.get_or_insert_with(|| {
                (position.x - DRAWER_RESIZE_HANDLE_WIDTH) / self.drawer_width_ratio
            });
            if *estimated_width > 0.0 {
                self.drawer_width_ratio =
                    (position.x / *estimated_width).clamp(MIN_DRAWER_RATIO, MAX_DRAWER_RATIO);
            }
        }
    }

    fn begin_bar_resize(&mut self) {
        if !self.main_window_is_layer {
            return;
        }
        let bar = self.settings_handle.borrow().settings().bar.clone();
        if !bar.enabled {
            return;
        }
        let height = clamp_bar_height(bar.height);
        let start_y = match bar.alignment {
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
            let alignment = self.settings_handle.borrow().settings().bar.alignment;
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
            .map(|s| {
                let alignment = self.settings_handle.borrow().settings().bar.alignment;
                self.settings_handle
                    .update(|settings| settings.set_bar_height(s.pending_height));
                self.apply_bar_layout(alignment, s.pending_height)
            })
            .unwrap_or_else(Task::none)
    }

    fn pending_bar_resize(&self) -> Option<(u32, u32)> {
        self.bar_resize_state
            .map(|s| (s.start_height, s.pending_height))
    }

    fn main_window_view(&self) -> Element<'_, Message> {
        let settings_ref = self.settings_handle.borrow();
        let use_decorations = settings_ref.settings().decorations;
        let bar = settings_ref.settings().bar.clone();
        drop(settings_ref);

        let content = self.visuals_with_toasts();
        let content = self.wrap_drawer(content);
        let content = self.wrap_bar_resize(content, &bar);
        self.wrap_window_resize(content, use_decorations, &bar)
    }

    fn visuals_with_toasts(&self) -> Element<'_, Message> {
        let visuals_view = self
            .visuals_page
            .view(self.drawer_open)
            .map(Message::Visuals);

        let now = Instant::now();
        let is_active = |deadline: Option<Instant>| deadline.is_some_and(|expires| now < expires);
        let toast_msgs: Vec<&str> = [
            (self.drawer_open && is_active(self.toast_until))
                .then_some("ctrl+shift+h to close drawer"),
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
                        .map(|m| container(text(*m).size(11)).padding([2, 6]).into())
                        .collect::<Vec<_>>())
                    .spacing(12),
                )
                .width(Length::Fill)
                .align_x(Horizontal::Center),
            );
        }
        layer.into()
    }

    fn wrap_drawer<'a>(&'a self, visuals: Element<'a, Message>) -> Element<'a, Message> {
        if !self.drawer_open {
            return visuals;
        }
        let drawer_portion = (self.drawer_width_ratio * 1000.0).round() as u16;
        let visuals_portion = 1000 - drawer_portion;
        let drawer: Element<'_, Message> = fill!(self.config_page.view().map(Message::Config))
            .width(Length::FillPortion(drawer_portion))
            .style(theme::opaque_container)
            .into();
        let handle: Element<'_, Message> = mouse_area(
            container(text(":").size(12).align_x(Horizontal::Center))
                .width(12)
                .height(Length::Fill)
                .align_x(Horizontal::Center)
                .align_y(Vertical::Center)
                .style(theme::resize_handle_container),
        )
        .on_press(Message::DrawerResizeStart)
        .interaction(iced::mouse::Interaction::ResizingHorizontally)
        .into();
        let visuals: Element<'_, Message> = fill!(visuals)
            .width(Length::FillPortion(visuals_portion))
            .into();
        row![drawer, handle, visuals]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
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
        let v_align = match bar.alignment {
            BarAlignment::Top => Vertical::Bottom,
            BarAlignment::Bottom => Vertical::Top,
        };
        let handle_layer = fill!(handle).align_y(v_align);

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
        &'a self,
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
