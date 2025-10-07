use iced_widget::core::event::{self, Event};
use iced_widget::core::layout;
use iced_widget::core::mouse;
use iced_widget::core::renderer;
use iced_widget::core::widget::{self, Tree};
use iced_widget::core::{self, Clipboard, Element, Layout, Rectangle, Shell};

/// pane content wrapper used by the pane grid.
#[allow(missing_debug_implementations)]
pub struct Content<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer>
where
    Renderer: core::Renderer,
{
    body: Element<'a, Message, Theme, Renderer>,
}

impl<'a, Message, Theme, Renderer> Content<'a, Message, Theme, Renderer>
where
    Renderer: core::Renderer,
{
    /// Creates new [`Content`] from any `Element`.
    pub fn new(body: impl Into<Element<'a, Message, Theme, Renderer>>) -> Self {
        Self { body: body.into() }
    }

    pub(super) fn state(&self) -> Tree {
        Tree::new(&self.body)
    }

    pub(super) fn diff(&self, tree: &mut Tree) {
        tree.diff(&self.body);
    }

    pub(super) fn layout(
        &self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.body.as_widget().layout(tree, renderer, limits)
    }

    pub(super) fn operate(
        &self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) where
        Message: 'a,
        Theme: 'a,
    {
        self.body
            .as_widget()
            .operate(tree, layout, renderer, operation);
    }

    pub(super) fn on_event(
        &mut self,
        tree: &mut Tree,
        event: Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) -> event::Status
    where
        Message: 'a,
        Theme: 'a,
    {
        self.body.as_widget_mut().on_event(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        )
    }

    pub(super) fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        defaults: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) where
        Message: 'a,
        Theme: 'a,
    {
        self.body
            .as_widget()
            .draw(tree, renderer, theme, defaults, layout, cursor, viewport);
    }

    pub(super) fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction
    where
        Message: 'a,
        Theme: 'a,
    {
        self.body
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }
}
