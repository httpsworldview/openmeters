// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::persistence::settings::SettingsHandle;
use crate::ui::widgets::pane_grid::{self, Content as PaneContent, Pane};
use crate::visuals::registry::{VisualKind, VisualManagerHandle, VisualSlotSnapshot};
use iced::widget::{container, mouse_area, text};
use iced::{Element, Length, Task};

#[derive(Debug, Clone)]
pub enum VisualsMessage {
    PaneDragged(pane_grid::DragEvent),
    PaneResized(pane_grid::ResizeWidths),
    PaneContextRequested(Pane),
    PaneHovered(Option<Pane>),
    SettingsRequested(VisualKind),
}

pub struct VisualsPage {
    visual_manager: VisualManagerHandle,
    settings: SettingsHandle,
    panes: Vec<VisualSlotSnapshot>,
    hovered_pane: Option<Pane>,
}

impl VisualsPage {
    pub fn new(visual_manager: VisualManagerHandle, settings: SettingsHandle) -> Self {
        let mut page = Self {
            visual_manager,
            settings,
            panes: Vec::new(),
            hovered_pane: None,
        };
        let snapshot = page.visual_manager.borrow().snapshot();
        page.apply_snapshot_excluding(&snapshot, |_| false);
        page
    }

    pub fn update(&mut self, message: VisualsMessage) -> Task<VisualsMessage> {
        match message {
            VisualsMessage::PaneResized(widths) => {
                let bases: Vec<_> = widths
                    .into_iter()
                    .filter_map(|(pane, basis)| {
                        let basis = crate::util::finite_positive(basis)?;
                        let visual = self.panes.iter_mut().find(|visual| visual.kind == pane)?;
                        visual.width_basis = basis;
                        Some((visual.kind, basis))
                    })
                    .collect();
                if !bases.is_empty() {
                    let mut manager = self.visual_manager.borrow_mut();
                    for &(kind, basis) in &bases {
                        manager.set_width_basis(kind, basis);
                    }
                    self.settings
                        .update(|s| s.data.visuals.width_basis.extend(bases));
                }
            }
            VisualsMessage::PaneDragged(pane_grid::DragEvent::Moved { pane, target }) => {
                if let [Some(from), Some(to)] = [pane, target]
                    .map(|kind| self.panes.iter().position(|visual| visual.kind == kind))
                    && from != to
                {
                    let visual = self.panes.remove(from);
                    self.panes.insert(to, visual);
                    let order: Vec<_> = self.panes.iter().map(|visual| visual.kind).collect();
                    self.visual_manager.borrow_mut().reorder(&order);
                }
            }
            VisualsMessage::PaneDragged(pane_grid::DragEvent::Dropped) => {
                self.settings.update(|s| {
                    s.data.visuals.order = self.visual_manager.borrow().order();
                });
            }
            VisualsMessage::PaneContextRequested(kind) => {
                if self.panes.iter().any(|visual| visual.kind == kind) {
                    return Task::done(VisualsMessage::SettingsRequested(kind));
                }
            }
            VisualsMessage::PaneHovered(pane) => self.hovered_pane = pane,
            VisualsMessage::SettingsRequested(_) => {}
        }
        Task::none()
    }

    pub fn hovered_visual(&self) -> Option<VisualKind> {
        let hovered = self.hovered_pane?;
        self.panes
            .iter()
            .any(|visual| visual.kind == hovered)
            .then_some(hovered)
    }

    pub fn view(&self, reorder_enabled: bool) -> Element<'_, VisualsMessage> {
        if self.panes.is_empty() {
            return container(text("enable some visuals to see them here (Ctrl+Shift+H)"))
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into();
        }

        pane_grid::PaneGrid::new(
            &self.panes,
            |visual| {
                (
                    visual.kind,
                    PaneContent::new(
                        mouse_area(visual.content.render())
                            .on_right_press(VisualsMessage::PaneContextRequested(visual.kind)),
                        visual.min_width,
                        visual.width_basis,
                    ),
                )
            },
            reorder_enabled.then_some(VisualsMessage::PaneDragged),
            VisualsMessage::PaneResized,
            VisualsMessage::PaneHovered,
        )
        .into()
    }

    pub(in crate::ui) fn apply_snapshot_excluding(
        &mut self,
        snapshot: &[VisualSlotSnapshot],
        exclude: impl Fn(VisualKind) -> bool,
    ) {
        let slots = || {
            snapshot
                .iter()
                .filter(|slot| slot.enabled && !exclude(slot.kind))
        };
        if self
            .panes
            .iter()
            .map(|pane| pane.kind)
            .eq(slots().map(|slot| slot.kind))
        {
            return;
        }
        self.panes = slots().cloned().collect();
        self.hovered_pane = None;
    }
}
