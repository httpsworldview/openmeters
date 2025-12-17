//! Main application logic.

pub mod config;
pub mod visuals;

use crate::audio::pw_registry::RegistrySnapshot;
use crate::ui::channel_subscription::channel_subscription;
use crate::ui::settings::SettingsHandle;
use crate::ui::theme;
use crate::ui::visualization::visual_manager::{
    VisualContent, VisualId, VisualKind, VisualManager, VisualManagerHandle, VisualMetadata,
    VisualSnapshot,
};
use async_channel::Receiver as AsyncReceiver;
use config::{ConfigMessage, ConfigPage};
use iced::alignment::{Horizontal, Vertical};
use iced::event::{self, Event};
use iced::keyboard::{self, Key};
use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, mouse_area, row, scrollable, stack, text};
use iced::{Element, Length, Result, Settings, Size, Subscription, Task, daemon, exit, window};
use rustc_hash::FxHashMap;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use visuals::{
    ActiveSettings, SettingsMessage, VisualsMessage, VisualsPage, create_settings_panel,
};

pub use config::RoutingCommand;

const WINDOW_MIN_SIZE: Size = Size::new(200.0, 150.0);
const SETTINGS_WINDOW_SIZE: Size = Size::new(480.0, 600.0);
const MAIN_WINDOW_INITIAL_SIZE: Size = Size::new(420.0, 520.0);
const TOAST_DISPLAY_DURATION: Duration = Duration::from_secs(2);

/// Wraps content in a container that expands to fill available space.
macro_rules! fill {
    ($e:expr) => {
        container($e).width(Length::Fill).height(Length::Fill)
    };
}

fn open_window(
    size: Size,
    with_decorations: bool,
    transparent: bool,
) -> (window::Id, Task<window::Id>) {
    window::open(window::Settings {
        size,
        min_size: Some(WINDOW_MIN_SIZE),
        resizable: true,
        decorations: with_decorations,
        transparent,
        ..Default::default()
    })
}

#[derive(Debug)]
struct PopoutWindow {
    visual_id: VisualId,
    kind: VisualKind,
    original_index: usize,
    cached: Option<(VisualMetadata, VisualContent)>,
}

impl PopoutWindow {
    fn sync_from_snapshot(&mut self, snapshot: &VisualSnapshot) {
        self.cached = snapshot
            .slots
            .iter()
            .find(|slot| slot.id == self.visual_id && slot.enabled)
            .map(|slot| (slot.metadata, slot.content.clone()));
    }

    fn view(&self) -> Element<'_, VisualsMessage> {
        let Some((meta, content)) = &self.cached else {
            return fill!(text("")).into();
        };
        let msg = VisualsMessage::SettingsRequested {
            visual_id: self.visual_id,
            kind: self.kind,
        };
        mouse_area(fill!(content.render(*meta)))
            .on_right_press(msg)
            .into()
    }
}

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

pub fn run(config: UiConfig) -> Result {
    daemon(move || UiApp::new(config.clone()), update, view)
        .settings(Settings {
            id: Some("openmeters-ui".into()),
            ..Default::default()
        })
        .subscription(UiApp::subscription)
        .title(UiApp::title)
        .theme(UiApp::theme)
        .run()
}

#[derive(Debug)]
struct UiApp {
    current_page: Page,
    config_page: ConfigPage,
    visuals_page: VisualsPage,
    visual_manager: VisualManagerHandle,
    settings_handle: SettingsHandle,
    audio_frames: Option<Arc<AsyncReceiver<Vec<f32>>>>,
    ui_visible: bool,
    rendering_paused: bool,
    overlay_until: Option<Instant>,
    main_window_id: window::Id,
    main_window_size: Size,
    settings_window: Option<(window::Id, ActiveSettings)>,
    popout_windows: FxHashMap<window::Id, PopoutWindow>,
    focused_window: Option<window::Id>,
    exit_warning_until: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Config,
    Visuals,
}

#[derive(Debug, Clone)]
enum Message {
    Page(Page),
    Config(ConfigMessage),
    Visuals(VisualsMessage),
    AudioFrame(Vec<f32>),
    ToggleChrome,
    TogglePause,
    PopOutOrDock,
    Quit,
    Resize,
    WindowOpened,
    WindowClosed(window::Id),
    WindowResized(window::Id, Size),
    WindowFocused(window::Id),
    WindowDragged(window::Id),
    Settings(window::Id, SettingsMessage),
}

