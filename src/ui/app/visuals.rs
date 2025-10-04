use iced::widget::{container, text};
use iced::{Element, Length, Subscription, Task};

#[derive(Debug, Clone)]
pub enum VisualsMessage {}

#[derive(Debug, Default)]
pub struct VisualsPage;

impl VisualsPage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscription(&self) -> Subscription<VisualsMessage> {
        Subscription::none()
    }

    pub fn update(&mut self, _message: VisualsMessage) -> Task<VisualsMessage> {
        Task::none()
    }

    pub fn view(&self) -> Element<'_, VisualsMessage> {
        container(text("Visual dashboards coming soon"))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}
