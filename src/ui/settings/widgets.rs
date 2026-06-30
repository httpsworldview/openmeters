// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::ui::widgets::palette_editor::{PaletteEditor, PaletteEvent};
use crate::ui::{clipped_text, theme};
use iced::Element;
use iced::Length::{Fill, Shrink};
use iced::alignment::Vertical;
use iced::widget::{column, container, pick_list, row, slider, toggler};
use std::borrow::Cow;
use std::fmt;

pub const FFT_OPTIONS: [usize; 5] = [1024, 2048, 4096, 8192, 16384];
pub const HOP_DIVISORS: [usize; 7] = [4, 6, 8, 16, 32, 64, 128];
pub struct SliderRange {
    pub min: f32,
    pub max: f32,
    pub step: f32,
}
impl SliderRange {
    pub const fn new(min: f32, max: f32, step: f32) -> Self {
        Self { min, max, step }
    }
    pub fn snap(self, value: f32) -> f32 {
        debug_assert!(self.step > 0.0, "SliderRange::snap expects a positive step");
        if self.step <= 0.0 {
            return value.clamp(self.min, self.max);
        }
        (self.min + ((value - self.min) / self.step).round() * self.step).clamp(self.min, self.max)
    }
}

pub fn set_if_changed<T: PartialEq>(target: &mut T, value: T) -> bool {
    if *target == value {
        return false;
    }
    *target = value;
    true
}

// Compare bits to avoid spurious writes for identical NaN payloads.
pub fn update_f32_range(target: &mut f32, value: f32, range: SliderRange) -> bool {
    let snapped = range.snap(value);
    if target.to_bits() == snapped.to_bits() {
        return false;
    }
    *target = snapped;
    true
}

pub fn update_usize_from_f32(target: &mut usize, value: f32, range: SliderRange) -> bool {
    debug_assert!(
        [range.min, range.max, range.step]
            .into_iter()
            .all(|v| v.fract().abs() <= f32::EPSILON),
        "update_usize_from_f32 expects integral slider bounds"
    );
    set_if_changed(target, range.snap(value).round() as usize)
}

pub fn get_closest_hop_divisor(fft_size: usize, hop_size: usize) -> usize {
    if fft_size == 0 || hop_size == 0 {
        return 8;
    }
    let ratio = fft_size as f32 / hop_size as f32;
    HOP_DIVISORS
        .iter()
        .copied()
        .min_by(|&a, &b| {
            (ratio - a as f32)
                .abs()
                .total_cmp(&(ratio - b as f32).abs())
        })
        .unwrap_or(8)
}

// Preserve the hop:fft ratio when fft_size changes.
pub fn update_fft_size(fft_size: &mut usize, hop_size: &mut usize, new: usize) -> bool {
    let hop_div = get_closest_hop_divisor(*fft_size, *hop_size).max(1);
    if !set_if_changed(fft_size, new) {
        return false;
    }
    *hop_size = (new / hop_div).max(1);
    true
}

pub fn update_hop_divisor(fft_size: usize, hop_size: &mut usize, divisor: usize) -> bool {
    set_if_changed(hop_size, (fft_size / divisor.max(1)).max(1))
}

pub fn slide<'a, M: Clone + 'a>(
    label: &'static str,
    value: f32,
    formatted: impl Into<String>,
    range: SliderRange,
    on_change: impl Fn(f32) -> M + 'a,
) -> iced::widget::Column<'a, M> {
    column![
        row![
            clipped_text(label, 12.0).width(Fill),
            clipped_text(formatted.into(), 11.0)
        ]
        .spacing(6.0)
        .width(Fill),
        slider::Slider::new(range.min..=range.max, value, on_change)
            .step(range.step)
            .style(theme::slider_style),
    ]
    .spacing(8.0)
    .width(Fill)
}

pub fn card<'a, M: 'a>(
    label: &'static str,
    content: impl Into<Element<'a, M>>,
) -> container::Container<'a, M> {
    container(
        column![clipped_text(label, 14.0), content.into()]
            .spacing(10)
            .width(Fill),
    )
    .padding(12)
    .width(Fill)
    .style(theme::weak_container)
}

pub fn split<'a, M: 'a>(
    left: impl Into<Element<'a, M>>,
    right: impl Into<Element<'a, M>>,
) -> iced::widget::Row<'a, M> {
    row![container(left).width(Fill), container(right).width(Fill)]
        .spacing(16)
        .width(Fill)
}

pub fn palette_card<'a, M: 'a>(
    palette: &'a PaletteEditor,
    map: impl Fn(PaletteEvent) -> M + 'a,
) -> container::Container<'a, M> {
    card("Colors", palette.view().map(map))
}

pub fn pick<'a, T, M>(
    label: &'static str,
    options: impl Into<Cow<'a, [T]>>,
    selected: T,
    on_select: impl Fn(T) -> M + 'a,
) -> iced::widget::Row<'a, M>
where
    T: Clone + PartialEq + fmt::Display + 'static,
    M: Clone + 'a,
{
    row![
        clipped_text(label, 12.0).width(Shrink),
        pick_list(options.into(), Some(selected), on_select).width(Fill),
    ]
    .spacing(8.0)
    .align_y(Vertical::Center)
    .width(Fill)
}

pub fn toggle<'a, M: 'a>(
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
