use iced::advanced::renderer;
use iced::advanced::widget::{self, Tree};
use iced::advanced::{self as core, Clipboard, Layout, Shell, layout, mouse};
use iced::{Element, Event, Rectangle};

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

    pub(super) fn width_spec(&self) -> (f32, f32) {
        (self.min_width, self.basis_width)
    }

    pub(super) fn state(&self) -> Tree {
        Tree::new(&self.body)
    }

    pub(super) fn diff(&self, tree: &mut Tree) {
        tree.diff(&self.body);
    }

    pub(super) fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.body.as_widget_mut().layout(tree, renderer, limits)
    }

    pub(super) fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) where
        Message: 'a,
        Theme: 'a,
    {
        self.body
            .as_widget_mut()
            .operate(tree, layout, renderer, operation);
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) where
        Message: 'a,
        Theme: 'a,
    {
        self.body.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        );
    }

    #[allow(clippy::too_many_arguments)]
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
