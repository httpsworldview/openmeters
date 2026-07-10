// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

pub(super) mod palette_editor;
pub(super) mod pane_grid;
pub(super) mod scroll_glow;

use crate::ui::theme;
use iced::{
    Element,
    Length::{Fill, Shrink},
    alignment::Vertical,
    widget::{
        Button, Column, Container, Row, Toggler, button, column, container, pick_list, row, slider,
        text,
        text::{IntoFragment, Wrapping},
        toggler,
    },
};
use std::{borrow::Cow, fmt};

pub(super) struct SliderRange {
    pub(super) min: f32,
    pub(super) max: f32,
    pub(super) step: f32,
}

impl SliderRange {
    pub(super) const fn new(min: f32, max: f32, step: f32) -> Self {
        Self { min, max, step }
    }

    pub(super) fn snap(self, value: f32) -> f32 {
        debug_assert!(self.step > 0.0, "SliderRange::snap expects a positive step");
        if self.step <= 0.0 {
            return value.clamp(self.min, self.max);
        }
        (self.min + ((value - self.min) / self.step).round() * self.step).clamp(self.min, self.max)
    }
}

pub(super) fn fill<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Container<'a, M> {
    container(content).width(Fill).height(Fill)
}

pub(super) fn page<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Container<'a, M> {
    fill(content).padding(16).style(theme::weak_container)
}

pub(super) fn clipped_text<'a, M: 'a>(
    content: impl IntoFragment<'a>,
    size: f32,
) -> Container<'a, M> {
    container(text(content).size(size).wrapping(Wrapping::None)).clip(true)
}

pub(super) fn slide<'a, M: Clone + 'a>(
    label: impl IntoFragment<'a>,
    value: f32,
    formatted: impl IntoFragment<'a>,
    range: SliderRange,
    on_change: impl Fn(f32) -> M + 'a,
) -> Column<'a, M> {
    column![
        row![
            clipped_text(label, theme::BODY_TEXT_SIZE).width(Fill),
            clipped_text(formatted, 11.0),
        ]
        .spacing(6)
        .width(Fill),
        slider::Slider::new(range.min..=range.max, value, on_change)
            .step(range.step)
            .style(theme::slider_style),
    ]
    .spacing(theme::CONTROL_GAP)
    .width(Fill)
}

pub(super) fn card<'a, M: 'a>(
    label: impl IntoFragment<'a>,
    content: impl Into<Element<'a, M>>,
) -> Container<'a, M> {
    container(
        column![clipped_text(label, 14.0), content.into()]
            .spacing(10)
            .width(Fill),
    )
    .padding(12)
    .width(Fill)
    .style(theme::weak_container)
}

pub(super) fn split<'a, M: 'a>(
    left: impl Into<Element<'a, M>>,
    right: impl Into<Element<'a, M>>,
) -> Row<'a, M> {
    row![container(left).width(Fill), container(right).width(Fill)]
        .spacing(16)
        .width(Fill)
}

pub(super) fn pick<'a, T, M>(
    label: impl IntoFragment<'a>,
    options: impl Into<Cow<'a, [T]>>,
    selected: T,
    on_select: impl Fn(T) -> M + 'a,
) -> Row<'a, M>
where
    T: Clone + PartialEq + fmt::Display + 'static,
    M: Clone + 'a,
{
    row![
        clipped_text(label, theme::BODY_TEXT_SIZE).width(Shrink),
        pick_list(options.into(), Some(selected), on_select).width(Fill),
    ]
    .spacing(theme::CONTROL_GAP)
    .align_y(Vertical::Center)
    .width(Fill)
}

pub(super) fn toggle<'a, M: 'a>(
    label: impl IntoFragment<'a>,
    value: bool,
    on_toggle: impl Fn(bool) -> M + 'a,
) -> Toggler<'a, M> {
    toggler(value)
        .label(label)
        .spacing(4)
        .text_size(11)
        .on_toggle(on_toggle)
}

pub(super) fn action_button<'a, M: Clone + 'a>(
    label: impl IntoFragment<'a>,
    message: Option<M>,
) -> Button<'a, M> {
    button(clipped_text(label, 12.0))
        .padding([6, 10])
        .style(|theme, status| theme::button_style(theme, false, status))
        .on_press_maybe(message)
}

pub(super) fn selectable_button<'a, M: Clone + 'a>(
    label: impl Into<String>,
    selected: bool,
    message: M,
) -> Button<'a, M> {
    button(clipped_text(label.into(), 12.0).width(Fill))
        .padding(theme::CONTROL_GAP)
        .width(Fill)
        .style(move |theme, status| theme::button_style(theme, selected, status))
        .on_press(message)
}
