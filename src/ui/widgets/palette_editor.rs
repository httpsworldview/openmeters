// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::ui::scroll_delta_lines;
use crate::ui::theme::{self as ui_theme, Palette};
use crate::ui::widgets::scroll_glow::ScrollGlow;
use crate::ui::widgets::{action_button, clipped_text};
use crate::util::color::{
    EPSILON, STOP_SPREAD_MAX, STOP_SPREAD_MIN, colors_equal, lerp_color, sanitize_stop_positions,
    sanitize_stop_spreads, with_alpha,
};
use iced::advanced::renderer::Quad;
use iced::advanced::{Renderer as _, Widget, mouse};
use iced::alignment::{Horizontal, Vertical};
use iced::widget::{Button, Column, Row, Space, container, slider};
use iced::{Background, Color, Element, Length, Point, Rectangle, Size};

const SWATCH_SIZE: (f32, f32) = (56.0, 28.0);
const GRADIENT_BAR_HEIGHT: f32 = 24.0;
const MARKER_HEIGHT: f32 = 8.0;
const MIN_STOP_GAP: f32 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PaletteEvent {
    Select(Option<usize>),
    Adjust { index: usize, color: Color },
    AdjustPosition { index: usize, position: f32 },
    AdjustSpread { index: usize, spread: f32 },
    Reset,
    HorizontalScroll(ScrollGlow),
}

pub struct PaletteEditor {
    palette: Palette,
    positions: Vec<f32>,
    spreads: Vec<f32>,
    active: Option<usize>,
    only_first_visible: bool,
    label_overrides: &'static [(usize, &'static str)],
    show_ramp: bool,
    scroll: ScrollGlow,
}

impl PaletteEditor {
    pub fn new(palette: Palette) -> Self {
        Self {
            positions: palette.default_positions.to_vec(),
            spreads: vec![1.0; palette.len()],
            palette,
            active: None,
            only_first_visible: false,
            label_overrides: &[],
            show_ramp: false,
            scroll: ScrollGlow::default(),
        }
    }

    pub fn set_show_ramp(&mut self, show: bool) {
        self.show_ramp = show;
    }

    pub fn set_only_first_visible(&mut self, only: bool) {
        self.only_first_visible = only;
        let _ = self.active.take_if(|active| only && *active != 0);
    }

