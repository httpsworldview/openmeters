// Main application logic.

pub mod config;
pub mod visuals;

// Wraps content in a container that expands to fill available space.
macro_rules! fill {
    ($e:expr) => {
        container($e).width(Length::Fill).height(Length::Fill)
    };
}

mod message;
mod windowing;

use crate::audio::pw_registry::RegistrySnapshot;
use crate::ui::channel_subscription::channel_subscription;
use crate::ui::settings::{BarAlignment, BarSettings, SettingsHandle, clamp_bar_height};
use crate::ui::theme;
use crate::ui::visualization::visual_manager::{
    VisualId, VisualKind, VisualManager, VisualManagerHandle,
};
use async_channel::Receiver as AsyncReceiver;
use config::{ConfigMessage, ConfigPage};
use iced::alignment::{Horizontal, Vertical};
use iced::event::{self, Event};
use iced::widget::{column, container, mouse_area, row, scrollable, stack, text};
use iced::{
    Element, Length, Settings as IcedSettings, Size, Subscription, Task, daemon as iced_daemon,
    exit, window,
};
use iced_layershell::settings::{LayerShellSettings, Settings as LayerSettings, StartMode};
use message::{Message, keyboard_shortcut};
use rustc_hash::FxHashMap;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use visuals::{
    ActiveSettings, SettingsMessage, VisualsMessage, VisualsPage, create_settings_panel,
};
use windowing::{
    BarResizeState, MAIN_WINDOW_INITIAL_SIZE, PopoutWindow, SETTINGS_WINDOW_SIZE, bar_anchor,
    layershell_available, namespace, open_base_window, open_main_window,
};

