// Color palette editor with optional gradient ramp for magnitude-mapped visuals.

use crate::ui::theme::f32_to_u8;
use crate::ui::theme::{self, Palette};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Layout, Renderer as _, Widget, layout, mouse};
use iced::alignment::{Horizontal, Vertical};
use iced::widget::text::Wrapping;
use iced::widget::{Button, Column, Row, Space, container, scrollable, slider, text};
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
}

#[derive(Debug, Clone)]
pub struct PaletteEditor {
    palette: Palette,
    positions: Vec<f32>,
    spreads: Vec<f32>,
    default_positions: Vec<f32>,
    default_spreads: Vec<f32>,
    active: Option<usize>,
    visible_indices: Option<Vec<usize>>,
    label_overrides: Vec<(usize, &'static str)>,
    show_ramp: bool,
}

impl PaletteEditor {
    pub fn new(palette: Palette) -> Self {
        let count = palette.len();
        let default_positions = theme::uniform_positions(count);
        let default_spreads = theme::default_spreads(count);
        Self {
            palette,
            positions: default_positions.clone(),
            spreads: default_spreads.clone(),
            default_positions,
            default_spreads,
            active: None,
            visible_indices: None,
            label_overrides: Vec::new(),
            show_ramp: false,
        }
    }

    pub fn set_show_ramp(&mut self, show: bool) {
        self.show_ramp = show;
    }

    pub fn set_visible_indices(&mut self, indices: Option<Vec<usize>>) {
        self.visible_indices = indices;
        if let Some(active) = self.active
            && let Some(ref visible) = self.visible_indices
            && !visible.contains(&active)
        {
            self.active = None;
        }
    }

