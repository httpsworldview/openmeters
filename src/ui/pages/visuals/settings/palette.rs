// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::widgets::clipped_text;
use crate::ui::theme::{self, Palette};
use crate::ui::widgets::scroll_glow::ScrollGlow;
use crate::util::color::{
    EPSILON, colors_equal, default_spreads, f32_to_u8, find_segment, lerp_color,
    sanitize_stop_positions, sanitize_stop_spreads, with_alpha,
};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Layout, Renderer as _, Widget, layout, mouse};
use iced::alignment::{Horizontal, Vertical};
use iced::widget::{Button, Column, Row, Space, container, slider};
use iced::{Background, Color, Element, Length, Point, Rectangle, Size};

const SWATCH_SIZE: (f32, f32) = (56.0, 28.0);
const GRADIENT_BAR_HEIGHT: f32 = 24.0;
const MARKER_HEIGHT: f32 = 8.0;
const MIN_STOP_GAP: f32 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PaletteEvent {
    Open(usize),
    Close,
    Adjust { index: usize, color: Color },
    AdjustPosition { index: usize, position: f32 },
    AdjustSpread { index: usize, spread: f32 },
    Reset,
    HorizontalScroll(ScrollGlow),
}

#[derive(Debug, Clone)]
pub struct PaletteEditor {
    palette: Palette,
    positions: Vec<f32>,
    spreads: Vec<f32>,
    default_spreads: Vec<f32>,
    active: Option<usize>,
    visible_indices: Option<Vec<usize>>,
    label_overrides: Vec<(usize, &'static str)>,
    show_ramp: bool,
    scroll: ScrollGlow,
}

impl PaletteEditor {
    pub fn new(palette: Palette) -> Self {
        let default_spreads = default_spreads(palette.len());
        Self {
            positions: palette.default_positions.to_vec(),
            spreads: default_spreads.clone(),
            palette,
            default_spreads,
            active: None,
            visible_indices: None,
            label_overrides: Vec::new(),
            show_ramp: false,
            scroll: ScrollGlow::default(),
        }
    }

    pub fn set_show_ramp(&mut self, show: bool) {
        self.show_ramp = show;
    }

    pub fn set_visible_indices(&mut self, indices: Option<Vec<usize>>) {
        self.visible_indices = indices;
        if let (Some(active), Some(visible)) = (self.active, &self.visible_indices)
            && !visible.contains(&active)
        {
            self.active = None;
        }
    }

