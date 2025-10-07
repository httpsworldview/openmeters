use crate::ui::pane_grid::{self, Content as PaneContent};
use crate::ui::visualization::visual_manager::{
    VisualContent, VisualId, VisualLayoutHint, VisualManagerHandle, VisualSlotSnapshot,
    VisualSnapshot,
};
use crate::ui::visualization::{lufs_meter, oscilloscope, spectrogram};
use iced::alignment::{Horizontal, Vertical};
use iced::widget::{container, text};
use iced::{Element, Length, Subscription, Task};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum VisualsMessage {
    PaneDragged(pane_grid::DragEvent),
}

#[derive(Debug, Clone)]
struct VisualPane {
    id: VisualId,
    content: VisualContent,
    layout_hint: VisualLayoutHint,
}

impl VisualPane {
    fn from_snapshot(snapshot: &VisualSlotSnapshot) -> Self {
        Self {
            id: snapshot.id,
            content: snapshot.content.clone(),
            layout_hint: snapshot.layout_hint,
        }
    }

    fn view(&self) -> PaneContent<'_, VisualsMessage> {
        let content: Element<'_, VisualsMessage> = match &self.content {
            VisualContent::LufsMeter { state } => {
                let meter = lufs_meter::widget_with_layout(
                    state,
                    self.layout_hint.preferred_width,
                    self.layout_hint.preferred_height,
                );
                Element::from(
                    container(meter)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .align_x(Horizontal::Center)
                        .align_y(Vertical::Bottom),
                )
            }
            VisualContent::Oscilloscope { state } => {
                let scope = oscilloscope::widget(state);
                Element::from(
                    container(scope)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .center_x(Length::Fill),
                )
            }
            VisualContent::Spectrogram { state } => {
                let spec = spectrogram::widget(state);
                Element::from(
                    container(spec)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .center_x(Length::Fill),
                )
            }
            VisualContent::Placeholder { message } => {
                let placeholder = text(message.as_ref())
                    .size(14)
                    .width(Length::Fill)
                    .align_x(Horizontal::Center);
                Element::from(placeholder)
            }
        };

        let target_width = self.layout_hint.preferred_width;
        let target_height = self.layout_hint.preferred_height;

        let width = if self.layout_hint.fill_horizontal {
            Length::Fill
        } else {
            Length::Fixed(target_width)
        };

        let height = if self.layout_hint.fill_vertical {
            Length::Fill
        } else {
            Length::Fixed(target_height)
        };

        let framed = container(content)
            .width(width)
            .height(height)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center);

        let element = container(framed)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center);

        PaneContent::new(element).with_width_hint(
            self.layout_hint.min_width,
            self.layout_hint.preferred_width,
            self.layout_hint.max_width,
        )
    }
}

#[derive(Debug)]
pub struct VisualsPage {
    visual_manager: VisualManagerHandle,
    panes: Option<pane_grid::State<VisualPane>>,
    order: Vec<VisualId>,
}

impl VisualsPage {
    pub fn new(visual_manager: VisualManagerHandle) -> Self {
        let mut page = Self {
            visual_manager,
            panes: None,
            order: Vec::new(),
        };
        page.sync_with_manager();
        page
    }

    pub fn subscription(&self) -> Subscription<VisualsMessage> {
        Subscription::none()
    }

    pub fn update(&mut self, message: VisualsMessage) -> Task<VisualsMessage> {
        match message {
            VisualsMessage::PaneDragged(event) => {
                if let Some(panes) = self.panes.as_mut() {
                    if let pane_grid::DragEvent::Dropped {
                        pane,
                        target: pane_grid::Target::Pane(target),
                    } = event
                    {
                        panes.swap(pane, target);
                        let new_order: Vec<VisualId> =
                            panes.iter().map(|(_, pane)| pane.id).collect();
                        self.order = new_order.clone();
                        self.visual_manager.borrow_mut().reorder(&new_order);
                    }
                }
            }
        }

        Task::none()
    }

    pub fn view(&self) -> Element<'_, VisualsMessage> {
        if let Some(panes) = &self.panes {
            let grid = pane_grid::PaneGrid::new(panes, |_, pane_state| pane_state.view())
                .width(Length::Fill)
                .height(Length::Fill)
                .spacing(16.0)
                .on_drag(VisualsMessage::PaneDragged);

            container(grid)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            container(text("enable one or more visual modules to get started"))
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into()
        }
    }

    pub fn sync_with_manager(&mut self) {
        let snapshot = self.visual_manager.snapshot();
        self.apply_snapshot(snapshot);
    }

    pub fn apply_snapshot(&mut self, snapshot: VisualSnapshot) {
        let enabled_slots: Vec<_> = snapshot.slots.iter().filter(|slot| slot.enabled).collect();
        let new_order: Vec<_> = enabled_slots.iter().map(|slot| slot.id).collect();

        if enabled_slots.is_empty() {
            self.order.clear();
            self.panes = None;
            return;
        }

        if self.panes.is_none() || new_order != self.order {
            self.order = new_order.clone();
            self.panes = Self::build_panes(&enabled_slots);
            return;
        }

        if let Some(panes) = self.panes.as_mut() {
            let slot_map: HashMap<VisualId, &VisualSlotSnapshot> =
                enabled_slots.iter().map(|slot| (slot.id, *slot)).collect();

            panes.for_each_mut(|_, pane_state| {
                if let Some(slot) = slot_map.get(&pane_state.id) {
                    pane_state.content = slot.content.clone();
                    pane_state.layout_hint = slot.layout_hint;
                }
            });
        }
    }

    fn build_panes(slots: &[&VisualSlotSnapshot]) -> Option<pane_grid::State<VisualPane>> {
        let mut iter = slots.iter();
        let Some(first) = iter.next() else {
            return None;
        };

        let (mut state, mut last_pane) = pane_grid::State::new(VisualPane::from_snapshot(first));

        for slot in iter {
            if let Some(pane) = state.insert_after(last_pane, VisualPane::from_snapshot(slot)) {
                last_pane = pane;
            }
        }

        Some(state)
    }
}
