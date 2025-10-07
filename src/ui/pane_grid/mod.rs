mod content;
mod pane;
pub mod state;

pub use content::Content;
pub use pane::Pane;
pub use state::State;

use iced_widget::core::event::{self, Event};
use iced_widget::core::layout;
use iced_widget::core::mouse;
use iced_widget::core::overlay;
use iced_widget::core::renderer;
use iced_widget::core::renderer::Quad;
use iced_widget::core::widget::{
    self,
    tree::{self, Tree},
};
use iced_widget::core::{
    self, Background, Clipboard, Color, Element, Layout, Length, Point, Rectangle, Shell, Size,
    Vector, Widget,
};

#[derive(Default)]
struct Interaction {
    dragging: Option<Pane>,
    hovered: Option<Pane>,
}

/// Event emitted when a drag interaction occurs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragEvent {
    Picked { pane: Pane },
    Dropped { pane: Pane, target: Target },
    Canceled { pane: Pane },
}

/// Drop target for drag events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Pane(Pane),
}

/// Lightweight, horizontal-only pane grid widget.
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
    spacing: f32,
    on_drag: Option<Box<dyn Fn(DragEvent) -> Message + 'a>>,
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
            spacing: 0.0,
            on_drag: None,
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

    pub fn spacing(mut self, amount: f32) -> Self {
        self.spacing = amount.max(0.0);
        self
    }

    pub fn on_drag(mut self, callback: impl Fn(DragEvent) -> Message + 'a) -> Self {
        self.on_drag = Some(Box::new(callback));
        self
    }

    fn drag_enabled(&self) -> bool {
        self.on_drag.is_some()
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
        &self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let count = self.entries.len();
        let size = limits.resolve(self.width, self.height, Size::ZERO);

        if count == 0 {
            return layout::Node::new(size);
        }

        let total_spacing = self.spacing * (count.saturating_sub(1) as f32);
        let available_width = (size.width - total_spacing).max(0.0);
        let pane_width = available_width / count as f32;

        let mut position = 0.0;
        let mut children = Vec::with_capacity(count);

        for ((_, content), child) in self.entries.iter().zip(tree.children.iter_mut()) {
            let limits = layout::Limits::new(
                Size::new(pane_width, size.height),
                Size::new(pane_width, size.height),
            );

            let node = content
                .layout(child, renderer, &limits)
                .move_to(Point::new(position, 0.0));

            position += pane_width + self.spacing;
            children.push(node);
        }

        layout::Node::with_children(size, children)
    }

    fn operate(
        &self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        for (((_, content), child), child_layout) in self
            .entries
            .iter()
            .zip(tree.children.iter_mut())
            .zip(layout.children())
        {
            content.operate(child, child_layout, renderer, operation);
        }
    }

    fn on_event(
        &mut self,
        tree: &mut Tree,
        event: Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) -> event::Status {
        let interaction = tree.state.downcast_mut::<Interaction>();
        let mut status = event::Status::Ignored;

        for (((_, content), child), child_layout) in self
            .entries
            .iter_mut()
            .zip(tree.children.iter_mut())
            .zip(layout.children())
        {
            status = status.merge(content.on_event(
                child,
                event.clone(),
                child_layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            ));
        }

        if let Event::Mouse(mouse_event) = event {
            use mouse::Button;

            match mouse_event {
                mouse::Event::ButtonPressed(Button::Left) if self.drag_enabled() => {
                    if let Some(on_drag) = &self.on_drag {
                        if let Some(cursor_position) = cursor.position() {
                            if let Some(pane) = self.pane_at(layout, cursor_position) {
                                interaction.dragging = Some(pane);
                                interaction.hovered = Some(pane);
                                shell.publish(on_drag(DragEvent::Picked { pane }));
                                return event::Status::Captured;
                            }
                        }
                    }
                }
                mouse::Event::CursorMoved { position } => {
                    if interaction.dragging.is_some() {
                        let hovered = self.pane_at(layout, position);

                        if interaction.hovered != hovered {
                            interaction.hovered = hovered;
                            status = status.merge(event::Status::Captured);
                        }
                    }
                }
                mouse::Event::ButtonReleased(Button::Left) => {
                    if let Some(pane) = interaction.dragging.take() {
                        interaction.hovered = None;
                        if let Some(on_drag) = &self.on_drag {
                            let event = cursor
                                .position()
                                .and_then(|pos| self.pane_at(layout, pos))
                                .map(|target| DragEvent::Dropped {
                                    pane,
                                    target: Target::Pane(target),
                                })
                                .unwrap_or(DragEvent::Canceled { pane });

                            shell.publish(on_drag(event));
                        }

                        return event::Status::Captured;
                    }
                }
                mouse::Event::CursorLeft => {
                    if let Some(pane) = interaction.dragging.take() {
                        if let Some(on_drag) = &self.on_drag {
                            shell.publish(on_drag(DragEvent::Canceled { pane }));
                        }
                    }

                    interaction.hovered = None;
                }
                _ => {}
            }
        }

        status
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
        let interaction = tree.state.downcast_ref::<Interaction>();
        let highlight_pane = if interaction.dragging.is_some() {
            interaction.hovered
        } else {
            None
        };

        let mut highlight_bounds = None;

        for (((pane, content), child), child_layout) in self
            .entries
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
        {
            if Some(*pane) == highlight_pane {
                highlight_bounds = Some(child_layout.bounds());
            }

            content.draw(
                child,
                renderer,
                theme,
                defaults,
                child_layout,
                cursor,
                viewport,
            );
        }

        if let Some(bounds) = highlight_bounds {
            let fill = Color::from_rgba(0.95, 0.97, 0.99, 0.18);
            let border = Color::from_rgba(0.80, 0.82, 0.86, 0.5);

            renderer.fill_quad(
                Quad {
                    bounds,
                    border: iced_widget::core::Border {
                        radius: Default::default(),
                        width: 1.0,
                        color: border,
                    },
                    shadow: Default::default(),
                },
                Background::Color(fill),
            );
        }
    }

    fn overlay<'b>(
        &'b mut self,
        _tree: &'b mut Tree,
        _layout: Layout<'_>,
        _renderer: &Renderer,
        _translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        None
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