    pub fn set_label_overrides(&mut self, overrides: Vec<(usize, &'static str)>) {
        self.label_overrides = overrides;
    }

    fn label_for(&self, index: usize) -> String {
        self.label_overrides
            .iter()
            .find_map(|&(i, label)| (i == index).then_some(label))
            .or_else(|| self.palette.labels().get(index).copied())
            .map_or_else(|| format!("Color {}", index + 1), str::to_owned)
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
        self.palette.set(colors);
    }

    pub fn update(&mut self, event: PaletteEvent) -> bool {
        match event {
            PaletteEvent::Open(i) => {
                if i < self.palette.len() {
                    self.active = (self.active != Some(i)).then_some(i);
                }
                false
            }
            PaletteEvent::Close => {
                self.active = None;
                false
            }
            PaletteEvent::Adjust { index, color } => {
                let colors = self.palette.colors();
                if index >= colors.len() || colors_equal(colors[index], color) {
                    return false;
                }
                let mut colors = colors.to_vec();
                colors[index] = color;
                self.palette.set(&colors);
                true
            }
            PaletteEvent::AdjustPosition { index, position } => {
                let n = self.palette.len();
                if n < 3 || index == 0 || index >= n - 1 {
                    return false;
                }
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
                if index >= self.palette.len() {
                    return false;
                }
                let next = spread.clamp(0.2, 5.0);
                if (self.spreads[index] - next).abs() < EPSILON {
                    return false;
                }
                self.spreads[index] = next;
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
                    self.spreads.clone_from(&self.default_spreads);
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
            && self.spreads == self.default_spreads
    }

    pub fn view(&self) -> Element<'_, PaletteEvent> {
        let colors = self.palette.colors();
        let indices: Vec<usize> = self.visible_indices.as_ref().map_or_else(
            || (0..colors.len()).collect(),
            |v| v.iter().copied().filter(|&i| i < colors.len()).collect(),
        );
        let row = indices.iter().fold(Row::new().spacing(12.0), |r, &i| {
            r.push(self.color_picker(i, colors[i]))
        });
        let mut col = Column::new().spacing(12.0);
        if self.show_ramp && colors.len() >= 2 {
            let positions = self.positions();
            let spreads = self.spreads();
            col = col.push(gradient_bar(colors, positions, spreads, self.active));
        }
        col = col.push(self.scroll.horizontal(row, PaletteEvent::HorizontalScroll));
        if let Some(i) = self.active
            && let Some(&c) = colors.get(i)
        {
            col = col.push(self.color_controls(i, c));
        }
        col.push(
            Button::new(clipped_text("Reset to defaults", 12.0))
                .padding([6, 10])
                .style(|t, s| theme::tab_button_style(t, false, s))
                .on_press_maybe((!self.is_default()).then_some(PaletteEvent::Reset)),
        )
        .into()
    }

    fn color_picker(&self, i: usize, c: Color) -> Element<'_, PaletteEvent> {
        let (w, h) = SWATCH_SIZE;
        let active = self.active == Some(i);
        Button::new(
            Column::new()
                .width(Length::Shrink)
                .spacing(4.0)
                .align_x(Horizontal::Center)
                .push(clipped_text(self.label_for(i), 11.0))
                .push(
                    container(Space::new().width(Length::Fill).height(Length::Fill))
                        .width(Length::Fixed(w))
                        .height(Length::Fixed(h))
                        .style(move |_| swatch_style(c, active)),
                )
                .push(clipped_text(to_hex(c), 11.0)),
        )
        .padding([6, 8])
        .style(|t, s| theme::tab_button_style(t, false, s))
        .on_press(PaletteEvent::Open(i))
        .into()
    }

    fn color_controls(&self, i: usize, c: Color) -> Element<'_, PaletteEvent> {
        let header = Row::new()
            .spacing(8.0)
            .align_y(Vertical::Center)
            .push(clipped_text(self.label_for(i), 12.0))
            .push(Space::new().width(Length::Fill).height(Length::Shrink))
            .push(
                Button::new(clipped_text("Done", 12.0))
                    .padding([6, 10])
                    .style(|t, s| theme::tab_button_style(t, false, s))
                    .on_press(PaletteEvent::Close),
            );

        let col = [("R", c.r, 0u8), ("G", c.g, 1), ("B", c.b, 2), ("A", c.a, 3)]
            .into_iter()
            .fold(
                Column::new().spacing(8.0).push(header),
                |col, (lbl, val, ch)| col.push(channel_slider(lbl, val, ch, i, c)),
            );
        container(col)
            .padding(12)
            .style(theme::weak_container)
            .into()
    }
}

fn swatch_style(color: Color, active: bool) -> container::Style {
    let a = color.a;
    let d = Color {
        r: color.r * a,
        g: color.g * a,
        b: color.b * a,
        a,
    };
    container::Style::default()
        .background(Background::Color(d))
        .border(if active {
            theme::focus_border()
        } else {
            theme::sharp_border()
        })
}

fn to_hex(c: Color) -> String {
    let [r, g, b, a] = [c.r, c.g, c.b, c.a].map(f32_to_u8);
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

#[derive(Debug, Default)]
struct GradientBarState {
    dragging: Option<usize>,
}

struct GradientBar<'a> {
    colors: &'a [Color],
    positions: &'a [f32],
    spreads: &'a [f32],
    active: Option<usize>,
}

fn gradient_bar<'a>(
    colors: &'a [Color],
    positions: &'a [f32],
    spreads: &'a [f32],
    active: Option<usize>,
) -> Element<'a, PaletteEvent> {
    Element::new(GradientBar {
        colors,
        positions,
        spreads,
        active,
    })
}

