// SPDX-License-Identifier: GPL-3.0-or-later AND MIT
// Copyright (C) 2026 Maika Namuo
// Copyright 2019 Hector Ramon, Iced contributors
//
// Adapted from iced_widget v0.13.4 pane_grid.
// See docs/licenses/iced_widget_pane_grid.md for the MIT notice.

use iced::advanced::renderer::Quad;
use iced::advanced::widget::{self, tree::Tree};
use iced::advanced::{Layout, Renderer as _, Shell, Widget, layout, mouse};
use iced::{Background, Element, Event, Length, Point, Rectangle, Size};

use crate::domain::visuals::VisualKind;
use crate::util::color::with_alpha;

pub type Pane = VisualKind;

const DIVIDER_HIT_WIDTH: f32 = 8.0;
const EPS: f32 = 0.001;

struct ResizeState {
    divider: usize,
    origin_x: f32,
    start: Vec<f32>,
    min: Vec<f32>,
    current: Vec<f32>,
}

#[derive(Default)]
struct Interaction {
    dragging: Option<(Pane, Point, f32)>,
    resizing: Option<ResizeState>,
    cursor_over: Option<Pane>,
}

pub type ResizeWidths = Vec<(Pane, f32)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragEvent {
    Moved { pane: Pane, target: Pane },
    Dropped,
}

// Element internals do not implement Debug; this mirrors iced's widget types.
#[allow(missing_debug_implementations)]
pub struct Content<'a, Message> {
    body: Element<'a, Message>,
    min_width: f32,
    basis_width: f32,
}

impl<'a, Message> Content<'a, Message> {
    pub fn new(body: impl Into<Element<'a, Message>>, min_width: f32, basis_width: f32) -> Self {
        Self {
            body: body.into(),
            min_width,
            basis_width,
        }
    }
}

// Callback closures do not implement Debug; this mirrors iced's widget types.
#[allow(missing_debug_implementations)]
pub struct PaneGrid<'a, Message> {
    entries: Vec<(Pane, Content<'a, Message>)>,
    on_drag: Option<fn(DragEvent) -> Message>,
    on_resize: fn(ResizeWidths) -> Message,
    on_hover: fn(Option<Pane>) -> Message,
}

impl<'a, Message: 'a> PaneGrid<'a, Message> {
    pub fn new<T>(
        state: &'a [T],
        view: impl Fn(&'a T) -> (Pane, Content<'a, Message>),
        on_drag: Option<fn(DragEvent) -> Message>,
        on_resize: fn(ResizeWidths) -> Message,
        on_hover: fn(Option<Pane>) -> Message,
    ) -> Self {
        Self {
            entries: state.iter().map(view).collect(),
            on_drag,
            on_resize,
            on_hover,
        }
    }

    fn pane_at(&self, layout: Layout<'_>, cursor: Point) -> Option<Pane> {
        self.entries
            .iter()
            .zip(layout.children())
            .find_map(|((pane, _), child)| child.bounds().contains(cursor).then_some(*pane))
    }

    fn divider_at(&self, layout: Layout<'_>, cursor: Point) -> Option<usize> {
        if !layout.bounds().contains(cursor) {
            return None;
        }
        let half = DIVIDER_HIT_WIDTH / 2.0;
        layout
            .children()
            .take(self.entries.len() - 1)
            .enumerate()
            .find_map(|(i, child)| {
                let x = child.bounds().x + child.bounds().width;
                ((cursor.x - x).abs() <= half).then_some(i)
            })
    }

    fn width_specs(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        self.entries
            .iter()
            .map(|(_, content)| (content.min_width, content.basis_width))
    }
}

impl<Message: 'static> Widget<Message, iced::Theme, iced::Renderer> for PaneGrid<'_, Message> {
    crate::macros::widget_method!(state Interaction);

