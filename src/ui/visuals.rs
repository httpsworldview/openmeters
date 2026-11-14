// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::persistence::settings::{SettingsHandle, WidthBasis};
use crate::ui::widgets::pane_grid::{self, Content as PaneContent, Pane};
use crate::visuals::registry::{
    VisualContent, VisualKind, VisualManagerHandle, VisualSlotSnapshot,
};
use iced::widget::{MouseArea, container, mouse_area, text};
use iced::{Element, Length};

#[derive(Debug, Clone)]
pub enum VisualsMessage {
    PaneMoved(Pane, usize),
    PaneResized(pane_grid::ResizeWidths),
    PaneHovered(Option<Pane>),
    SettingsRequested(VisualKind),
}

pub(in crate::ui) fn visual_area(content: &VisualContent) -> MouseArea<'_, VisualsMessage> {
    mouse_area(content.render()).on_right_press(VisualsMessage::SettingsRequested(content.kind()))
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

    pub fn update(&mut self, message: VisualsMessage) {
        match message {
            VisualsMessage::PaneResized(widths) => {
                let bases = widths.into_iter().filter_map(|(pane, basis)| {
                    let shown = self.panes.iter().any(|visual| visual.kind() == pane);
                    shown.then_some((pane, WidthBasis::new(basis)?))
                });
                let bases: Vec<_> = bases.collect();
                self.settings.edit(|s| s.visuals.width_basis.extend(bases));
            }
            VisualsMessage::PaneMoved(pane, to) => {
                let mut shown: Vec<_> = self.panes.iter().map(VisualSlotSnapshot::kind).collect();
                let Some(from) = shown.iter().position(|&kind| kind == pane) else {
                    return;
                };
                if from == to || to >= shown.len() {
                    return;
                }
                let mut order = self.visual_manager.borrow().order();
                let slots: Vec<_> = (0..order.len())
                    .filter(|&slot| shown.contains(&order[slot]))
                    .collect();
                let moved = shown.remove(from);
                shown.insert(to, moved);
                // Shown visuals trade slots; hidden ones keep theirs.
                for (slot, kind) in slots.into_iter().zip(shown) {
                    order[slot] = kind;
                }
                self.settings.edit(|s| s.visuals.order = order);
            }
            VisualsMessage::PaneHovered(pane) => self.hovered_pane = pane,
            VisualsMessage::SettingsRequested(_) => {}
        }
    }

    pub fn hovered_visual(&self) -> Option<VisualKind> {
        self.hovered_pane
            .filter(|&hovered| self.panes.iter().any(|visual| visual.kind() == hovered))
    }

    pub fn view(&self, reorder_enabled: bool) -> Element<'_, VisualsMessage> {
        if self.panes.is_empty() {
            return container(text("enable some visuals to see them here (Ctrl+Shift+H)"))
                .center(Length::Fill)
                .into();
        }

        Element::new(pane_grid::PaneGrid::new(
            &self.panes,
            |visual| {
                (
                    visual.kind(),
                    PaneContent::new(
                        visual_area(&visual.content),
                        visual.min_width,
                        visual.width_basis.get(),
                    ),
                )
            },
            reorder_enabled.then_some(VisualsMessage::PaneMoved),
            VisualsMessage::PaneResized,
            VisualsMessage::PaneHovered,
        ))
    }

    pub(in crate::ui) fn apply_snapshot_excluding(
        &mut self,
        snapshot: &[VisualSlotSnapshot],
        exclude: impl Fn(VisualKind) -> bool,
    ) {
        let shown = snapshot
            .iter()
            .filter(|slot| slot.enabled && !exclude(slot.kind()));
        self.panes = shown.cloned().collect();
    }
}
