use iced::advanced::renderer::{self, Quad};
use iced::advanced::widget::{
    self,
    tree::{self, Tree},
};
use iced::advanced::{self as core, Clipboard, Layout, Shell, Widget, layout, mouse};
use iced::{Background, Element, Event, Length, Point, Rectangle, Size};

use crate::util::color::with_alpha;

// This type is adapted from iced_widget v0.13.4 (MIT License).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pane(usize);

#[derive(Debug, Clone)]
pub struct State<T> {
    panes: Vec<(Pane, T)>,
}

impl<T> State<T> {
    pub fn from_iter(items: impl IntoIterator<Item = T>) -> Option<Self> {
        let panes = items
            .into_iter()
            .enumerate()
            .map(|(id, item)| (Pane(id), item))
            .collect::<Vec<_>>();
        (!panes.is_empty()).then_some(Self { panes })
    }

    pub fn get(&self, pane: Pane) -> Option<&T> {
        self.panes.iter().find(|(p, _)| *p == pane).map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, pane: Pane) -> Option<&mut T> {
        self.panes
            .iter_mut()
            .find(|(p, _)| *p == pane)
            .map(|(_, v)| v)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Pane, &T)> {
        self.panes.iter().map(|(pane, state)| (pane, state))
    }

    pub fn move_to(&mut self, a: Pane, b: Pane) -> bool {
        let (Some(from), Some(to)) = (self.position(a), self.position(b)) else {
            return false;
        };
        if from == to {
            return false;
        }
        let pane = self.panes.remove(from);
        self.panes.insert(to, pane);
        true
    }

    pub fn for_each_mut(&mut self, mut f: impl FnMut(Pane, &mut T)) {
        for (pane, value) in &mut self.panes {
            f(*pane, value);
        }
    }

    fn position(&self, pane: Pane) -> Option<usize> {
        self.panes.iter().position(|(id, _)| *id == pane)
    }
}

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
    dragging: Option<(Pane, Point)>,
    resizing: Option<ResizeState>,
    last_x: Option<f32>,
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
pub struct Content<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer>
where
    Renderer: core::Renderer,
{
    body: Element<'a, Message, Theme, Renderer>,
    min_width: f32,
    basis_width: f32,
}

impl<'a, Message, Theme, Renderer> Content<'a, Message, Theme, Renderer>
where
    Renderer: core::Renderer,
{
    pub fn new(body: impl Into<Element<'a, Message, Theme, Renderer>>) -> Self {
        Self {
            body: body.into(),
            min_width: 0.0,
            basis_width: 0.0,
        }
    }

    pub fn with_width_basis(mut self, min: f32, basis: f32) -> Self {
        self.min_width = min;
        self.basis_width = basis;
        self
    }
}