    pub fn set_label_overrides(&mut self, overrides: &'static [(usize, &'static str)]) {
        self.label_overrides = overrides;
    }

    fn label_for(&self, index: usize) -> String {
        self.label_overrides
            .iter()
            .find_map(|&(i, label)| (i == index).then_some(label))
            .unwrap_or(self.palette.labels()[index])
            .to_owned()
    }

    pub fn positions(&self) -> &[f32] {
        &self.positions
    }

    pub fn spreads(&self) -> &[f32] {
        &self.spreads
    }

    pub fn set_positions(&mut self, positions: Option<&[f32]>) {
        self.positions = sanitize_stop_positions(positions, self.palette.default_positions);
    }

    pub fn default_positions(&self) -> &'static [f32] {
        self.palette.default_positions
    }

    pub fn defaults(&self) -> &'static [Color] {
        self.palette.defaults
    }

    pub fn set_spreads(&mut self, spreads: Option<&[f32]>) {
        self.spreads = sanitize_stop_spreads(spreads, self.palette.len());
    }

    pub fn set_colors(&mut self, colors: &[Color]) {
        self.palette.set_colors(colors);
    }

    pub fn update(&mut self, event: PaletteEvent) -> bool {
        match event {
            PaletteEvent::Select(index) => {
                self.active = index;
                false
            }
            PaletteEvent::Adjust { index, color } => {
                let colors = self.palette.colors();
                if colors_equal(colors[index], color) {
                    return false;
                }
                let mut colors = colors.to_vec();
                colors[index] = color;
                self.palette.set_colors(&colors);
                true
            }
            PaletteEvent::AdjustPosition { index, position } => {
                let lo = (self.positions[index - 1] + MIN_STOP_GAP).max(MIN_STOP_GAP);
                let hi = (self.positions[index + 1] - MIN_STOP_GAP).min(1.0 - MIN_STOP_GAP);
                if lo > hi {
                    return false;
                }
                let next = position.clamp(lo, hi);
                if (self.positions[index] - next).abs() < EPSILON {
                    return false;
                }
                self.positions[index] = next;
                true
            }
            PaletteEvent::AdjustSpread { index, spread } => {
                if (self.spreads[index] - spread).abs() < EPSILON {
                    return false;
                }
                self.spreads[index] = spread;
                true
            }
            PaletteEvent::HorizontalScroll(g) => {
                self.scroll = g;
                false
            }
            PaletteEvent::Reset => {
                self.active = None;
                if self.is_default() {
                    false
                } else {
                    self.palette.reset();
                    self.positions = self.palette.default_positions.to_vec();
                    self.spreads = vec![1.0; self.palette.len()];
                    true
                }
            }
        }
    }

    pub fn colors(&self) -> &[Color] {
        self.palette.colors()
    }

    pub fn is_default(&self) -> bool {
        self.palette.is_default()
            && self.positions == self.palette.default_positions
            && self.spreads.iter().all(|&spread| spread == 1.0)
    }

    pub fn view(&self) -> Element<'_, PaletteEvent> {
        let colors = self.palette.colors();
        let mut row = Row::new().spacing(12);
        let visible = if self.only_first_visible {
            1
        } else {
            colors.len()
        };
        for (i, &color) in colors.iter().take(visible).enumerate() {
            row = row.push(self.color_picker(i, color));
        }
        let mut col = Column::new().spacing(12);
        if self.show_ramp {
            col = col.push(Element::new(self));
        }
        col = col.push(self.scroll.horizontal(row, PaletteEvent::HorizontalScroll));
        if let Some(i) = self.active {
            col = col.push(self.color_controls(i, colors[i]));
        }
        col.push(action_button(
            "Reset to defaults",
            (!self.is_default()).then_some(PaletteEvent::Reset),
        ))
        .into()
    }

    fn color_picker(&self, i: usize, c: Color) -> Element<'_, PaletteEvent> {
        let (w, h) = SWATCH_SIZE;
        let active = self.active == Some(i);
        Button::new(
            Column::new()
                .width(Length::Shrink)
                .spacing(4)
                .align_x(Horizontal::Center)
                .push(clipped_text(self.label_for(i), 11.0))
                .push(
                    container(Space::new().width(Length::Fill).height(Length::Fill))
                        .width(Length::Fixed(w))
                        .height(Length::Fixed(h))
                        .style(move |theme| {
                            container::Style::default()
                                .background(Background::Color(c))
                                .border(ui_theme::border(theme, active))
                        }),
                )
                .push(clipped_text(to_hex(c), 11.0)),
        )
        .padding([6, 8])
        .style(|theme, status| ui_theme::button_style(theme, false, status))
        .on_press(PaletteEvent::Select((!active).then_some(i)))
        .into()
    }

    fn color_controls(&self, i: usize, c: Color) -> Element<'_, PaletteEvent> {
        let header = Row::new()
            .spacing(8)
            .align_y(Vertical::Center)
            .push(clipped_text(self.label_for(i), 12.0))
            .push(Space::new().width(Length::Fill).height(Length::Shrink))
            .push(action_button("Done", Some(PaletteEvent::Select(None))));

        let col = [("R", c.r, 0u8), ("G", c.g, 1), ("B", c.b, 2), ("A", c.a, 3)]
            .into_iter()
            .fold(
                Column::new().spacing(8).push(header),
                |col, (lbl, val, ch)| col.push(channel_slider(lbl, val, ch, i, c)),
            );
        container(col)
            .padding(12)
            .style(ui_theme::weak_container)
            .into()
    }
}