fn handle_keyboard_shortcut(event: keyboard::Event) -> Option<Message> {
    let keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
        return None;
    };
    let (ctrl, shift, no_modifiers) =
        (modifiers.control(), modifiers.shift(), modifiers.is_empty());
    match &key {
        Key::Character(ch) if ctrl && shift && ch.eq_ignore_ascii_case("h") => {
            Some(Message::ToggleChrome)
        }
        Key::Named(keyboard::key::Named::Space) if ctrl => Some(Message::PopOutOrDock),
        Key::Character(ch) if no_modifiers && ch.eq_ignore_ascii_case("p") => {
            Some(Message::TogglePause)
        }
        Key::Character(ch) if no_modifiers && ch.eq_ignore_ascii_case("q") => Some(Message::Quit),
        _ => None,
    }
}

impl UiApp {
    fn new(config: UiConfig) -> (Self, Task<Message>) {
        let UiConfig {
            routing_sender,
            registry_updates,
            audio_frames,
        } = config;
        let settings_handle = SettingsHandle::load_or_default();
        let (visual_settings, use_decorations) = {
            let guard = settings_handle.borrow();
            (
                guard.settings().visuals.clone(),
                guard.settings().decorations,
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
        );
        let visuals_page = VisualsPage::new(visual_manager.clone(), settings_handle.clone());
        let (main_id, open_task) = open_window(MAIN_WINDOW_INITIAL_SIZE, use_decorations, true);
        (
            Self {
                current_page: Page::Config,
                config_page,
                visuals_page,
                visual_manager,
                settings_handle,
                audio_frames,
                ui_visible: true,
                rendering_paused: false,
                overlay_until: None,
                main_window_id: main_id,
                main_window_size: MAIN_WINDOW_INITIAL_SIZE,
                settings_window: None,
                popout_windows: FxHashMap::default(),
                focused_window: Some(main_id),
                exit_warning_until: None,
            },
            open_task.map(|_| Message::WindowOpened),
        )
    }

    fn subscription(&self) -> Subscription<Message> {
        let page_sub = match self.current_page {
            Page::Config => self.config_page.subscription().map(Message::Config),
            Page::Visuals => self.visuals_page.subscription().map(Message::Visuals),
        };
        let audio_sub = self
            .audio_frames
            .as_ref()
            .map(|rx| channel_subscription(Arc::clone(rx)).map(Message::AudioFrame));
        let focus_sub = event::listen_with(|evt, _, window_id| {
            matches!(evt, Event::Window(window::Event::Focused))
                .then_some(Message::WindowFocused(window_id))
        });
        Subscription::batch(
            [
                Some(page_sub),
                audio_sub,
                Some(keyboard::listen().filter_map(handle_keyboard_shortcut)),
                Some(window::close_events().map(Message::WindowClosed)),
                Some(window::resize_events().map(|(id, size)| Message::WindowResized(id, size))),
                Some(focus_sub),
            ]
            .into_iter()
            .flatten(),
        )
    }

    fn toggle_visibility(&mut self) {
        self.ui_visible = !self.ui_visible;
        if !self.ui_visible {
            self.current_page = Page::Visuals;
        }
        self.overlay_until = (!self.ui_visible).then(|| Instant::now() + TOAST_DISPLAY_DURATION);
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
        let (new_id, open_task) = open_window(SETTINGS_WINDOW_SIZE, true, false);
        self.settings_window = Some((new_id, new_panel));
        match previous {
            Some((old_id, _)) => Task::batch([
                window::close(old_id),
                open_task.map(|_| Message::WindowOpened),
            ]),
            None => open_task.map(|_| Message::WindowOpened),
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
        let (new_id, open_task) = open_window(window_size, true, true);
        let mut popout = PopoutWindow {
            visual_id,
            kind,
            original_index: index,
            cached: None,
        };
        popout.sync_from_snapshot(&snapshot);
        self.popout_windows.insert(new_id, popout);
        open_task.map(|_| Message::WindowOpened)
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
        let title_info = self
            .settings_window
            .as_ref()
            .filter(|(id, _)| *id == window_id)
            .map(|(_, panel)| (panel.visual_id(), " settings"))
            .or_else(|| {
                self.popout_windows
                    .get(&window_id)
                    .map(|popout| (popout.visual_id, ""))
            });
        title_info
            .and_then(|(visual_id, suffix)| {
                self.visual_manager
                    .snapshot()
                    .slots
                    .iter()
                    .find(|slot| slot.id == visual_id)
                    .map(|slot| format!("{}{} - OpenMeters", slot.metadata.display_name, suffix))
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
        let use_decorations = self.settings_handle.borrow().settings().decorations;
        let visuals_view = self
            .visuals_page
            .view(self.ui_visible)
            .map(Message::Visuals);
        let now = Instant::now();
        let is_active = |deadline: Option<Instant>| deadline.is_some_and(|expires| now < expires);
        let toasts: Vec<_> = [
            (!self.ui_visible && is_active(self.overlay_until))
                .then_some("ctrl+shift+h to restore"),
            self.rendering_paused.then_some("paused (p to resume)"),
            is_active(self.exit_warning_until).then_some("q again to exit"),
        ]
        .into_iter()
        .flatten()
        .collect();
        let toast_bar = || {
            container(
                row(toasts
                    .iter()
                    .map(|toast_msg| container(text(*toast_msg).size(11)).padding([2, 6]).into())
                    .collect::<Vec<_>>())
                .spacing(12),
            )
            .width(Length::Fill)
            .align_x(Horizontal::Center)
        };
        let content: Element<'_, Message> = if self.ui_visible {
            let mut tabs = row![
                create_tab_button("config", Page::Config, self.current_page),
                create_tab_button("visuals", Page::Visuals, self.current_page)
            ]
            .spacing(8)
            .width(Length::Fill);
            if !use_decorations {
                tabs = tabs.push(create_drag_handle(
                    "::",
                    Message::WindowDragged(self.main_window_id),
                    iced::mouse::Interaction::Grab,
                ))
            }
            let page_content = match self.current_page {
                Page::Config => fill!(self.config_page.view().map(Message::Config))
                    .style(theme::opaque_container)
                    .into(),
                Page::Visuals => visuals_view,
            };
            let inner = if toasts.is_empty() {
                column![tabs, fill!(page_content)].spacing(12)
            } else {
                column![column![tabs, fill!(page_content)].spacing(12), toast_bar()].spacing(0)
            };
            fill!(inner).padding(16).into()
        } else {
            let inner = if toasts.is_empty() {
                column![fill!(visuals_view)]
            } else {
                column![fill!(visuals_view), toast_bar()]
            };
            inner
                .spacing(0)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        };
        if use_decorations {
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

    fn handle_popout_or_dock(&mut self) -> Task<Message> {
        if let Some(focused) = self.focused_window
            && let Some(popout) = self.popout_windows.remove(&focused)
        {
            self.visual_manager
                .borrow_mut()
                .restore_position(popout.visual_id, popout.original_index);
            self.sync_visuals_page();
            self.settings_handle
                .update(|settings| settings.set_visual_order(&self.visual_manager.snapshot()));
            return window::close(focused);
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

    fn recreate_windows(&mut self, use_decorations: bool) -> Task<Message> {
        let old_main_id = self.main_window_id;
        let (new_main_id, open_main) = open_window(self.main_window_size, use_decorations, true);
        self.main_window_id = new_main_id;
        let snapshot = self.visual_manager.snapshot();
        let settings_task = self
            .settings_window
            .take()
            .map(|(old_settings_id, panel)| {
                let visual_id = panel.visual_id();
                snapshot
                    .slots
                    .iter()
                    .find(|slot| slot.id == visual_id)
                    .map(|slot| {
                        let (new_settings_id, open_settings) =
                            open_window(SETTINGS_WINDOW_SIZE, true, false);
                        self.settings_window = Some((
                            new_settings_id,
                            create_settings_panel(visual_id, slot.kind, &self.visual_manager),
                        ));
                        Task::batch([
                            open_settings.map(|_| Message::WindowOpened),
                            window::close(old_settings_id),
                        ])
                    })
                    .unwrap_or_else(|| window::close(old_settings_id))
            })
            .unwrap_or_else(Task::none);
        Task::batch([
            open_main.map(|_| Message::WindowOpened),
            window::close(old_main_id),
            settings_task,
        ])
    }
}

fn update(app: &mut UiApp, msg: Message) -> Task<Message> {
    match msg {
        Message::Page(page) => {
            app.current_page = page;
            Task::none()
        }
        Message::Config(config_msg) => {
            let decoration_task = if let ConfigMessage::DecorationsToggled(enabled) = &config_msg {
                app.recreate_windows(*enabled)
            } else {
                Task::none()
            };
            Task::batch([
                app.config_page.update(config_msg).map(Message::Config),
                decoration_task,
                app.sync_all_windows(),
            ])
        }
        Message::Visuals(VisualsMessage::SettingsRequested { visual_id, kind }) => {
            app.open_settings_window(visual_id, kind)
        }
        Message::Visuals(VisualsMessage::WindowDragRequested) => window::drag(app.main_window_id),
        Message::Visuals(visuals_msg) => app.visuals_page.update(visuals_msg).map(Message::Visuals),
        Message::ToggleChrome => {
            app.toggle_visibility();
            Task::none()
        }
        Message::TogglePause => {
            app.rendering_paused = !app.rendering_paused;
            Task::none()
        }
        Message::PopOutOrDock => app.handle_popout_or_dock(),
        Message::Quit => {
            if app
                .exit_warning_until
                .is_some_and(|deadline| Instant::now() < deadline)
            {
                return exit();
            }
            app.exit_warning_until = Some(Instant::now() + TOAST_DISPLAY_DURATION);
            Task::none()
        }
        Message::Resize => window::drag_resize(app.main_window_id, window::Direction::SouthEast),
        Message::AudioFrame(samples) if !app.rendering_paused => {
            app.visual_manager.borrow_mut().ingest_samples(&samples);
            app.sync_all_windows()
        }
        Message::AudioFrame(_) | Message::WindowOpened => Task::none(),
        Message::WindowClosed(window_id) => app.on_window_closed(window_id),
        Message::WindowResized(window_id, new_size) => {
            if window_id == app.main_window_id {
                app.main_window_size = new_size
            }
            Task::none()
        }
        Message::WindowFocused(window_id) => {
            app.focused_window = Some(window_id);
            Task::none()
        }
        Message::WindowDragged(window_id) => window::drag(window_id),
        Message::Settings(window_id, settings_msg) => {
            if let Some((settings_wid, panel)) = app.settings_window.as_mut()
                && *settings_wid == window_id
            {
                panel.handle_message(&settings_msg, &app.visual_manager, &app.settings_handle)
            }
            Task::none()
        }
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
        let scrollable_content = scrollable(panel.view())
            .width(Length::Fill)
            .height(Length::Fill);
        let inner: Element<'_, SettingsMessage> = fill!(scrollable_content)
            .padding(16)
            .style(theme::weak_container)
            .into();
        return inner.map(move |msg| Message::Settings(window_id, msg));
    }
    app.popout_windows
        .get(&window_id)
        .map(|popout| popout.view().map(Message::Visuals))
        .unwrap_or_else(|| fill!(text("")).into())
}

fn create_tab_button(
    label: &'static str,
    target: Page,
    current: Page,
) -> Element<'static, Message> {
    let is_active = current == target;
    let inner = container(text(label).wrapping(Wrapping::None))
        .width(Length::Fill)
        .clip(true);
    let tab = button(inner)
        .style(move |theme, status| theme::tab_button_style(theme, is_active, status))
        .width(Length::Fill)
        .padding(8);
    if is_active {
        tab
    } else {
        tab.on_press(Message::Page(target))
    }
    .into()
}

fn create_drag_handle(
    label: &str,
    on_press: Message,
    cursor: iced::mouse::Interaction,
) -> Element<'_, Message> {
    mouse_area(container(text(label).size(14).align_y(Vertical::Center)).padding(4))
        .on_press(on_press)
        .interaction(cursor)
        .into()
}