    fn children(&self) -> Vec<Tree> {
        self.entries
            .iter()
            .map(|(_, content)| Tree::new(&content.body))
            .collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children_custom(
            &self.entries,
            |state, entry| state.diff(&entry.1.body),
            |entry| Tree::new(&entry.1.body),
        );
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let count = self.entries.len();
        let size = limits.resolve(Length::Fill, Length::Fill, Size::ZERO);
        let available_width = size.width.max(0.0);
        let resizing = tree.state.downcast_ref::<Interaction>().resizing.as_ref();
        let widths = resizing
            .filter(|r| {
                r.current.len() == count
                    && (r.current.iter().sum::<f32>() - available_width).abs() < 0.5
            })
            .map_or_else(
                || solve_widths(self.width_specs(), available_width),
                |r| fit_sum(r.current.clone(), available_width),
            );

        let mut x = 0.0;
        let children = self
            .entries
            .iter_mut()
            .zip(tree.children.iter_mut())
            .zip(widths)
            .map(|(((_, content), child), width)| {
                let width = width.max(0.0);
                let limits = layout::Limits::new(
                    Size::new(width, size.height),
                    Size::new(width, size.height),
                );
                let node = content
                    .body
                    .as_widget_mut()
                    .layout(child, renderer, &limits)
                    .move_to(Point::new(x, 0.0));
                x += width;
                node
            })
            .collect();
        layout::Node::with_children(size, children)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        for (((_, content), child), child_layout) in self
            .entries
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
        {
            content
                .body
                .as_widget_mut()
                .operate(child, child_layout, renderer, operation);
        }
    }

    crate::macros::widget_method!(update Message; this; tree, event, layout, cursor, renderer, clipboard, shell, viewport => {
        let interaction = tree.state.downcast_mut::<Interaction>();
        if let Event::Mouse(event) = event
            && (this.update_resize(interaction, event, shell)
                || this.update_interaction(interaction, event, layout, cursor, shell))
        {
            return;
        }

        for (((_, content), child), child_layout) in this
            .entries
            .iter_mut()
            .zip(tree.children.iter_mut())
            .zip(layout.children())
        {
            content.body.as_widget_mut().update(
                child,
                event,
                child_layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
        }
        if shell.is_event_captured() {
            return;
        }
        if let Event::Mouse(mouse::Event::CursorMoved { position }) = event {
            let pane = this.pane_at(layout, *position);
            this.hover(tree.state.downcast_mut::<Interaction>(), pane, shell);
        }
    });

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let interaction = tree.state.downcast_ref::<Interaction>();
        if interaction.dragging.is_some() {
            return mouse::Interaction::Grabbing;
        }
        if interaction.resizing.is_some()
            || cursor
                .position()
                .is_some_and(|p| self.divider_at(layout, p).is_some())
        {
            return mouse::Interaction::ResizingHorizontally;
        }
        if self.on_drag.is_some()
            && cursor
                .position()
                .is_some_and(|p| self.pane_at(layout, p).is_some())
        {
            return mouse::Interaction::Grab;
        }
        self.entries
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|(((_, content), child), child_layout)| {
                content.body.as_widget().mouse_interaction(
                    child,
                    child_layout,
                    cursor,
                    viewport,
                    renderer,
                )
            })
            .max()
            .unwrap_or_default()
    }

    crate::macros::widget_method!(draw this; tree, renderer, theme, defaults, layout, cursor, viewport => {
        let interaction = tree.state.downcast_ref::<Interaction>();
        let accent = || theme.extended_palette().primary.base.color;
        for (((pane, content), child), child_layout) in this
            .entries
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
        {
            renderer.with_layer(child_layout.bounds(), |renderer| {
                content.body.as_widget().draw(
                    child,
                    renderer,
                    theme,
                    defaults,
                    child_layout,
                    cursor,
                    viewport,
                );
            });
            if interaction.dragging.is_some_and(|(p, _, _)| p == *pane) {
                renderer.fill_quad(
                    Quad {
                        bounds: child_layout.bounds(),
                        border: iced::Border {
                            width: 2.0,
                            color: with_alpha(accent(), 0.9),
                            ..Default::default()
                        },
                        snap: true,
                        ..Default::default()
                    },
                    Background::Color(with_alpha(accent(), 0.4)),
                );
            }
        }
        if let Some(r) = &interaction.resizing
            && let Some(child) = layout.children().nth(r.divider)
        {
            let b = layout.bounds();
            renderer.fill_quad(
                Quad {
                    bounds: Rectangle::new(
                        Point::new(child.bounds().x + child.bounds().width - 1.0, b.y),
                        Size::new(2.0, b.height),
                    ),
                    snap: true,
                    ..Default::default()
                },
                Background::Color(with_alpha(accent(), 0.75)),
            );
        }
    });
}

