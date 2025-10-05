pub mod config;
pub mod visuals;

use crate::audio::pw_registry::RegistrySnapshot;
use crate::ui::theme;
use async_channel::Receiver as AsyncReceiver;
use config::{ConfigMessage, ConfigPage};
use visuals::{VisualsMessage, VisualsPage};

use iced::widget::{button, column, container, row, text};
use iced::{Element, Length, Result, Settings, Size, Subscription, Task, application};
use std::sync::{Arc, mpsc};

pub use config::RoutingCommand;

const APP_PADDING: f32 = 16.0;

pub struct UiConfig {
    routing_sender: mpsc::Sender<RoutingCommand>,
    registry_updates: Option<Arc<AsyncReceiver<RegistrySnapshot>>>,
}

impl UiConfig {
    pub fn new(
        routing_sender: mpsc::Sender<RoutingCommand>,
        registry_updates: Option<Arc<AsyncReceiver<RegistrySnapshot>>>,
    ) -> Self {
        Self {
            routing_sender,
            registry_updates,
        }
    }
}

pub fn run(config: UiConfig) -> Result {
    let settings = Settings {
        id: Some(String::from("openmeters-ui")),
        ..Settings::default()
    };

    application("OpenMeters", update, view)
        .settings(settings)
        .window_size(Size::new(420.0, 520.0))
        .resizable(true)
        .theme(|_| theme::theme())
        .subscription(|state: &UiApp| state.subscription())
        .run_with(move || UiApp::new(config))
}

#[derive(Debug)]
struct UiApp {
    current_page: Page,
    config_page: ConfigPage,
    visuals_page: VisualsPage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Config,
    Visuals,
}

#[derive(Debug, Clone)]
enum Message {
    PageSelected(Page),
    Config(ConfigMessage),
    Visuals(VisualsMessage),
}

impl UiApp {
    fn new(config: UiConfig) -> (Self, Task<Message>) {
        let UiConfig {
            routing_sender,
            registry_updates,
        } = config;

        let config_page = ConfigPage::new(routing_sender.clone(), registry_updates.clone());
        let visuals_page = VisualsPage::new();

        (
            Self {
                current_page: Page::Config,
                config_page,
                visuals_page,
            },
            Task::none(),
        )
    }

    fn subscription(&self) -> Subscription<Message> {
        match self.current_page {
            Page::Config => self.config_page.subscription().map(Message::Config),
            Page::Visuals => self.visuals_page.subscription().map(Message::Visuals),
        }
    }
}

fn update(app: &mut UiApp, message: Message) -> Task<Message> {
    match message {
        Message::PageSelected(page) => {
            app.current_page = page;
            Task::none()
        }
        Message::Config(msg) => app.config_page.update(msg).map(Message::Config),
        Message::Visuals(msg) => app.visuals_page.update(msg).map(Message::Visuals),
    }
}

fn view(app: &UiApp) -> Element<'_, Message> {
    let config_button = {
        let mut btn = button(text("config")).style(move |_theme, status| {
            let mut style = iced::widget::button::Style::default();
            style.background = Some(iced::Background::Color(
                if app.current_page == Page::Config {
                    theme::elevated_color()
                } else {
                    theme::surface_color()
                },
            ));
            style.text_color = theme::text_color();
            style.border = theme::sharp_border();

            match status {
                iced::widget::button::Status::Hovered => {
                    style.background = Some(iced::Background::Color(theme::hover_color()));
                }
                iced::widget::button::Status::Pressed => {
                    style.border = theme::focus_border();
                }
                _ => {}
            }

            style
        });
        if app.current_page != Page::Config {
            btn = btn.on_press(Message::PageSelected(Page::Config));
        }
        btn.width(Length::Fill).padding(8)
    };

    let visuals_button = {
        let mut btn = button(text("visuals")).style(move |_theme, status| {
            let mut style = iced::widget::button::Style::default();
            style.background = Some(iced::Background::Color(
                if app.current_page == Page::Visuals {
                    theme::elevated_color()
                } else {
                    theme::surface_color()
                },
            ));
            style.text_color = theme::text_color();
            style.border = theme::sharp_border();

            match status {
                iced::widget::button::Status::Hovered => {
                    style.background = Some(iced::Background::Color(theme::hover_color()));
                }
                iced::widget::button::Status::Pressed => {
                    style.border = theme::focus_border();
                }
                _ => {}
            }

            style
        });
        if app.current_page != Page::Visuals {
            btn = btn.on_press(Message::PageSelected(Page::Visuals));
        }
        btn.width(Length::Fill).padding(8)
    };

    let tabs = row![config_button, visuals_button]
        .spacing(8)
        .width(Length::Fill);

    let page_content = match app.current_page {
        Page::Config => app.config_page.view().map(Message::Config),
        Page::Visuals => app.visuals_page.view().map(Message::Visuals),
    };

    let layout = column![
        tabs,
        container(page_content)
            .width(Length::Fill)
            .height(Length::Fill)
    ]
    .spacing(12);

    container(layout)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(APP_PADDING)
        .into()
}
