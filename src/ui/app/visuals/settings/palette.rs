// Color palette editor.

use crate::ui::theme::f32_to_u8;
use crate::ui::theme::{self, Palette};
use iced::alignment::{Horizontal, Vertical};
use iced::widget::text::Wrapping;
use iced::widget::{Button, Column, Row, Space, container, scrollable, slider, text};
use iced::{Background, Color, Element, Length};

const SWATCH_SIZE: (f32, f32) = (56.0, 28.0);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PaletteEvent {
    Open(usize),
    Close,
    Adjust { index: usize, color: Color },
    Reset,
}

#[derive(Debug, Clone)]
pub struct PaletteEditor {
    palette: Palette,
    active: Option<usize>,
    // Optional visibility filter: only show these indices (if set).
    visible_indices: Option<Vec<usize>>,
    // Optional label overrides for specific indices.
    label_overrides: Vec<(usize, &'static str)>,
}

impl PaletteEditor {
    // Creates a new editor from a `Palette` definition.
    pub fn new(palette: Palette) -> Self {
        Self {
            palette,
            active: None,
            visible_indices: None,
            label_overrides: Vec::new(),
        }
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
            PaletteEvent::Reset => {
                self.active = None;
                if self.palette.is_default() {
                    false
                } else {
                    self.palette.reset();
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
        let mut col = Column::new()
            .spacing(12.0)
            .push(scrollable(row).horizontal().width(Length::Fill));
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
