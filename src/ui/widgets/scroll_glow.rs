// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

// Scrollable wrapper that replaces scrollbars with subtle gradient edge glows,
// indicating when content overflows above/below or left/right.

use iced::gradient;
use iced::widget::{Space, column, container, row, scrollable, scrollable::Scrollbar, stack};
use iced::{Color, Element, Length};
use std::f32::consts::{FRAC_PI_2, PI};

const GLOW_SIZE: f32 = 24.0;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ScrollGlow {
    pub show_start: bool,
    pub show_end: bool,
}

impl ScrollGlow {
    fn from_axis(content: f32, bounds: f32, rel: f32) -> Self {
        let overflows = content > bounds;
        Self {
            show_start: overflows && rel > 0.01,
            show_end: overflows && rel < 0.99,
        }
    }

    pub fn vertical<'a, M: 'a>(
        &self,
        content: impl Into<Element<'a, M>>,
        on_scroll: impl Fn(Self) -> M + 'a,
    ) -> Element<'a, M> {
        let body = scrollable(content)
            .direction(scrollable::Direction::Vertical(
                Scrollbar::new().width(0).scroller_width(0),
            ))
            .width(Length::Fill)
            .height(Length::Fill)
            .on_scroll(move |vp: scrollable::Viewport| {
                on_scroll(Self::from_axis(
                    vp.content_bounds().height,
                    vp.bounds().height,
                    vp.relative_offset().y,
                ))
            });
        stack![
            body,
            column![
                glow(self.show_start, PI, true),
                Space::new().width(Length::Fill).height(Length::Fill),
                glow(self.show_end, 0.0, true),
            ]
        ]
        .into()
    }

    pub fn horizontal<'a, M: 'a>(
        &self,
        content: impl Into<Element<'a, M>>,
        on_scroll: impl Fn(Self) -> M + 'a,
    ) -> Element<'a, M> {
        let body = scrollable(content)
            .direction(scrollable::Direction::Horizontal(
                Scrollbar::new().width(0).scroller_width(0),
            ))
            .width(Length::Fill)
            .on_scroll(move |vp: scrollable::Viewport| {
                on_scroll(Self::from_axis(
                    vp.content_bounds().width,
                    vp.bounds().width,
                    vp.relative_offset().x,
                ))
            });
        stack![
            body,
            row![
                glow(self.show_start, FRAC_PI_2, false),
                Space::new().width(Length::Fill).height(Length::Shrink),
                glow(self.show_end, PI + FRAC_PI_2, false),
            ]
        ]
        .into()
    }
}

fn glow<'a, M: 'a>(
    show: bool,
    angle: f32,
    vertical: bool,
) -> container::Container<'a, M, iced::Theme> {
    let size = Length::Fixed(if show { GLOW_SIZE } else { 0.0 });
    let (w, h) = if vertical {
        (Length::Fill, size)
    } else {
        (size, Length::Fill)
    };
    let c = container(Space::new().width(w).height(h));
    if !show {
        return c;
    }
    c.style(move |theme: &iced::Theme| {
        let bg = theme.extended_palette().background.weak.color;
        container::Style {
            background: Some(
                gradient::Linear::new(angle)
                    .add_stop(0.0, bg)
                    .add_stop(1.0, Color { a: 0.0, ..bg })
                    .into(),
            ),
            ..Default::default()
        }
    })
}