fn to_hex(c: Color) -> String {
    let [r, g, b, a] = c.into_rgba8();
    if a == 255 {
        format!("#{r:02X}{g:02X}{b:02X}")
    } else {
        format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
    }
}

const HANDLE_WIDTH: f32 = 10.0;
const HANDLE_HIT_SLOP: f32 = 6.0;
const INDICATOR_WIDTH: f32 = 1.0;
const TOTAL_HEIGHT: f32 = GRADIENT_BAR_HEIGHT + MARKER_HEIGHT;
const HANDLE_HIT_RADIUS: f32 = (HANDLE_WIDTH + HANDLE_HIT_SLOP) * 0.5;

fn nearest_handle(
    range: std::ops::Range<usize>,
    positions: &[f32],
    bounds: &Rectangle,
    cursor_x: f32,
) -> Option<usize> {
    range
        .filter_map(|i| {
            let d = (cursor_x - (bounds.x + positions[i] * bounds.width)).abs();
            (d <= HANDLE_HIT_RADIUS).then_some((i, d))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

fn find_segment(positions: &[f32], spreads: &[f32], t: f32) -> (usize, usize, f32) {
    let count = positions.len();
    let t = t.clamp(0.0, 1.0);
    let hi = positions
        .partition_point(|&pos| pos < t)
        .clamp(1, count - 1);
    let lo = hi - 1;
    let linear =
        ((t - positions[lo]) / (positions[hi] - positions[lo]).max(f32::EPSILON)).clamp(0.0, 1.0);
    let sl = spreads[lo];
    let sr = spreads[hi];
    let f = if (sl - 1.0).abs() < EPSILON && (sr - 1.0).abs() < EPSILON {
        linear
    } else {
        linear.powf(sl / sr).clamp(0.0, 1.0)
    };
    (lo, hi, f)
}

impl Widget<PaletteEvent, iced::Theme, iced::Renderer> for &PaletteEditor {
    crate::macros::widget_method!(state Option<usize>);

    crate::macros::widget_method!(layout
        Size::new(Length::Fill, Length::Fixed(TOTAL_HEIGHT)),
        |limits| limits.resolve(Length::Fill, Length::Fixed(TOTAL_HEIGHT), Size::ZERO)
    );

    crate::macros::widget_method!(update PaletteEvent; this; tree, event, layout, cursor, _, _, shell, _ => {
        let editor = *this;
        let n = editor.positions.len();
        let dragging = tree.state.downcast_mut::<Option<usize>>();
        let bounds = layout.bounds();
        let iced::Event::Mouse(mouse_event) = event else {
            return;
        };
        match mouse_event {
            mouse::Event::ButtonPressed(mouse::Button::Left) => {
                if let Some(pos) = cursor.position().filter(|p| bounds.contains(*p))
                    && let Some(i) = nearest_handle(1..n - 1, &editor.positions, &bounds, pos.x)
                {
                    *dragging = Some(i);
                    shell.capture_event();
                }
            }
            mouse::Event::CursorMoved { position } => {
                if let Some(i) = *dragging {
                    let t = ((position.x - bounds.x) / bounds.width).clamp(0.0, 1.0);
                    shell.publish(PaletteEvent::AdjustPosition {
                        index: i,
                        position: t,
                    });
                    shell.capture_event();
                }
            }
            mouse::Event::ButtonReleased(mouse::Button::Left) if dragging.take().is_some() => {
                shell.capture_event();
            }
            mouse::Event::WheelScrolled { delta } => {
                if let Some(pos) = cursor.position().filter(|p| bounds.contains(*p))
                    && let Some(i) = nearest_handle(0..n, &editor.positions, &bounds, pos.x)
                {
                    let dy = scroll_delta_lines(*delta);
                    let current = editor.spreads[i];
                    let new_spread = (current + dy * 0.2).clamp(STOP_SPREAD_MIN, STOP_SPREAD_MAX);
                    if (current - new_spread).abs() >= EPSILON {
                        shell.publish(PaletteEvent::AdjustSpread {
                            index: i,
                            spread: new_spread,
                        });
                    }
                    shell.capture_event();
                }
            }
            _ => {}
        }
    });

    crate::macros::widget_method!(draw this; _, renderer, theme, _, layout, _, _ => {
        let editor = *this;
        let colors = editor.palette.colors();
        let bounds = layout.bounds();
        let bar_w = bounds.width;
        let mut paint = |bounds: Rectangle, border, bg| {
            renderer.fill_quad(
                Quad {
                    bounds,
                    border,
                    ..Default::default()
                },
                bg,
            );
        };

        let steps = (bar_w as usize).clamp(1, 512);
        let step_w = bar_w / steps as f32;
        for i in 0..steps {
            let t = i as f32 / (steps - 1).max(1) as f32;
            let (lo, hi, f) = find_segment(&editor.positions, &editor.spreads, t);
            let c = lerp_color(colors[lo], colors[hi], f);
            let x = bounds.x + i as f32 * step_w;
            paint(
                Rectangle::new(
                    Point::new(x, bounds.y),
                    Size::new(
                        if i + 1 == steps {
                            bounds.x + bar_w - x
                        } else {
                            step_w
                        },
                        GRADIENT_BAR_HEIGHT,
                    ),
                ),
                iced::Border::default(),
                Background::Color(c),
            );
        }
        paint(
            Rectangle::new(bounds.position(), Size::new(bar_w, GRADIENT_BAR_HEIGHT)),
            ui_theme::border(theme, false),
            Background::Color(Color::TRANSPARENT),
        );

        let handle_y = bounds.y + GRADIENT_BAR_HEIGHT + 1.0;
        for (i, &pos) in editor.positions.iter().enumerate() {
            let x = bounds.x + pos.clamp(0.0, 1.0) * bar_w;
            let c = colors[i];
            let active = editor.active == Some(i);
            let line_alpha = if active { 1.0 } else { 0.5 };
            paint(
                Rectangle::new(
                    Point::new(x - INDICATOR_WIDTH * 0.5, bounds.y),
                    Size::new(INDICATOR_WIDTH, GRADIENT_BAR_HEIGHT),
                ),
                iced::Border::default(),
                Background::Color(with_alpha(Color::WHITE, line_alpha)),
            );
            let hw = if active {
                HANDLE_WIDTH
            } else {
                HANDLE_WIDTH - 2.0
            };
            let fill = Color {
                r: c.r.max(0.12),
                g: c.g.max(0.12),
                b: c.b.max(0.12),
                a: 1.0,
            };
            paint(
                Rectangle::new(
                    Point::new(x - hw * 0.5, handle_y),
                    Size::new(hw, MARKER_HEIGHT - 1.0),
                ),
                ui_theme::border(theme, active),
                Background::Color(fill),
            );
        }
    });
}

fn channel_slider<'a>(
    lbl: &'a str,
    val: f32,
    ch: u8,
    index: usize,
    base: Color,
) -> Row<'a, PaletteEvent> {
    let display = if ch == 3 {
        format!("{:>3}%", (val.clamp(0.0, 1.0) * 100.0).round() as u8)
    } else {
        format!("{:>3}", (val.clamp(0.0, 1.0) * 255.0).round() as u8)
    };
    Row::new()
        .spacing(8)
        .align_y(Vertical::Center)
        .push(clipped_text(lbl, 12.0).width(Length::Fixed(32.0)))
        .push(
            slider::Slider::new(0.0..=1.0, val, move |nv| {
                let nv = if ch == 3 && nv < 0.005 { 0.0 } else { nv };
                let mut nc = base;
                match ch {
                    0 => nc.r = nv,
                    1 => nc.g = nv,
                    2 => nc.b = nv,
                    _ => nc.a = nv,
                }
                PaletteEvent::Adjust { index, color: nc }
            })
            .step(0.01f32)
            .style(ui_theme::slider_style)
            .width(Length::Fill),
        )
        .push(clipped_text(display, 12.0))
}