impl<'a, Message: 'a> PaneGrid<'a, Message> {
    fn hover(&self, state: &mut Interaction, pane: Option<Pane>, shell: &mut Shell<'_, Message>) {
        if state.cursor_over != pane {
            state.cursor_over = pane;
            shell.publish((self.on_hover)(pane));
        }
    }

    fn update_interaction(
        &self,
        interaction: &mut Interaction,
        event: &mouse::Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        shell: &mut Shell<'_, Message>,
    ) -> bool {
        use mouse::Button;

        if matches!(event, mouse::Event::CursorLeft) {
            let dragging = interaction.dragging.take();
            self.hover(interaction, None, shell);
            if dragging.is_some() {
                self.publish_drop(shell);
                shell.capture_event();
            }
            return dragging.is_some();
        }

        if let Some((pane, origin, last_x)) = interaction.dragging {
            match event {
                mouse::Event::CursorMoved { position } => {
                    if position.distance(origin) > 5.0
                        && let Some(idx) = self.entries.iter().position(|(p, _)| *p == pane)
                    {
                        let neighbor = if position.x > last_x {
                            (idx + 1 < self.entries.len()).then_some(idx + 1)
                        } else if position.x < last_x {
                            idx.checked_sub(1)
                        } else {
                            None
                        };
                        if let Some(n) =
                            neighbor.and_then(|n| layout.children().nth(n).map(|l| (n, l)))
                        {
                            let b = n.1.bounds();
                            let crossed = (n.0 > idx && position.x > b.x + b.width / 2.0)
                                || (n.0 < idx && position.x < b.x + b.width / 2.0);
                            if crossed && let Some(on_drag) = &self.on_drag {
                                shell.publish(on_drag(DragEvent::Moved {
                                    pane,
                                    target: self.entries[n.0].0,
                                }));
                            }
                        }
                    }
                    interaction.dragging = Some((pane, origin, position.x));
                }
                mouse::Event::ButtonReleased(Button::Left) => {
                    interaction.dragging = None;
                    self.publish_drop(shell);
                }
                _ => {}
            }
            shell.capture_event();
            return true;
        }

        if let mouse::Event::ButtonPressed(Button::Left) = event {
            let Some(position) = cursor.position() else {
                return false;
            };
            if let Some(divider) = self.divider_at(layout, position) {
                let start: Vec<_> = layout
                    .children()
                    .map(|c| c.bounds().width.max(0.0))
                    .collect();
                interaction.resizing = Some(ResizeState {
                    divider,
                    origin_x: position.x,
                    min: fit_mins(self.width_specs(), start.iter().sum()),
                    current: start.clone(),
                    start,
                });
                shell.capture_event();
                shell.request_redraw();
                return true;
            }
            if self.on_drag.is_some()
                && let Some(pane) = self.pane_at(layout, position)
            {
                interaction.dragging = Some((pane, position, position.x));
                shell.capture_event();
                return true;
            }
        }
        false
    }

    fn publish_drop(&self, shell: &mut Shell<'_, Message>) {
        if let Some(on_drag) = &self.on_drag {
            shell.publish(on_drag(DragEvent::Dropped));
        }
    }

    fn update_resize(
        &self,
        interaction: &mut Interaction,
        event: &mouse::Event,
        shell: &mut Shell<'_, Message>,
    ) -> bool {
        use mouse::Button;

        let Some(mut resizing) = interaction.resizing.take() else {
            return false;
        };

        match event {
            mouse::Event::CursorMoved { position } => {
                let next = resize_widths(
                    &resizing.start,
                    &resizing.min,
                    resizing.divider,
                    position.x - resizing.origin_x,
                );
                if !widths_equal(&next, &resizing.current) {
                    resizing.current = next;
                    shell.invalidate_layout();
                    shell.request_redraw();
                }
                interaction.resizing = Some(resizing);
            }
            mouse::Event::ButtonReleased(Button::Left) => {
                if !widths_equal(&resizing.current, &resizing.start) {
                    let widths = self
                        .entries
                        .iter()
                        .map(|(pane, _)| *pane)
                        .zip(resizing.current.iter().copied())
                        .collect();
                    shell.publish((self.on_resize)(widths));
                    shell.invalidate_layout();
                }
                shell.request_redraw();
            }
            mouse::Event::CursorLeft => {
                if !widths_equal(&resizing.current, &resizing.start) {
                    shell.invalidate_layout();
                }
                self.hover(interaction, None, shell);
                shell.request_redraw();
            }
            _ => interaction.resizing = Some(resizing),
        }
        shell.capture_event();
        true
    }
}

