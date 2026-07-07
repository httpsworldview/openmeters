// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::util::color::with_alpha;
use iced::Element;
use iced::Length::{Fill, Fixed, Shrink};
use iced::gradient;
use iced::widget::{Space, column, container, row, scrollable, scrollable::Scrollbar, stack};
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
            on_scroll: impl Fn(Self) -> M + 'a,
        ) -> Element<'a, M> {
            let body = scrollable(content)
                .direction(scrollable::Direction::$direction(hidden_scrollbar()))
                .width(Fill)
                $(.height($height))?
                .on_scroll(move |vp: scrollable::Viewport| {
                    on_scroll(Self::from_axis(
                        vp.content_bounds().$extent,
                        vp.bounds().$extent,
                        vp.relative_offset().$offset,
                    ))
                });
            stack![
                body,
                $layout![
                    glow(self.show_start, $start, $vertical),
                    Space::new().width(Fill).height($spacer_height),
                    glow(self.show_end, $end, $vertical),
                ]
            ]
            .into()
        }
    };
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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
