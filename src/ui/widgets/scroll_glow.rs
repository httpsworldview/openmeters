// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::util::color::with_alpha;
use iced::Length::{Fill, Fixed, Shrink};
use iced::gradient;
use iced::widget::{
    Column, Row, Space, container, scrollable, scrollable::Scrollbar, sensor, stack,
};
use iced::{Element, Size};
use std::f32::consts::{FRAC_PI_2, PI};

const GLOW_SIZE: f32 = 24.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScrollEvent {
    Scrolled(ScrollGlow),
    // Iced reports no viewport once content fits, e.g. after it shrinks.
    ContentResized(f32),
    BoundsResized(f32),
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ScrollGlow {
    content: f32,
    bounds: f32,
    offset: f32,
}

impl ScrollGlow {
    pub fn update(&mut self, event: ScrollEvent) {
        match event {
            ScrollEvent::Scrolled(glow) => *self = glow,
            ScrollEvent::ContentResized(content) => self.content = content,
            ScrollEvent::BoundsResized(bounds) => self.bounds = bounds,
        }
    }

    fn visible(&self) -> [bool; 2] {
        let range = self.content - self.bounds;
        if range <= 0.0 {
            return [false; 2];
        }
        let relative = self.offset / range;
        [relative > 0.01, relative < 0.99]
    }

    pub fn vertical<'a, M: 'a>(
        &self,
        content: impl Into<Element<'a, M>>,
        on_scroll: impl Fn(ScrollEvent) -> M + Copy + 'a,
    ) -> Element<'a, M> {
        self.view(true, content, on_scroll)
    }

    pub fn horizontal<'a, M: 'a>(
        &self,
        content: impl Into<Element<'a, M>>,
        on_scroll: impl Fn(ScrollEvent) -> M + Copy + 'a,
    ) -> Element<'a, M> {
        self.view(false, content, on_scroll)
    }

    fn view<'a, M: 'a>(
        &self,
        vertical: bool,
        content: impl Into<Element<'a, M>>,
        on_scroll: impl Fn(ScrollEvent) -> M + Copy + 'a,
    ) -> Element<'a, M> {
        let extent = move |size: Size| if vertical { size.height } else { size.width };
        let (direction, height, angles) = if vertical {
            let direction = scrollable::Direction::Vertical(Scrollbar::hidden());
            (direction, Fill, [PI, 0.0])
        } else {
            let direction = scrollable::Direction::Horizontal(Scrollbar::hidden());
            (direction, Shrink, [FRAC_PI_2, PI + FRAC_PI_2])
        };
        let content = sensor(content)
            .on_resize(move |size| on_scroll(ScrollEvent::ContentResized(extent(size))));
        let body = scrollable(content)
            .direction(direction)
            .width(Fill)
            .height(height)
            .on_scroll(move |vp: scrollable::Viewport| {
                let offset = vp.absolute_offset();
                on_scroll(ScrollEvent::Scrolled(Self {
                    content: extent(vp.content_bounds().size()),
                    bounds: extent(vp.bounds().size()),
                    offset: if vertical { offset.y } else { offset.x },
                }))
            });
        let body =
            sensor(body).on_resize(move |size| on_scroll(ScrollEvent::BoundsResized(extent(size))));
        let [show_start, show_end] = self.visible();
        let glows = [
            glow(show_start, angles[0], vertical).into(),
            Space::new().width(Fill).height(height).into(),
            glow(show_end, angles[1], vertical).into(),
        ];
        let glows: Element<'a, M> = if vertical {
            Column::with_children(glows).into()
        } else {
            Row::with_children(glows).into()
        };
        stack![body, glows].into()
    }
}

fn glow<'a, M: 'a>(
    show: bool,
    angle: f32,
    vertical: bool,
) -> container::Container<'a, M, iced::Theme> {
    let size = Fixed(if show { GLOW_SIZE } else { 0.0 });
    let (w, h) = if vertical { (Fill, size) } else { (size, Fill) };
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
                    .add_stop(1.0, with_alpha(bg, 0.0))
                    .into(),
            ),
            ..Default::default()
        }
    })
}