fn solve_widths(specs: impl IntoIterator<Item = (f32, f32)>, available: f32) -> Vec<f32> {
    let available = finite_nonnegative(available);
    let mut free: Vec<_> = specs.into_iter().enumerate().collect();
    let mut min = fit_mins(free.iter().map(|&(_, spec)| spec), available);
    if min.iter().sum::<f32>() >= available - EPS {
        return fit_sum(min, available);
    }

    let mut remaining = available;
    while !free.is_empty() {
        let basis_sum: f64 = free
            .iter()
            .map(|&(i, spec)| width_basis(spec, min[i]))
            .sum();
        let available = f64::from(remaining.max(0.0));
        let mut fixed = false;
        free.retain(|&(i, spec)| {
            let keep = (available * width_basis(spec, min[i]) / basis_sum) as f32 >= min[i] - EPS;
            if !keep {
                remaining -= min[i];
                fixed = true;
            }
            keep
        });
        if !fixed {
            for (i, spec) in free {
                min[i] = (available * width_basis(spec, min[i]) / basis_sum) as f32;
            }
            break;
        }
    }
    fit_sum(min, available)
}

fn fit_mins(specs: impl IntoIterator<Item = (f32, f32)>, available: f32) -> Vec<f32> {
    let mut min: Vec<_> = specs
        .into_iter()
        .map(|(min, _)| finite_nonnegative(min))
        .collect();
    let sum = min.iter().sum::<f32>();
    if sum > available {
        for w in &mut min {
            *w *= available / sum;
        }
    }
    min
}

fn width_basis(spec: (f32, f32), min: f32) -> f64 {
    f64::from(finite_nonnegative(spec.1).max(min).max(1.0))
}

fn finite_nonnegative(value: f32) -> f32 {
    crate::util::finite_positive(value).unwrap_or(0.0)
}

fn fit_sum(mut widths: Vec<f32>, available: f32) -> Vec<f32> {
    let delta = available - widths.iter().sum::<f32>();
    if let Some(last) = widths.last_mut() {
        *last = (*last + delta).max(0.0);
    }
    widths
}

fn widths_equal(a: &[f32], b: &[f32]) -> bool {
    std::iter::zip(a, b).all(|(a, b)| (a - b).abs() <= EPS)
}

fn resize_widths(start: &[f32], min: &[f32], divider: usize, delta: f32) -> Vec<f32> {
    if delta.abs() <= EPS {
        return start.to_vec();
    }
    let mut widths = start.to_vec();
    if delta > 0.0 {
        transfer_width(&mut widths, min, divider, divider + 1..start.len(), delta);
    } else {
        transfer_width(&mut widths, min, divider + 1, (0..=divider).rev(), -delta);
    }
    fit_sum(widths, start.iter().sum())
}

fn transfer_width(
    widths: &mut [f32],
    min: &[f32],
    grow: usize,
    shrink: impl Iterator<Item = usize> + Clone,
    requested: f32,
) {
    let mut amount = requested.min(shrink.clone().map(|i| (widths[i] - min[i]).max(0.0)).sum());
    widths[grow] += amount;
    for i in shrink {
        let taken = (widths[i] - min[i]).max(0.0).min(amount);
        widths[i] -= taken;
        amount -= taken;
        if amount <= EPS {
            break;
        }
    }
}

impl<'a, Message: 'static> From<PaneGrid<'a, Message>> for Element<'a, Message> {
    fn from(pane_grid: PaneGrid<'a, Message>) -> Self {
        Element::new(pane_grid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solve_widths_uses_basis_and_minimums() {
        assert_eq!(
            solve_widths([(0.0, 1.0), (0.0, 3.0)], 800.0),
            [200.0, 600.0]
        );
        assert_eq!(
            solve_widths([(300.0, 1.0), (0.0, 100.0)], 400.0),
            [300.0, 100.0]
        );
    }

    #[test]
    fn resize_widths_takes_from_nearest_pane_first() {
        assert_eq!(
            resize_widths(&[200.0, 300.0, 500.0], &[100.0, 250.0, 100.0], 0, 200.0),
            [400.0, 250.0, 350.0],
        );
    }
}
