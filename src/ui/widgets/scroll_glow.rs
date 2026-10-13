// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::util::color::with_alpha;
use iced::Length::{Fill, Fixed, Shrink};
use iced::gradient;
use iced::widget::{
    Space, column, container, row, scrollable, scrollable::Scrollbar, sensor, stack,
};
use iced::{Element, Size};
use std::f32::consts::{FRAC_PI_2, PI};

const GLOW_SIZE: f32 = 24.0;

macro_rules! scroll_glow_axis {
    (
        $name:ident, $direction:ident, $extent:ident, $offset:ident, $layout:ident,
        $spacer_height:expr, $start:expr, $end:expr, $vertical:expr $(, $height:expr)?
    ) => {
        pub fn $name<'a, M: 'a>(
            &self,
            content: impl Into<Element<'a, M>>,
            on_scroll: impl Fn(ScrollEvent) -> M + Copy + 'a,
        ) -> Element<'a, M> {
            let content = sensor(content).on_resize(move |size: Size| {
                on_scroll(ScrollEvent::ContentResized(size.$extent))
            });
            let body = scrollable(content)
                .direction(scrollable::Direction::$direction(hidden_scrollbar()))
                .width(Fill)
                $(.height($height))?
                .on_scroll(move |vp: scrollable::Viewport| {
                    on_scroll(ScrollEvent::Scrolled(Self {
                        content: vp.content_bounds().$extent,
                        bounds: vp.bounds().$extent,
                        offset: vp.absolute_offset().$offset,
                    }))
                });
            let body = sensor(body).on_resize(move |size: Size| {
                on_scroll(ScrollEvent::BoundsResized(size.$extent))
            });
            let [show_start, show_end] = self.visible();
            stack![
                body,
                $layout![
                    glow(show_start, $start, $vertical),
                    Space::new().width(Fill).height($spacer_height),
                    glow(show_end, $end, $vertical),
                ]
            ]
            .into()
        }
    };
}

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

    scroll_glow_axis!(
        vertical, Vertical, height, y, column, Fill, PI, 0.0, true, Fill
    );
    scroll_glow_axis!(
        horizontal,
        Horizontal,
        width,
        x,
        row,
        Shrink,
        FRAC_PI_2,
        PI + FRAC_PI_2,
        false
    );
}

fn hidden_scrollbar() -> Scrollbar {
    Scrollbar::new().width(0).scroller_width(0)
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
