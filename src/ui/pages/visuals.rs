// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::persistence::settings::SettingsHandle;
use crate::ui::widgets::pane_grid::{self, Content as PaneContent, Pane};
use crate::visuals::registry::{
    VisualContent, VisualKind, VisualManagerHandle, VisualSlotSnapshot,
};
pub mod settings;
pub use settings::{ActiveSettings, SettingsMessage, create_panel as create_settings_panel};

use iced::widget::{container, mouse_area, text};
use iced::{Element, Length, Task};

#[derive(Debug, Clone)]
pub enum VisualsMessage {
    PaneDragged(pane_grid::DragEvent),
    PaneResized(pane_grid::ResizeWidths),
    PaneContextRequested(Pane),
    PaneHovered(Option<Pane>),
    SettingsRequested(VisualKind),
    WindowDragRequested,
}

#[derive(Clone)]
struct VisualPane {
    kind: VisualKind,
    content: VisualContent,
    min_width: f32,
    width_basis: f32,
}

impl VisualPane {
    fn view(&self) -> PaneContent<'_, VisualsMessage> {
        PaneContent::new(self.content.render()).with_width_basis(self.min_width, self.width_basis)
    }
}

pub struct VisualsPage {
    visual_manager: VisualManagerHandle,
    settings: SettingsHandle,
    panes: Option<pane_grid::State<VisualPane>>,
    hovered_pane: Option<Pane>,
}

impl VisualsPage {
    pub fn new(visual_manager: VisualManagerHandle, settings: SettingsHandle) -> Self {
        let mut page = Self {
            visual_manager,
            settings,
            panes: None,
            hovered_pane: None,
        };
        let snapshot = page.visual_manager.borrow().snapshot();
        page.apply_snapshot_excluding(&snapshot, &[]);
        page
    }

    pub fn update(&mut self, message: VisualsMessage) -> Task<VisualsMessage> {
        match message {
            VisualsMessage::PaneResized(widths) => {
                let bases = self.apply_resize_width_basis(&widths);
                if !bases.is_empty() {
                    self.settings
                        .update(|s| s.data.visuals.width_basis.extend(bases));
                }
            }
            VisualsMessage::PaneDragged(pane_grid::DragEvent::Moved { pane, target }) => {
                if let Some(panes) = self.panes.as_mut()
                    && panes.move_to(pane, target)
                {
                    let order: Vec<_> = panes.iter().map(|(_, p)| p.kind).collect();
                    self.visual_manager.borrow_mut().reorder(&order);
                }
            }
            VisualsMessage::PaneDragged(pane_grid::DragEvent::Dropped) => {
                self.settings.update(|s| {
                    s.data.visuals.order = self.visual_manager.borrow().order();
                });
            }
            VisualsMessage::PaneContextRequested(pane) => {
                if let Some(p) = self.panes.as_ref().and_then(|ps| ps.get(pane)) {
                    return Task::done(VisualsMessage::SettingsRequested(p.kind));
                }
            }
            VisualsMessage::PaneHovered(pane) => self.hovered_pane = pane,
            VisualsMessage::SettingsRequested(_) | VisualsMessage::WindowDragRequested => {}
        }
        Task::none()
    }

    pub fn hovered_visual(&self) -> Option<VisualKind> {
        self.panes.as_ref()?.get(self.hovered_pane?).map(|p| p.kind)
    }

    pub fn view(&self, controls_visible: bool) -> Element<'_, VisualsMessage> {
        let Some(panes) = &self.panes else {
            return container(text("enable some visuals to see them here (Ctrl+Shift+H)"))
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into();
        };

        let mut grid = pane_grid::PaneGrid::new(panes, |_, p| p.view())
            .width(Length::Fill)
            .height(Length::Fill)
            .on_resize(VisualsMessage::PaneResized)
            .on_context_request(VisualsMessage::PaneContextRequested)
            .on_hover(VisualsMessage::PaneHovered);

        if controls_visible {
            grid = grid.on_drag(VisualsMessage::PaneDragged);
            container(grid)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            mouse_area(container(grid).width(Length::Fill).height(Length::Fill))
                .on_press(VisualsMessage::WindowDragRequested)
                .interaction(iced::mouse::Interaction::Grab)
                .into()
        }
    }

    pub(crate) fn apply_snapshot_excluding(
        &mut self,
        snapshot: &[VisualSlotSnapshot],
        exclude: &[VisualKind],
    ) {
        let slots: Vec<_> = snapshot
            .iter()
            .filter(|s| s.enabled && !exclude.contains(&s.kind))
            .collect();
        if slots.is_empty() {
            self.panes = None;
            self.hovered_pane = None;
            return;
        }
        if self.panes.as_ref().is_none_or(|panes| {
            panes
                .iter()
                .map(|(_, p)| p.kind)
                .ne(slots.iter().map(|s| s.kind))
        }) {
            self.panes = Some(self.build_panes(&slots));
            self.hovered_pane = None;
            return;
        }
        if let Some(panes) = self.panes.as_mut() {
            panes.for_each_mut(|_, p| {
                if let Some(s) = slots.iter().copied().find(|s| s.kind == p.kind) {
                    p.content = s.content.clone();
                }
            });
        }
    }

    fn apply_resize_width_basis(&mut self, widths: &[(Pane, f32)]) -> Vec<(VisualKind, f32)> {
        let Some(panes) = self.panes.as_mut() else {
            return Vec::new();
        };
        widths
            .iter()
            .filter_map(|&(pane, basis)| {
                if !basis.is_finite() || basis <= 0.0 {
                    return None;
                }
                let visual = panes.get_mut(pane)?;
                visual.width_basis = basis;
                Some((visual.kind, basis))
            })
            .collect()
    }

    fn build_panes(&self, slots: &[&VisualSlotSnapshot]) -> pane_grid::State<VisualPane> {
        let settings = self.settings.borrow();
        let saved_width_basis = &settings.data.visuals.width_basis;
        pane_grid::State::from_iter(slots.iter().map(|&slot| {
            VisualPane {
                kind: slot.kind,
                content: slot.content.clone(),
                min_width: slot.metadata.min_width,
                width_basis: saved_width_basis
                    .get(&slot.kind)
                    .copied()
                    .filter(|basis| basis.is_finite() && *basis > 0.0)
                    .unwrap_or(slot.metadata.preferred_width),
            }
        }))
    }
}