pub use config::RoutingCommand;

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
    popout_windows: FxHashMap<window::Id, PopoutWindow>,
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
                popout_windows: FxHashMap::default(),
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

    fn open_settings_window(&mut self, visual_id: VisualId, kind: VisualKind) -> Task<Message> {
        let new_panel = create_settings_panel(visual_id, kind, &self.visual_manager);
        let previous = self.settings_window.take();
        if previous
            .as_ref()
            .is_some_and(|(_, panel)| panel.visual_id() == visual_id)
        {
            self.settings_window = previous.map(|(id, _)| (id, new_panel));
            return Task::none();
        }
        let (new_id, open_task) =
            open_base_window(self.use_layershell, SETTINGS_WINDOW_SIZE, true, false);
        self.settings_window = Some((new_id, new_panel));
        match previous {
            Some((old_id, _)) => Task::batch([window::close(old_id), open_task]),
            None => open_task,
        }
    }

    fn open_popout_window(&mut self, visual_id: VisualId, kind: VisualKind) -> Task<Message> {
        if self
            .popout_windows
            .values()
            .any(|popout| popout.visual_id == visual_id)
        {
            return Task::none();
        }
        let snapshot = self.visual_manager.snapshot();
        let Some((index, slot)) = snapshot
            .slots
            .iter()
            .enumerate()
            .find(|(_, s)| s.id == visual_id)
        else {
            return Task::none();
        };
        let window_size = Size::new(
            slot.metadata.preferred_width.max(400.0),
            slot.metadata.preferred_height.max(300.0),
        );
        let (new_id, open_task) = open_base_window(self.use_layershell, window_size, true, true);
        let mut popout = PopoutWindow {
            visual_id,
            kind,
            original_index: index,
            cached: None,
        };
        popout.sync_from_snapshot(&snapshot);
        self.popout_windows.insert(new_id, popout);
        open_task
    }

    fn on_window_closed(&mut self, id: window::Id) -> Task<Message> {
        if id == self.main_window_id {
            return exit();
        }
        if self.settings_window.as_ref().is_some_and(|(w, _)| *w == id) {
            self.settings_window = None
        }
        self.popout_windows.remove(&id);
        Task::none()
    }

    fn popped_out_ids(&self) -> Vec<VisualId> {
        self.popout_windows.values().map(|w| w.visual_id).collect()
    }

    fn sync_all_windows(&mut self) -> Task<Message> {
        let snapshot = self.visual_manager.snapshot();
        let close_settings_task = self
            .settings_window
            .take_if(|(_, panel)| {
                !snapshot
                    .slots
                    .iter()
                    .any(|slot| slot.id == panel.visual_id() && slot.enabled)
            })
            .map(|(id, _)| window::close::<Message>(id));
        self.popout_windows
            .values_mut()
            .for_each(|popout| popout.sync_from_snapshot(&snapshot));
        let stale_windows: Vec<_> = self
            .popout_windows
            .iter()
            .filter_map(|(id, popout)| popout.cached.is_none().then_some(*id))
            .collect();
        self.popout_windows
            .retain(|_, popout| popout.cached.is_some());
        self.visuals_page
            .apply_snapshot_excluding(snapshot, &self.popped_out_ids());
        Task::batch(
            close_settings_task
                .into_iter()
                .chain(stale_windows.into_iter().map(window::close)),
        )
    }

    fn title(&self, window_id: window::Id) -> String {
        if window_id == self.main_window_id {
            return "OpenMeters".into();
        }
        self.settings_window
            .as_ref()
            .filter(|(id, _)| *id == window_id)
            .map(|(_, panel)| (panel.visual_id(), " settings"))
            .or_else(|| {
                self.popout_windows
                    .get(&window_id)
                    .map(|p| (p.visual_id, ""))
            })
            .and_then(|(visual_id, suffix)| {
                self.visual_manager
                    .snapshot()
                    .slots
                    .iter()
                    .find(|s| s.id == visual_id)
                    .map(|s| format!("{}{} - OpenMeters", s.metadata.display_name, suffix))
            })
            .unwrap_or_else(|| "OpenMeters".into())
    }

    fn theme(&self, window_id: window::Id) -> iced::Theme {
        let custom_bg = (window_id == self.main_window_id
            || self.popout_windows.contains_key(&window_id))
        .then(|| self.settings_handle.borrow().settings().background_color)
        .flatten();
        theme::theme(custom_bg.map(Into::into))
    }

    fn main_window_view(&self) -> Element<'_, Message> {
        let settings_ref = self.settings_handle.borrow();
        let use_decorations = settings_ref.settings().decorations;
        let bar = settings_ref.settings().bar.clone();
        drop(settings_ref);

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

        let mut visuals_layer = column![fill!(visuals_view)]
            .width(Length::Fill)
            .height(Length::Fill);
        if !toast_msgs.is_empty() {
            visuals_layer = visuals_layer.push(
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
        let visuals_layer: Element<'_, Message> = visuals_layer.into();

        let content: Element<'_, Message> = if self.drawer_open {
            let drawer_portion = (self.drawer_width_ratio * 1000.0).round() as u16;
            let visuals_portion = 1000 - drawer_portion;
            let drawer: Element<'_, Message> = fill!(self.config_page.view().map(Message::Config))
                .width(Length::FillPortion(drawer_portion))
                .style(theme::opaque_container)
                .into();
            let resize_handle: Element<'_, Message> = mouse_area(
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
            let visuals: Element<'_, Message> = fill!(visuals_layer)
                .width(Length::FillPortion(visuals_portion))
                .into();
            row![drawer, resize_handle, visuals]
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            visuals_layer
        };

        let content: Element<'_, Message> = if self.main_window_is_layer && bar.enabled {
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
        } else {
            content
        };

        if use_decorations || bar.enabled {
            content
        } else {
            let resize_handle = mouse_area(container(text(" ")).width(20).height(20))
                .on_press(Message::Resize)
                .interaction(iced::mouse::Interaction::ResizingDiagonallyDown);
            stack![
                content,
                fill!(resize_handle)
                    .align_x(Horizontal::Right)
                    .align_y(Vertical::Bottom)
                    .padding(4)
            ]
            .into()
        }
    }

    fn handle_popout_or_dock(&mut self, source_window: window::Id) -> Task<Message> {
        if let Some(popout) = self.popout_windows.remove(&source_window) {
            self.visual_manager
                .borrow_mut()
                .restore_position(popout.visual_id, popout.original_index);
            self.sync_visuals_page();
            self.settings_handle.update(|settings| {
                settings
                    .set_visual_order(self.visual_manager.snapshot().slots.iter().map(|s| s.kind))
            });
            return window::close(source_window);
        }
        let Some((id, kind)) = self.visuals_page.hovered_visual() else {
            return Task::none();
        };
        let task = self.open_popout_window(id, kind);
        self.sync_visuals_page();
        task
    }

    fn sync_visuals_page(&mut self) {
        self.visuals_page
            .apply_snapshot_excluding(self.visual_manager.snapshot(), &self.popped_out_ids());
    }

    fn apply_bar_layout(&mut self, alignment: BarAlignment, height: u32) -> Task<Message> {
        if !self.main_window_is_layer {
            return Task::none();
        }
        let height = clamp_bar_height(height);
        self.main_window_size.height = height as f32;
        Task::batch([
            Task::done(Message::AnchorSizeChange {
                id: self.main_window_id,
                anchor: bar_anchor(alignment),
                size: (0, height),
            }),
            Task::done(Message::ExclusiveZoneChange {
                id: self.main_window_id,
                zone_size: height as i32,
            }),
        ])
    }

    fn handle_main_window_resize(
        &mut self,
        window_id: window::Id,
        new_size: Size,
    ) -> Task<Message> {
        if window_id != self.main_window_id {
            return Task::none();
        }

        self.main_window_size = new_size;
        if self.main_window_is_layer {
            let height = clamp_bar_height(new_size.height.round().max(1.0) as u32);
            let current_height = self.settings_handle.borrow().settings().bar.height;
            if current_height != height {
                self.settings_handle.update(|s| s.set_bar_height(height));
            }
            return Task::done(Message::ExclusiveZoneChange {
                id: self.main_window_id,
                zone_size: height as i32,
            });
        }

        self.last_base_window_size = new_size;
        Task::none()
    }

    fn recreate_main_window(
        &mut self,
        bar_settings: BarSettings,
        use_decorations: bool,
    ) -> Task<Message> {
        let old_main_id = self.main_window_id;
        let (new_main_id, open_main, main_is_layer, main_size) = open_main_window(
            self.use_layershell,
            bar_settings,
            self.last_base_window_size,
            use_decorations,
        );
        self.main_window_id = new_main_id;
        self.main_window_size = main_size;
        self.main_window_is_layer = main_is_layer;
        self.focused_window = Some(new_main_id);
        Task::batch([open_main, window::close(old_main_id)])
    }

    fn handle_bar_config_message(&mut self, config_msg: &ConfigMessage) -> Task<Message> {
        if !self.use_layershell {
            return Task::none();
        }
        let bar = self.settings_handle.borrow().settings().bar.clone();
        match config_msg {
            ConfigMessage::BarModeToggled(enabled) if *enabled == self.main_window_is_layer => {
                if self.main_window_is_layer {
                    self.apply_bar_layout(bar.alignment, bar.height)
                } else {
                    Task::none()
                }
            }
            ConfigMessage::BarModeToggled(enabled) => {
                let decorations = self.settings_handle.borrow().settings().decorations;
                self.recreate_main_window(
                    BarSettings {
                        enabled: *enabled,
                        ..bar
                    },
                    decorations,
                )
            }
            ConfigMessage::BarAlignmentChanged(alignment) if self.main_window_is_layer => {
                self.apply_bar_layout(*alignment, bar.height)
            }
            ConfigMessage::BarHeightChanged(height) if self.main_window_is_layer => {
                self.apply_bar_layout(bar.alignment, *height as u32)
            }
            _ => Task::none(),
        }
    }

    fn recreate_settings_window(&mut self) -> Task<Message> {
        let Some((old_id, panel)) = self.settings_window.take() else {
            return Task::none();
        };
        let visual_id = panel.visual_id();
        let snapshot = self.visual_manager.snapshot();
        let Some(slot) = snapshot.slots.iter().find(|s| s.id == visual_id) else {
            return window::close(old_id);
        };
        let (new_id, open_task) =
            open_base_window(self.use_layershell, SETTINGS_WINDOW_SIZE, true, false);
        self.settings_window = Some((
            new_id,
            create_settings_panel(visual_id, slot.kind, &self.visual_manager),
        ));
        Task::batch([open_task, window::close(old_id)])
    }

    fn recreate_windows(&mut self, use_decorations: bool) -> Task<Message> {
        let old_main_id = self.main_window_id;
        let (new_main_id, open_main) = open_base_window(
            self.use_layershell,
            self.main_window_size,
            use_decorations,
            true,
        );
        self.main_window_id = new_main_id;
        self.main_window_is_layer = false;
        let settings_task = self.recreate_settings_window();
        Task::batch([open_main, window::close(old_main_id), settings_task])
    }
}

fn update(app: &mut UiApp, msg: Message) -> Task<Message> {
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
            Task::batch([
                app.config_page.update(config_msg).map(Message::Config),
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
        // Layer shell infrastructure messages — handled internally by iced_layershell
        _ => Task::none(),
    }
}

fn view(app: &UiApp, window_id: window::Id) -> Element<'_, Message> {
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
