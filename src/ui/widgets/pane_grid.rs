mod content;
mod pane;
pub mod state;

pub use content::Content;
pub use pane::Pane;
pub use state::State;

use iced::advanced::renderer::{self, Quad};
use iced::advanced::widget::{
    self,
    tree::{self, Tree},
};
use iced::advanced::{self as core, Clipboard, Layout, Shell, Widget, layout, mouse};
use iced::{Background, Element, Event, Length, Point, Rectangle, Size};

use crate::util::color::with_alpha;

#[derive(Default)]
struct Interaction {
    dragging: Option<(Pane, Point)>,
    last_x: Option<f32>,
    cursor_over: Option<Pane>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragEvent {
    Picked { pane: Pane },
    Moved { pane: Pane, target: Pane },
    Dropped { pane: Pane },
    Canceled { pane: Pane },
}

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
            .find(|(_, child)| child.bounds().contains(cursor))
            .map(|((pane, _), _)| *pane)
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
            .map(|(_, content)| content.state())
            .collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children_custom(
            &self.entries,
            |state, entry| entry.1.diff(state),
            |entry| entry.1.state(),
        );
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
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

        let mut widths: Vec<f32> = Vec::with_capacity(count);
        let mut min_widths: Vec<f32> = Vec::with_capacity(count);
        let mut max_widths: Vec<f32> = Vec::with_capacity(count);

        for (_, content) in &self.entries {
            let (min, preferred, max) = content.width_hint();
            min_widths.push(min.max(0.0));
            widths.push(preferred.max(min));
            max_widths.push(max.max(min));
        }

        let total_width: f32 = widths.iter().sum();

        if total_width > available_width {
            distribute_deficit(&mut widths, &min_widths, total_width - available_width);
        } else if total_width < available_width {
            distribute_surplus(&mut widths, &max_widths, available_width - total_width);
        }

        let mut position = 0.0;
        let mut children = Vec::with_capacity(count);