    // Sets label overrides for specific indices.
    pub fn set_label_overrides(&mut self, overrides: Vec<(usize, &'static str)>) {
        self.label_overrides = overrides;
    }

    fn label_for(&self, index: usize) -> String {
        if let Some((_, label)) = self.label_overrides.iter().find(|(i, _)| *i == index) {
            return (*label).to_string();
        }
        self.palette
            .labels()
            .get(index)
            .map_or_else(|| format!("Color {}", index + 1), |s| (*s).to_string())
    }

    pub fn positions(&self) -> &[f32] {
        &self.positions
    }

    pub fn spreads(&self) -> &[f32] {
        &self.spreads
    }

    pub fn set_positions(&mut self, positions: Option<&[f32]>) {
        self.positions = theme::sanitize_stop_positions(positions, self.palette.len());
    }

    pub fn set_spreads(&mut self, spreads: Option<&[f32]>) {
        self.spreads = theme::sanitize_stop_spreads(spreads, self.palette.len());
    }

    pub fn set_colors(&mut self, colors: &[Color]) {
        self.palette.set(colors);
    }

    pub fn update(&mut self, event: PaletteEvent) -> bool {
        match event {
            PaletteEvent::Open(i) if i < self.palette.len() => {
                self.active = (self.active != Some(i)).then_some(i);
                false
            }
            PaletteEvent::Close => {
                self.active = None;
                false
            }
            PaletteEvent::Adjust { index, color } => {
                let colors = self.palette.colors();
                if index < colors.len() && !theme::colors_equal(colors[index], color) {
                    let mut c = colors.to_vec();
                    c[index] = color;
                    self.palette.set(&c);
                    true
                } else {
                    false
                }
            }
            PaletteEvent::AdjustPosition { index, position } => {
                let n = self.palette.len();
                if index == 0 || index >= n - 1 || n < 3 {
                    return false;
                }
                let lo = (self.positions[index - 1] + MIN_STOP_GAP).max(MIN_STOP_GAP);
                let hi = (self.positions[index + 1] - MIN_STOP_GAP).min(1.0 - MIN_STOP_GAP);
                if lo > hi {
                    return false;
                }
                let next = position.clamp(lo, hi);
                if (self.positions[index] - next).abs() < 1e-4 {
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
                if (self.spreads[index] - next).abs() < 1e-4 {
                    return false;
                }
                self.spreads[index] = next;
                true
            }
            PaletteEvent::Reset => {
                self.active = None;
                if self.is_default() {
                    false
                } else {
                    self.palette.reset();
                    self.positions.clone_from(&self.default_positions);
                    self.spreads.clone_from(&self.default_spreads);
                    true
                }
            }
            _ => false,
        }
    }

    pub fn colors(&self) -> &[Color] {
        self.palette.colors()
    }

    pub fn is_default(&self) -> bool {
        self.palette.is_default()
            && self.positions == self.default_positions
            && self.spreads == self.default_spreads
    }

    pub fn view(&self) -> Element<'_, PaletteEvent> {
        let colors = self.palette.colors();
        let indices: Vec<usize> = self
            .visible_indices
            .as_ref()
            .map(|v| v.iter().copied().filter(|&i| i < colors.len()).collect())
            .unwrap_or_else(|| (0..colors.len()).collect());
        let row = indices.iter().fold(Row::new().spacing(12.0), |r, &i| {
            r.push(self.color_picker(i, colors[i]))
        });
        let mut col = Column::new().spacing(12.0);
        if self.show_ramp && colors.len() >= 2 {
            let positions = self.positions();
            let spreads = self.spreads();
            col = col.push(gradient_bar(colors, positions, spreads, self.active));
        }
        col = col.push(scrollable(row).horizontal().width(Length::Fill));
        if let Some(i) = self.active
            && let Some(&c) = colors.get(i)
        {
            col = col.push(self.color_controls(i, c));
        }
        col.push(
            Button::new(
                container(text("Reset to defaults").size(12).wrapping(Wrapping::None)).clip(true),
            )
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
                .push(
                    container(text(self.label_for(i)).size(11).wrapping(Wrapping::None)).clip(true),
                )
                .push(
                    container(Space::new().width(Length::Fill).height(Length::Fill))
                        .width(Length::Fixed(w))
                        .height(Length::Fixed(h))
                        .style(move |_| swatch_style(c, active)),
                )
                .push(container(text(to_hex(c)).size(11).wrapping(Wrapping::None)).clip(true)),
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
            .push(container(text(self.label_for(i)).size(12).wrapping(Wrapping::None)).clip(true))
            .push(Space::new().width(Length::Fill).height(Length::Shrink))
            .push(
                Button::new(container(text("Done").size(12).wrapping(Wrapping::None)).clip(true))
                    .padding([6, 10])
                    .style(|t, s| theme::tab_button_style(t, false, s))
                    .on_press(PaletteEvent::Close),
            );

        let channels = [("R", c.r, 0u8), ("G", c.g, 1), ("B", c.b, 2), ("A", c.a, 3)];
        let col = channels.into_iter().fold(
            Column::new().spacing(8.0).push(header),
            |col, (lbl, val, ch)| {
                let display = if ch == 3 {
                    format!("{:>3}%", (val.clamp(0.0, 1.0) * 100.0).round() as u8)
                } else {
                    format!("{:>3}", f32_to_u8(val))
                };
                col.push(
                    Row::new()
                        .spacing(8.0)
                        .align_y(Vertical::Center)
                        .push(
                            container(text(lbl).size(12).wrapping(Wrapping::None))
                                .width(Length::Fixed(32.0))
                                .clip(true),
                        )
                        .push(
                            slider::Slider::new(0.0..=1.0, val, move |nv| {
                                let nv = if ch == 3 && nv < 0.005 { 0.0 } else { nv };
                                let mut nc = c;
                                match ch {
                                    0 => nc.r = nv,
                                    1 => nc.g = nv,
                                    2 => nc.b = nv,
                                    _ => nc.a = nv,
                                }
                                PaletteEvent::Adjust {
                                    index: i,
                                    color: nc,
                                }
                            })
                            .step(0.01)
                            .style(theme::slider_style)
                            .width(Length::Fill),
                        )
                        .push(
                            container(text(display).size(12).wrapping(Wrapping::None)).clip(true),
                        ),
                )
            },
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
    let border = if active {
        theme::focus_border()
    } else {
        theme::sharp_border()
    };
    container::Style::default()
        .background(Background::Color(d))
        .border(border)
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
        .map(|i| {
            (
                i,
                (cursor_x - (bounds.x + positions[i] * bounds.width)).abs(),
            )
        })
        .filter(|(_, d)| *d <= HANDLE_HIT_RADIUS)
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
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
        match event {
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if n >= 3
                    && let Some(pos) = cursor.position().filter(|p| bounds.contains(*p))
                    && let Some(i) = nearest_handle(1..n - 1, self.positions, &bounds, pos.x)
                {
                    st.dragging = Some(i);
                    shell.capture_event();
                }
            }
            iced::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some(i) = st.dragging {
                    let t = ((position.x - bounds.x) / bounds.width).clamp(0.0, 1.0);
                    shell.publish(PaletteEvent::AdjustPosition {
                        index: i,
                        position: t,
                    });
                    shell.capture_event();
                }
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if st.dragging.take().is_some() {
                    shell.capture_event();
                }
            }
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
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
        for i in 0..steps {
            let t = i as f32 / (steps - 1).max(1) as f32;
            let c = theme::sample_gradient_positioned(self.colors, self.positions, self.spreads, t);
            let display = Color {
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
                Background::Color(display),
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
            let is_active = self.active == Some(i);
            let line_color = if is_active {
                Color::WHITE
            } else {
                Color {
                    a: 0.5,
                    ..Color::WHITE
                }
            };
            paint(
                Rectangle::new(
                    Point::new(x - INDICATOR_WIDTH * 0.5, bounds.y),
                    Size::new(INDICATOR_WIDTH, GRADIENT_BAR_HEIGHT),
                ),
                Default::default(),
                Background::Color(line_color),
            );
            let hw = if is_active {
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
                if is_active {
                    theme::focus_border()
                } else {
                    theme::sharp_border()
                },
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
