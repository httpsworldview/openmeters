// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::persistence::settings::SettingsHandle;
use crate::ui::widgets::pane_grid::{self, Content as PaneContent, Pane};
use crate::visuals::registry::{
    VisualContent, VisualId, VisualKind, VisualManagerHandle, VisualMetadata, VisualSlotSnapshot,
    VisualSnapshot,
};
pub mod settings;
pub use settings::{ActiveSettings, SettingsMessage, create_panel as create_settings_panel};

use iced::widget::{container, mouse_area, text};
use iced::{Element, Length, Task};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum VisualsMessage {
    PaneDragged(pane_grid::DragEvent),
    PaneResized(pane_grid::ResizeWidths),
    PaneContextRequested(Pane),
    PaneHovered(Option<Pane>),
    SettingsRequested {
        visual_id: VisualId,
        kind: VisualKind,
    },
    WindowDragRequested,
}

#[derive(Debug, Clone)]
struct VisualPane {
    id: VisualId,
    kind: VisualKind,
    metadata: VisualMetadata,
    content: VisualContent,
    width_basis: f32,
}

impl VisualPane {
    fn from_snapshot(s: &VisualSlotSnapshot, width_basis: f32) -> Self {
        Self {
            id: s.id,
            kind: s.kind,
            metadata: s.metadata,
            content: s.content.clone(),
            width_basis,
        }
    }
    fn view(&self) -> PaneContent<'_, VisualsMessage> {
        PaneContent::new(self.content.render(self.metadata))
            .with_width_basis(self.metadata.min_width, self.width_basis)
    }
}

#[derive(Debug)]
pub struct VisualsPage {
    visual_manager: VisualManagerHandle,
    settings: SettingsHandle,
    panes: Option<pane_grid::State<VisualPane>>,
    order: Vec<VisualId>,
    hovered_pane: Option<Pane>,
}

impl VisualsPage {
    pub fn new(visual_manager: VisualManagerHandle, settings: SettingsHandle) -> Self {
        let mut page = Self {
            visual_manager,
            settings,
            panes: None,
            order: Vec::new(),
            hovered_pane: None,
        };
        page.sync_with_manager();
        page
    }

    pub fn update(&mut self, message: VisualsMessage) -> Task<VisualsMessage> {
        match message {
            VisualsMessage::PaneResized(widths) => {
                let bases = self.apply_resize_width_basis(&widths);
                if !bases.is_empty() {
                    self.settings.update(|s| s.set_visual_width_basis(bases));
                }
            }
            VisualsMessage::PaneDragged(pane_grid::DragEvent::Moved { pane, target }) => {
                if let Some(panes) = self.panes.as_mut()
                    && panes.move_to(pane, target)
                {
                    if let (Some(a), Some(b)) = (panes.get(pane), panes.get(target)) {
                        self.visual_manager.borrow_mut().swap_entries(a.id, b.id);
                    }
                    self.order = panes.iter().map(|(_, p)| p.id).collect();
                }
            }
            VisualsMessage::PaneDragged(pane_grid::DragEvent::Dropped { .. }) => {
                self.settings.update(|s| {
                    s.set_visual_order(self.visual_manager.snapshot().slots.iter().map(|s| s.kind));
                });
            }
            VisualsMessage::PaneContextRequested(pane) => {
                if let Some(p) = self.panes.as_ref().and_then(|ps| ps.get(pane)) {
                    return Task::done(VisualsMessage::SettingsRequested {
                        visual_id: p.id,
                        kind: p.kind,
                    });
                }
            }
            VisualsMessage::PaneHovered(pane) => self.hovered_pane = pane,
            VisualsMessage::PaneDragged(_)
            | VisualsMessage::SettingsRequested { .. }
            | VisualsMessage::WindowDragRequested => {}
        }
        Task::none()
    }

    pub fn hovered_visual(&self) -> Option<(VisualId, VisualKind)> {
        self.panes
            .as_ref()?
            .get(self.hovered_pane?)
            .map(|p| (p.id, p.kind))
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

    pub fn sync_with_manager(&mut self) {
        self.apply_snapshot_excluding(self.visual_manager.snapshot(), &[]);
    }

    pub(crate) fn apply_snapshot_excluding(
        &mut self,
        snapshot: VisualSnapshot,
        exclude: &[VisualId],
    ) {
        let slots: Vec<_> = snapshot
            .slots
            .iter()
            .filter(|s| s.enabled && !exclude.contains(&s.id))
            .collect();
        let new_order: Vec<_> = slots.iter().map(|s| s.id).collect();

        if slots.is_empty() {
            self.order.clear();
            self.panes = None;
            return;
        }
        if self.panes.is_none() || new_order != self.order {
            self.order = new_order;
            self.panes = self.build_panes(&slots);
            return;
        }
        if let Some(panes) = self.panes.as_mut() {
            let map: HashMap<_, _> = slots.iter().map(|s| (s.id, *s)).collect();
            panes.for_each_mut(|_, p| {
                if let Some(s) = map.get(&p.id) {
                    p.content = s.content.clone();
                    p.metadata = s.metadata;
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

    fn build_panes(&self, slots: &[&VisualSlotSnapshot]) -> Option<pane_grid::State<VisualPane>> {
        let settings = self.settings.borrow();
        let saved_width_basis = &settings.settings().visuals.width_basis;
        let visual_pane = |slot: &VisualSlotSnapshot| {
            VisualPane::from_snapshot(
                slot,
                Self::width_basis_from_settings(slot, saved_width_basis),
            )
        };
        let (first, rest) = slots.split_first()?;
        let (mut state, mut last) = pane_grid::State::new(visual_pane(first));
        for slot in rest {
            if let Some(p) = state.insert_after(last, visual_pane(slot)) {
                last = p;
            }
        }
        Some(state)
    }

    fn width_basis_from_settings(
        slot: &VisualSlotSnapshot,
        saved_width_basis: &HashMap<VisualKind, f32>,
    ) -> f32 {
        saved_width_basis
            .get(&slot.kind)
            .copied()
            .filter(|basis| basis.is_finite() && *basis > 0.0)
            .unwrap_or(slot.metadata.preferred_width)
    }
}
