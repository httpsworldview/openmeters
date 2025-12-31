//! Color palette editor.

use crate::ui::theme;
use iced::alignment::{Horizontal, Vertical};
use iced::widget::text::Wrapping;
use iced::widget::{Button, Column, Row, Space, container, slider, text};
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
    colors: Vec<Color>,
    defaults: Vec<Color>,
    labels: Vec<&'static str>,
    active: Option<usize>,
}

impl PaletteEditor {
    pub fn new(current: &[Color], defaults: &[Color]) -> Self {
        Self::with_labels(current, defaults, &[])
    }

    pub fn with_labels(current: &[Color], defaults: &[Color], labels: &[&'static str]) -> Self {
        Self {
            colors: if current.len() == defaults.len() {
                current.to_vec()
            } else {
                defaults.to_vec()
            },
            defaults: defaults.to_vec(),
            labels: labels.to_vec(),
            active: None,
        }
    }

    fn label_for(&self, index: usize) -> String {
        self.labels
            .get(index)
            .map_or_else(|| format!("Color {}", index + 1), |s| (*s).to_string())
    }

    pub fn update(&mut self, event: PaletteEvent) -> bool {
        match event {
            PaletteEvent::Open(i) if i < self.colors.len() => {
                self.active = (self.active != Some(i)).then_some(i);
                false
            }
            PaletteEvent::Close => {
                self.active = None;
                false
            }
            PaletteEvent::Adjust { index, color } => self.colors.get_mut(index).is_some_and(|s| {
                if theme::colors_equal(*s, color) {
                    false
                } else {
                    *s = color;
                    true
                }
            }),
            PaletteEvent::Reset => {
                self.active = None;
                if self.is_default() {
                    false
                } else {
                    self.colors.clone_from(&self.defaults);
                    true
                }
            }
            _ => false,
        }
    }

    pub fn colors(&self) -> &[Color] {
        &self.colors
    }

    pub fn is_default(&self) -> bool {
        self.colors.len() == self.defaults.len()
            && self
                .colors
                .iter()
                .zip(&self.defaults)
                .all(|(c, d)| theme::colors_equal(*c, *d))
    }

    pub fn view(&self) -> Element<'_, PaletteEvent> {
        let row = self
            .colors
            .iter()
            .enumerate()
            .fold(Row::new().spacing(12.0), |r, (i, &c)| {
                r.push(self.color_picker(i, c))
            });
        let mut col = Column::new().spacing(12.0).push(row);
        if let Some(i) = self.active
            && let Some(&c) = self.colors.get(i)
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
                    format!("{:>3}", (val.clamp(0.0, 1.0) * 255.0).round() as u8)
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
    let d = Color {
        r: color.r * color.a,
        g: color.g * color.a,
        b: color.b * color.a,
        a: color.a,
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
    let (r, g, b, a) = (
        (c.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.b.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.a.clamp(0.0, 1.0) * 255.0).round() as u8,
    );
    if a == 255 {
        format!("#{r:02X}{g:02X}{b:02X}")
    } else {
        format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
    }
}