impl Widget<PaletteEvent, iced::Theme, iced::Renderer> for GradientBar<'_> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<GradientBarState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(GradientBarState::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(TOTAL_HEIGHT))
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(Length::Fill, Length::Fixed(TOTAL_HEIGHT), Size::ZERO))
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &iced::Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn iced::advanced::Clipboard,
        shell: &mut iced::advanced::Shell<'_, PaletteEvent>,
        _viewport: &Rectangle,
    ) {
        let n = self.positions.len();
        if n < 2 {
            return;
        }
        let st = tree.state.downcast_mut::<GradientBarState>();
        let bounds = layout.bounds();
        let iced::Event::Mouse(mouse_event) = event else {
            return;
        };
        match mouse_event {
            mouse::Event::ButtonPressed(mouse::Button::Left) => {
                if n >= 3
                    && let Some(pos) = cursor.position().filter(|p| bounds.contains(*p))
                    && let Some(i) = nearest_handle(1..n - 1, self.positions, &bounds, pos.x)
                {
                    st.dragging = Some(i);
                    shell.capture_event();
                }
            }
            mouse::Event::CursorMoved { position } => {
                if let Some(i) = st.dragging {
                    let t = ((position.x - bounds.x) / bounds.width).clamp(0.0, 1.0);
                    shell.publish(PaletteEvent::AdjustPosition {
                        index: i,
                        position: t,
                    });
                    shell.capture_event();
                }
            }
            mouse::Event::ButtonReleased(mouse::Button::Left) if st.dragging.take().is_some() => {
                shell.capture_event();
            }
            mouse::Event::WheelScrolled { delta } => {
                if let Some(pos) = cursor.position().filter(|p| bounds.contains(*p))
                    && let Some(i) = nearest_handle(0..n, self.positions, &bounds, pos.x)
                {
                    let dy = scroll_delta(*delta);
                    let current = self.spreads.get(i).copied().unwrap_or(1.0);
                    let new_spread = (current + dy * 0.2).clamp(0.2, 5.0);
                    shell.publish(PaletteEvent::AdjustSpread {
                        index: i,
                        spread: new_spread,
                    });
                    shell.capture_event();
                }
            }
            _ => {}
        }
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        if self.colors.len() < 2 || self.positions.len() != self.colors.len() {
            return;
        }
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
        let stop_count = self.colors.len();
        for i in 0..steps {
            let t = i as f32 / (steps - 1).max(1) as f32;
            let (lo, hi, f) = find_segment(self.positions, self.spreads, t, stop_count);
            let c = lerp_color(self.colors[lo], self.colors[hi], f);
            let premul = Color {
                r: c.r * c.a,
                g: c.g * c.a,
                b: c.b * c.a,
                a: 1.0,
            };
            paint(
                Rectangle::new(
                    Point::new(bounds.x + i as f32 * step_w, bounds.y),
                    Size::new(step_w + 0.5, GRADIENT_BAR_HEIGHT),
                ),
                Default::default(),
                Background::Color(premul),
            );
        }
        paint(
            Rectangle::new(bounds.position(), Size::new(bar_w, GRADIENT_BAR_HEIGHT)),
            theme::sharp_border(),
            Background::Color(Color::TRANSPARENT),
        );

        let handle_y = bounds.y + GRADIENT_BAR_HEIGHT + 1.0;
        for (i, &pos) in self.positions.iter().enumerate() {
            let x = bounds.x + pos.clamp(0.0, 1.0) * bar_w;
            let c = self.colors.get(i).copied().unwrap_or(Color::WHITE);
            let active = self.active == Some(i);
            let line_alpha = if active { 1.0 } else { 0.5 };
            paint(
                Rectangle::new(
                    Point::new(x - INDICATOR_WIDTH * 0.5, bounds.y),
                    Size::new(INDICATOR_WIDTH, GRADIENT_BAR_HEIGHT),
                ),
                Default::default(),
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
            let border = if active {
                theme::focus_border()
            } else {
                theme::sharp_border()
            };
            paint(
                Rectangle::new(
                    Point::new(x - hw * 0.5, handle_y),
                    Size::new(hw, MARKER_HEIGHT - 1.0),
                ),
                border,
                Background::Color(fill),
            );
        }
    }
}

#[inline]
fn scroll_delta(delta: mouse::ScrollDelta) -> f32 {
    match delta {
        mouse::ScrollDelta::Lines { y, .. } => y,
        mouse::ScrollDelta::Pixels { y, .. } => y / 50.0,
    }
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
        format!("{:>3}", f32_to_u8(val))
    };
    Row::new()
        .spacing(8.0)
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
            .step(0.01)
            .style(theme::slider_style)
            .width(Length::Fill),
        )
        .push(clipped_text(display, 12.0))
}