// Callback closures do not implement Debug; this mirrors iced's widget types.
#[allow(missing_debug_implementations)]
pub struct PaneGrid<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: core::Renderer,
{
    entries: Vec<(Pane, Content<'a, Message, Theme, Renderer>)>,
    width: Length,
    height: Length,
    on_drag: Option<Box<dyn Fn(DragEvent) -> Message + 'a>>,
    on_resize: Option<Box<dyn Fn(ResizeWidths) -> Message + 'a>>,
    on_context: Option<Box<dyn Fn(Pane) -> Message + 'a>>,
    on_hover: Option<Box<dyn Fn(Option<Pane>) -> Message + 'a>>,
}

impl<'a, Message, Theme, Renderer> PaneGrid<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: core::Renderer,
{
    pub fn new<T>(
        state: &'a State<T>,
        view: impl Fn(Pane, &'a T) -> Content<'a, Message, Theme, Renderer>,
    ) -> Self {
        let entries = state
            .iter()
            .map(|(pane, value)| (*pane, view(*pane, value)))
            .collect();

        Self {
            entries,
            width: Length::Fill,
            height: Length::Fill,
            on_drag: None,
            on_resize: None,
            on_context: None,
            on_hover: None,
        }
    }

    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    pub fn on_drag(mut self, callback: impl Fn(DragEvent) -> Message + 'a) -> Self {
        self.on_drag = Some(Box::new(callback));
        self
    }

    pub fn on_resize(mut self, callback: impl Fn(ResizeWidths) -> Message + 'a) -> Self {
        self.on_resize = Some(Box::new(callback));
        self
    }

    pub fn on_context_request(mut self, callback: impl Fn(Pane) -> Message + 'a) -> Self {
        self.on_context = Some(Box::new(callback));
        self
    }

    pub fn on_hover(mut self, callback: impl Fn(Option<Pane>) -> Message + 'a) -> Self {
        self.on_hover = Some(Box::new(callback));
        self
    }

    fn pane_at(&self, layout: Layout<'_>, cursor: Point) -> Option<Pane> {
        self.entries
            .iter()
            .zip(layout.children())
            .find_map(|((pane, _), child)| child.bounds().contains(cursor).then_some(*pane))
    }

    fn divider_at(&self, layout: Layout<'_>, cursor: Point) -> Option<usize> {
        if self.entries.len() < 2 || !layout.bounds().contains(cursor) {
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

    fn width_specs(&self) -> Vec<(f32, f32)> {
        self.entries
            .iter()
            .map(|(_, content)| (content.min_width, content.basis_width))
            .collect()
    }

    fn pair_widths(&self, widths: &[f32]) -> Vec<(Pane, f32)> {
        self.entries
            .iter()
            .map(|(pane, _)| *pane)
            .zip(widths.iter().copied())
            .collect()
    }
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for PaneGrid<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: core::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Interaction>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(Interaction::default())
    }

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
        Size::new(self.width, self.height)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let count = self.entries.len();
        let size = limits.resolve(self.width, self.height, Size::ZERO);

        if count == 0 {
            return layout::Node::new(size);
        }

        let available_width = size.width.max(0.0);
        let interaction = tree.state.downcast_ref::<Interaction>();
        let widths = interaction
            .resizing
            .as_ref()
            .filter(|r| {
                r.current.len() == count
                    && (r.current.iter().sum::<f32>() - available_width).abs() < 0.5
            })
            .map(|r| fit_sum(r.current.clone(), available_width))
            .unwrap_or_else(|| solve_widths(&self.width_specs(), available_width));

        let mut position = 0.0;
        let mut children = Vec::with_capacity(count);

        for (((_, content), child), width) in self
            .entries
            .iter_mut()
            .zip(tree.children.iter_mut())
            .zip(widths)
        {
            let pane_width = width.max(0.0);
            let limits = layout::Limits::new(
                Size::new(pane_width, size.height),
                Size::new(pane_width, size.height),
            );

            let node = content
                .body
                .as_widget_mut()
                .layout(child, renderer, &limits)
                .move_to(Point::new(position, 0.0));

            position += pane_width;
            children.push(node);
        }

        layout::Node::with_children(size, children)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        for (((_, content), child), child_layout) in self
            .entries
            .iter_mut()
            .zip(tree.children.iter_mut())
            .zip(layout.children())
        {
            content
                .body
                .as_widget_mut()
                .operate(child, child_layout, renderer, operation);
        }
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if self.update_resize(tree, event, shell)
            || self.update_interaction(tree, event, layout, cursor, shell)
        {
            return;
        }

        for (((_, content), child), child_layout) in self
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
            let pane = self.pane_at(layout, *position);
            self.publish_hover(tree.state.downcast_mut::<Interaction>(), pane, shell);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let interaction = tree.state.downcast_ref::<Interaction>();

        if interaction.dragging.is_some() {
            return mouse::Interaction::Grabbing;
        }
        if interaction.resizing.is_some()
            || (self.on_resize.is_some()
                && cursor
                    .position()
                    .is_some_and(|p| self.divider_at(layout, p).is_some()))
        {
            return mouse::Interaction::ResizingHorizontally;
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

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        defaults: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let interaction = tree.state.downcast_ref::<Interaction>();
        for (((pane, content), child), child_layout) in self
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
            if interaction.dragging.is_some_and(|(p, _)| p == *pane) {
                let accent = crate::ui::theme::accent_primary();
                renderer.fill_quad(
                    Quad {
                        bounds: child_layout.bounds(),
                        border: iced::Border {
                            radius: Default::default(),
                            width: 2.0,
                            color: with_alpha(accent, 0.9),
                        },
                        shadow: Default::default(),
                        snap: true,
                    },
                    Background::Color(with_alpha(accent, 0.4)),
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
                    border: Default::default(),
                    shadow: Default::default(),
                    snap: true,
                },
                Background::Color(with_alpha(crate::ui::theme::accent_primary(), 0.75)),
            );
        }
    }
}

impl<'a, Message, Theme, Renderer> PaneGrid<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: core::Renderer,
{
    fn publish_hover(
        &self,
        interaction: &mut Interaction,
        pane: Option<Pane>,
        shell: &mut Shell<'_, Message>,
    ) {
        if interaction.cursor_over != pane {
            interaction.cursor_over = pane;
            if let Some(on_hover) = &self.on_hover {
                shell.publish(on_hover(pane));
            }
        }
    }