        for (((_, content), child), width) in self
            .entries
            .iter_mut()
            .zip(tree.children.iter_mut())
            .zip(widths.into_iter())
        {
            let pane_width = width.max(0.0);
            let limits = layout::Limits::new(
                Size::new(pane_width, size.height),
                Size::new(pane_width, size.height),
            );

            let node = content
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
            content.operate(child, child_layout, renderer, operation);
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
        let interaction = tree.state.downcast_mut::<Interaction>();

        for (((_, content), child), child_layout) in self
            .entries
            .iter_mut()
            .zip(tree.children.iter_mut())
            .zip(layout.children())
        {
            content.update(
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

        if let Event::Mouse(mouse_event) = event {
            use mouse::Button;

            match mouse_event {
                mouse::Event::ButtonPressed(Button::Left) if self.on_drag.is_some() => {
                    if let Some(on_drag) = &self.on_drag
                        && let Some(cursor_position) = cursor.position()
                        && let Some(pane) = self.pane_at(layout, cursor_position)
                    {
                        interaction.dragging = Some((pane, cursor_position));
                        interaction.last_x = Some(cursor_position.x);
                        shell.publish(on_drag(DragEvent::Picked { pane }));
                        shell.capture_event();
                    }
                }
                mouse::Event::ButtonPressed(Button::Right) => {
                    if let Some(on_context) = &self.on_context
                        && let Some(cursor_position) = cursor.position()
                        && let Some(pane) = self.pane_at(layout, cursor_position)
                    {
                        shell.publish(on_context(pane));
                        shell.capture_event();
                    }
                }
                mouse::Event::CursorMoved { position } => {
                    let pane_under_cursor = self.pane_at(layout, *position);

                    if interaction.cursor_over != pane_under_cursor {
                        interaction.cursor_over = pane_under_cursor;
                        if let Some(on_hover) = &self.on_hover {
                            shell.publish(on_hover(pane_under_cursor));
                        }
                    }

                    if let Some((pane, origin)) = interaction.dragging {
                        const DRAG_DEADBAND: f32 = 5.0;
                        if position.distance(origin) > DRAG_DEADBAND {
                            let last_x = interaction.last_x.unwrap_or(position.x);
                            let dragged_idx = self.entries.iter().position(|(p, _)| *p == pane);

                            if let Some(idx) = dragged_idx {
                                let neighbor_idx = if position.x > last_x {
                                    (idx + 1 < self.entries.len()).then_some(idx + 1)
                                } else if position.x < last_x {
                                    idx.checked_sub(1)
                                } else {
                                    None
                                };

                                if let Some(n_idx) = neighbor_idx
                                    && let Some(child_layout) = layout.children().nth(n_idx)
                                {
                                    let n_bounds = child_layout.bounds();
                                    let n_center = n_bounds.x + n_bounds.width / 2.0;
                                    let crossed = (n_idx > idx && position.x > n_center)
                                        || (n_idx < idx && position.x < n_center);

                                    if crossed && let Some(on_drag) = &self.on_drag {
                                        let target = self.entries[n_idx].0;
                                        shell.publish(on_drag(DragEvent::Moved { pane, target }));
                                    }
                                }
                            }
                        }
                        interaction.last_x = Some(position.x);
                        shell.capture_event();
                    }
                }
                mouse::Event::ButtonReleased(Button::Left) => {
                    if let Some((pane, _)) = interaction.dragging.take() {
                        interaction.last_x = None;
                        if let Some(on_drag) = &self.on_drag {
                            shell.publish(on_drag(DragEvent::Dropped { pane }));
                        }
                        shell.capture_event();
                    }
                }
                mouse::Event::CursorLeft => {
                    if let Some((pane, _)) = interaction.dragging.take()
                        && let Some(on_drag) = &self.on_drag
                    {
                        shell.publish(on_drag(DragEvent::Canceled { pane }));
                    }

                    interaction.last_x = None;
                    if interaction.cursor_over.is_some() {
                        interaction.cursor_over = None;
                        if let Some(on_hover) = &self.on_hover {
                            shell.publish(on_hover(None));
                        }
                    }
                }
                _ => {}
            }
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

        self.entries
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|(((_, content), child), child_layout)| {
                content.mouse_interaction(child, child_layout, cursor, viewport, renderer)
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
        let dragging = tree.state.downcast_ref::<Interaction>().dragging;
        for (((pane, content), child), child_layout) in self
            .entries
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
        {
            content.draw(
                child,
                renderer,
                theme,
                defaults,
                child_layout,
                cursor,
                viewport,
            );
            if dragging.is_some_and(|(p, _)| p == *pane) {
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
    }
}

fn distribute_proportionally(
    widths: &mut [f32],
    entries: &mut [(usize, f32)],
    target: f32,
    sign: f32,
) {
    loop {
        let delta = (target - widths.iter().sum::<f32>()) * sign;
        if delta <= f32::EPSILON {
            break;
        }
        let remaining: f32 = entries.iter().map(|(_, c)| *c).sum();
        if remaining <= f32::EPSILON {
            break;
        }
        for (i, capacity) in entries.iter_mut() {
            if *capacity <= f32::EPSILON {
                continue;
            }
            let portion = (delta * (*capacity / remaining)).min(*capacity);
            widths[*i] += sign * portion;
            *capacity -= portion;
        }
    }
}

fn distribute_deficit(widths: &mut [f32], min_widths: &[f32], initial_deficit: f32) {
    let target = widths.iter().sum::<f32>() - initial_deficit;
    let mut entries: Vec<(usize, f32)> = widths
        .iter()
        .enumerate()
        .map(|(i, w)| (i, (w - min_widths[i]).max(0.0)))
        .collect();
    distribute_proportionally(widths, &mut entries, target, -1.0);
}

fn distribute_surplus(widths: &mut [f32], max_widths: &[f32], initial_surplus: f32) {
    let target = widths.iter().sum::<f32>() + initial_surplus;
    let growable: Vec<(usize, f32)> = widths
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let max = max_widths[i];
            let capacity = if max.is_infinite() {
                f32::INFINITY
            } else {
                (max - w).max(0.0)
            };
            (i, capacity)
        })
        .collect();

    // Infinite-capacity entries absorb all surplus first.
    let infinite_indices: Vec<usize> = growable
        .iter()
        .filter_map(|(i, c)| c.is_infinite().then_some(*i))
        .collect();

    if !infinite_indices.is_empty() {
        let share = initial_surplus / infinite_indices.len() as f32;
        for i in infinite_indices {
            widths[i] += share;
        }
        return;
    }

    let mut finite: Vec<(usize, f32)> = growable.into_iter().filter(|(_, c)| *c > 0.0).collect();
    distribute_proportionally(widths, &mut finite, target, 1.0);
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
