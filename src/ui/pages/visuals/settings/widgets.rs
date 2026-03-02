// Shared widgets and utilities for the settings panes

use crate::ui::theme;
use iced::Length;
use iced::alignment::Vertical;
use iced::widget::text::Wrapping;
use iced::widget::{column, container, pick_list, row, slider, text, toggler};
use std::borrow::Cow;
use std::fmt;

pub struct SliderRange {
    pub min: f32,
    pub max: f32,
    pub step: f32,
}
impl SliderRange {
    pub const fn new(min: f32, max: f32, step: f32) -> Self {
        Self { min, max, step }
    }
    #[inline]
    pub fn snap(self, value: f32) -> f32 {
        debug_assert!(self.step > 0.0, "SliderRange::snap expects a positive step");
        if self.step <= 0.0 {
            return value.clamp(self.min, self.max);
        }
        (self.min + ((value - self.min) / self.step).round() * self.step).clamp(self.min, self.max)
    }
}

#[inline]
pub fn set_if_changed<T: PartialEq>(target: &mut T, value: T) -> bool {
    if *target != value {
        *target = value;
        true
    } else {
        false
    }
}

#[inline]
pub fn set_f32(target: &mut f32, value: f32) -> bool {
    if (*target).to_bits() != value.to_bits() {
        *target = value;
        true
    } else {
        false
    }
}

#[inline]
pub fn update_f32_range(target: &mut f32, value: f32, range: SliderRange) -> bool {
    set_f32(target, range.snap(value))
}

#[inline]
pub fn update_usize_from_f32(target: &mut usize, value: f32, range: SliderRange) -> bool {
    debug_assert!(
        [range.min, range.max, range.step]
            .into_iter()
            .all(|v| v.fract().abs() <= f32::EPSILON),
        "update_usize_from_f32 expects integral slider bounds"
    );
    set_if_changed(target, range.snap(value).round() as usize)
}

pub fn labeled_slider<'a, M: Clone + 'a>(
    label: &'static str,
    value: f32,
    formatted: String,
    range: SliderRange,
    on_change: impl Fn(f32) -> M + 'a,
) -> iced::widget::Column<'a, M> {
    column![
        row![
            container(text(label).size(12).wrapping(Wrapping::None)).clip(true),
            container(text(formatted).size(11).wrapping(Wrapping::None)).clip(true),
        ]
        .spacing(6.0),
        slider::Slider::new(range.min..=range.max, value, on_change)
            .step(range.step)
            .style(theme::slider_style),
    ]
    .spacing(8.0)
}

pub fn labeled_pick_list<'a, T, M>(
    label: &'static str,
    options: impl Into<Cow<'a, [T]>>,
    selected: Option<T>,
    on_select: impl Fn(T) -> M + 'a,
) -> iced::widget::Row<'a, M>
where
    T: Clone + PartialEq + fmt::Display + 'static,
    M: Clone + 'a,
{
    row![
        container(text(label).size(12).wrapping(Wrapping::None))
            .width(Length::Shrink)
            .clip(true),
        pick_list(options.into(), selected, on_select),
    ]
    .spacing(8.0)
    .align_y(Vertical::Center)
}

pub fn labeled_toggler<'a, M: 'a>(
    label: &'static str,
    value: bool,
    on_toggle: impl Fn(bool) -> M + 'a,
) -> iced::widget::Toggler<'a, M> {
    toggler(value)
        .label(label)
        .spacing(4)
        .text_size(11)
        .on_toggle(on_toggle)
}

pub fn section_title<'a, M: 'a>(label: &'static str) -> container::Container<'a, M> {
    container(text(label).size(14).wrapping(Wrapping::None)).clip(true)
}