    fn update_interaction(
        &self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        shell: &mut Shell<'_, Message>,
    ) -> bool {
        let Event::Mouse(mouse_event) = event else {
            return false;
        };
        use mouse::Button;

        if let mouse::Event::CursorLeft = mouse_event {
            let interaction = tree.state.downcast_mut::<Interaction>();
            let dragging = interaction.dragging.take();
            interaction.last_x = None;
            self.publish_hover(interaction, None, shell);
            if dragging.is_none() {
                return false;
            }
            shell.capture_event();
            return true;
        }

        let interaction = tree.state.downcast_ref::<Interaction>();
        if let Some((pane, origin)) = interaction.dragging {
            match mouse_event {
                mouse::Event::CursorMoved { position } => {
                    const DRAG_DEADBAND: f32 = 5.0;
                    let last_x = interaction.last_x.unwrap_or(position.x);
                    if position.distance(origin) > DRAG_DEADBAND
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
                    tree.state.downcast_mut::<Interaction>().last_x = Some(position.x);
                }
                mouse::Event::ButtonReleased(Button::Left) => {
                    let interaction = tree.state.downcast_mut::<Interaction>();
                    interaction.dragging = None;
                    interaction.last_x = None;
                    if let Some(on_drag) = &self.on_drag {
                        shell.publish(on_drag(DragEvent::Dropped));
                    }
                }
                _ => {}
            }
            shell.capture_event();
            return true;
        }

        match mouse_event {
            mouse::Event::ButtonPressed(Button::Left) => {
                let Some(position) = cursor.position() else {
                    return false;
                };
                if self.on_resize.is_some()
                    && let Some(divider) = self.divider_at(layout, position)
                {
                    let start = layout
                        .children()
                        .map(|c| c.bounds().width.max(0.0))
                        .collect::<Vec<_>>();
                    tree.state.downcast_mut::<Interaction>().resizing = Some(ResizeState {
                        divider,
                        origin_x: position.x,
                        min: fit_mins(&self.width_specs(), start.iter().sum()),
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
                    let interaction = tree.state.downcast_mut::<Interaction>();
                    interaction.dragging = Some((pane, position));
                    interaction.last_x = Some(position.x);
                    shell.capture_event();
                    return true;
                }
            }
            mouse::Event::ButtonPressed(Button::Right) => {
                if let Some(on_context) = &self.on_context
                    && let Some(position) = cursor.position()
                    && let Some(pane) = self.pane_at(layout, position)
                {
                    shell.publish(on_context(pane));
                    shell.capture_event();
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    fn update_resize(
        &self,
        tree: &mut Tree,
        event: &Event,
        shell: &mut Shell<'_, Message>,
    ) -> bool {
        let interaction = tree.state.downcast_mut::<Interaction>();
        let Some(mut resizing) = interaction.resizing.take() else {
            return false;
        };
        let Event::Mouse(mouse_event) = event else {
            interaction.resizing = Some(resizing);
            return false;
        };
        use mouse::Button;

        match mouse_event {
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
                    if let Some(on_resize) = &self.on_resize {
                        shell.publish(on_resize(self.pair_widths(&resizing.current)));
                    }
                    shell.invalidate_layout();
                }
                shell.request_redraw();
            }
            mouse::Event::CursorLeft => {
                if !widths_equal(&resizing.current, &resizing.start) {
                    shell.invalidate_layout();
                }
                self.publish_hover(interaction, None, shell);
                shell.request_redraw();
            }
            _ => {
                interaction.resizing = Some(resizing);
            }
        }
        shell.capture_event();
        true
    }
}

fn solve_widths(specs: &[(f32, f32)], available: f32) -> Vec<f32> {
    let available = finite_positive(available);
    let mut min = fit_mins(specs, available);
    let min_sum = min.iter().sum::<f32>();
    if min_sum >= available - EPS {
        return fit_sum(min, available);
    }

    let mut free: Vec<_> = (0..specs.len()).collect();
    let mut remaining = available;
    while !free.is_empty() {
        let basis_sum: f64 = free.iter().map(|&i| width_basis(specs[i], min[i])).sum();
        let available = f64::from(remaining.max(0.0));
        let mut fixed = false;
        for i in std::mem::take(&mut free) {
            let width = (available * width_basis(specs[i], min[i]) / basis_sum) as f32;
            if width < min[i] - EPS {
                remaining -= min[i];
                fixed = true;
            } else {
                free.push(i);
            }
        }
        if !fixed {
            for i in free {
                min[i] = (available * width_basis(specs[i], min[i]) / basis_sum) as f32;
            }
            break;
        }
    }
    fit_sum(min, available)
}

fn fit_mins(specs: &[(f32, f32)], available: f32) -> Vec<f32> {
    let mut min: Vec<_> = specs.iter().map(|(min, _)| finite_positive(*min)).collect();
    let sum = min.iter().sum::<f32>();
    if sum > available && sum > EPS {
        let scale = available / sum;
        min.iter_mut().for_each(|w| *w *= scale);
    }
    min
}

fn width_basis(spec: (f32, f32), min: f32) -> f64 {
    f64::from(finite_positive(spec.1).max(min).max(1.0))
}

fn finite_positive(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn fit_sum(mut widths: Vec<f32>, available: f32) -> Vec<f32> {
    let delta = available - widths.iter().sum::<f32>();
    if let Some(last) = widths.last_mut() {
        *last = (*last + delta).max(0.0);
    }
    widths
}

fn widths_equal(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && std::iter::zip(a, b).all(|(a, b)| (a - b).abs() <= EPS)
}

fn resize_widths(start: &[f32], min: &[f32], divider: usize, delta: f32) -> Vec<f32> {
    if start.len() != min.len() || divider + 1 >= start.len() || delta.abs() <= EPS {
        return start.to_vec();
    }
    let mut widths = start.to_vec();
    if delta > 0.0 {
        let amount = delta.min(shrink_capacity(&widths, min, divider + 1..start.len()));
        apply_nearest(&mut widths, min, (0..=divider).rev(), amount, true);
        apply_nearest(&mut widths, min, divider + 1..start.len(), amount, false);
    } else {
        let amount = (-delta).min(shrink_capacity(&widths, min, (0..=divider).rev()));
        apply_nearest(&mut widths, min, divider + 1..start.len(), amount, true);
        apply_nearest(&mut widths, min, (0..=divider).rev(), amount, false);
    }
    fit_sum(widths, start.iter().sum())
}

fn shrink_capacity(widths: &[f32], min: &[f32], order: impl Iterator<Item = usize>) -> f32 {
    order.map(|i| (widths[i] - min[i]).max(0.0)).sum()
}

fn apply_nearest(
    widths: &mut [f32],
    min: &[f32],
    order: impl Iterator<Item = usize>,
    mut amount: f32,
    grow: bool,
) {
    for i in order {
        if amount <= EPS {
            break;
        }
        let cap = if grow {
            amount
        } else {
            (widths[i] - min[i]).max(0.0).min(amount)
        };
        widths[i] += if grow { cap } else { -cap };
        amount -= cap;
    }
}

impl<'a, Message, Theme, Renderer> From<PaneGrid<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: core::Renderer + 'a,
{
    fn from(pane_grid: PaneGrid<'a, Message, Theme, Renderer>) -> Self {
        Element::new(pane_grid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solve_widths_uses_basis_and_minimums() {
        assert_eq!(
            solve_widths(&[(0.0, 1.0), (0.0, 3.0)], 800.0),
            [200.0, 600.0]
        );
        assert_eq!(
            solve_widths(&[(300.0, 1.0), (0.0, 100.0)], 400.0),
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
