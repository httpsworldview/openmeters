// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::persistence::settings::PaletteSettings;
use crate::ui::theme::{self as ui_theme, Palette};
use crate::ui::widgets::scroll_glow::{ScrollEvent, ScrollGlow};
use crate::ui::widgets::{action_button, clipped_text, split, value_label};
use crate::util::color::{
    EPSILON, STOP_SPREAD_MAX, STOP_SPREAD_MIN, colors_equal, from_hsv, lerp_color, parse_hex_color,
    sanitize_stop_positions, sanitize_stop_spreads, to_hex, to_hsv, with_alpha,
};
use crate::visuals::render::common::scroll_delta_lines;
use iced::advanced::graphics::color;
use iced::advanced::graphics::mesh::{self, Mesh, Renderer as _, SolidVertex2D};
use iced::advanced::widget::{Operation, Tree, tree, tree::Tag};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, text};
use iced::alignment::{Horizontal, Vertical};
use iced::widget::{Button, Column, Row, Space, column, container, row, text_input};
use iced::{
    Border, Color, Element, Event, Length, Point, Rectangle, Size, Transformation, Vector,
    keyboard, touch, window,
};

const SWATCH_SIZE: (f32, f32) = (56.0, 28.0);
const GRADIENT_BAR_HEIGHT: f32 = 24.0;
const MARKER_HEIGHT: f32 = 8.0;
const MIN_STOP_GAP: f32 = 0.01;
const SAT_VAL_HEIGHT: f32 = 120.0;
const BAR_HEIGHT: f32 = 12.0;
const RAMP_SAMPLE_WIDTH: f32 = 4.0;
const HANDLE_WIDTH: f32 = 10.0;

#[derive(Debug, Clone, PartialEq)]
pub enum PaletteEvent {
    Select(Option<usize>),
    EditHex(Option<String>),
    /// Applies a valid hex draft and ends a drag.
    Commit,
    /// Applies a valid hex draft, ending a drag only if it does.
    ApplyHex,
    Adjust(HsvEdit),
    ResetColor,
    AdjustPosition {
        index: usize,
        position: f32,
    },
    AdjustSpread {
        index: usize,
        spread: f32,
    },
    Reset,
    HorizontalScroll(ScrollEvent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteChange {
    /// A drag moved it; show it without saving.
    Preview,
    /// It differs from when it was last saved.
    Save,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HsvEdit {
    SatVal(f32, f32),
    Hue(f32),
    Alpha(f32),
}

struct Selection {
    index: usize,
    hex_draft: Option<String>,
    /// Keeps hue and saturation where the color no longer defines them.
    hsv: [f32; 3],
}

pub struct PaletteEditor {
    palette: Palette,
    positions: Vec<f32>,
    spreads: Vec<f32>,
    active: Option<Selection>,
    saved: Option<PaletteSettings>,
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
            saved: None,
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
        let _ = self.active.take_if(|active| only && active.index != 0);
    }

    fn active_index(&self) -> Option<usize> {
        self.active.as_ref().map(|active| active.index)
    }

    pub fn set_label_overrides(&mut self, overrides: &'static [(usize, &'static str)]) {
        self.label_overrides = overrides;
    }

    fn label_for(&self, index: usize) -> &'static str {
        self.label_overrides
            .iter()
            .find_map(|&(i, label)| (i == index).then_some(label))
            .unwrap_or(self.palette.labels()[index])
    }

    pub fn load(&mut self, stored: &PaletteSettings) {
        let positions = stored.stop_positions.as_deref();
        self.positions = sanitize_stop_positions(positions, self.palette.default_positions);
        self.spreads = sanitize_stop_spreads(stored.stop_spreads.as_deref(), self.palette.len());
        let stops: Vec<Color> = stored.stops.iter().copied().map(Into::into).collect();
        self.set_colors(&stops);
    }

    /// The stored form, or `None` while everything is default.
    pub fn settings(&self) -> Option<PaletteSettings> {
        PaletteSettings::from_state(
            self.palette.colors(),
            self.palette.defaults,
            &self.positions,
            self.palette.default_positions,
            &self.spreads,
        )
    }

    /// Replaces the colors with saved ones.
    pub fn set_colors(&mut self, colors: &[Color]) {
        self.palette.set_colors(colors);
        self.saved = self.settings();
        if let Some(active) = &mut self.active {
            active.hex_draft = None;
            active.hsv = to_hsv(self.palette.colors()[active.index], active.hsv);
        }
    }

    pub fn update(&mut self, event: PaletteEvent) -> Option<PaletteChange> {
        use PaletteEvent::*;
        let ends_drag = match event {
            Adjust(_) | AdjustPosition { .. } => {
                self.apply(event);
                return Some(PaletteChange::Preview);
            }
            Select(_) | EditHex(_) | HorizontalScroll(_) => false,
            // Blurring the hex input by starting a drag must not end it.
            ApplyHex => self.draft_color().is_some(),
            Commit | ResetColor | AdjustSpread { .. } | Reset => true,
        };
        self.apply(event);
        ends_drag.then(|| self.save_changes()).flatten()
    }

    /// Saves changes since the last save, e.g. from a drag whose release was lost.
    pub fn save_changes(&mut self) -> Option<PaletteChange> {
        let settings = self.settings();
        (settings != self.saved).then(|| {
            self.saved = settings;
            PaletteChange::Save
        })
    }

    fn apply(&mut self, event: PaletteEvent) {
        match event {
            PaletteEvent::Select(index) => {
                self.active = index.map(|index| Selection {
                    index,
                    hex_draft: None,
                    hsv: to_hsv(self.palette.colors()[index], [0.0; 3]),
                });
            }
            PaletteEvent::EditHex(value) => {
                if let Some(active) = &mut self.active {
                    active.hex_draft = value;
                }
            }
            PaletteEvent::Commit | PaletteEvent::ApplyHex => {
                if let Some(color) = self.draft_color() {
                    self.set_color(color, None);
                }
            }
            PaletteEvent::Adjust(edit) => {
                let Some(active) = &self.active else {
                    return;
                };
                // Controls can change before or without blurring the hex input.
                let (base, hsv) = match self.draft_color() {
                    Some(draft) => (draft, to_hsv(draft, active.hsv)),
                    None => (self.palette.colors()[active.index], active.hsv),
                };
                let [h, s, v] = hsv;
                let (color, hsv) = match edit {
                    HsvEdit::SatVal(s, v) => (from_hsv([h, s, v], base.a), [h, s, v]),
                    HsvEdit::Hue(h) => (from_hsv([h, s, v], base.a), [h, s, v]),
                    // Opacity edits skip the lossy HSV round trip.
                    HsvEdit::Alpha(a) => (with_alpha(base, a), hsv),
                };
                self.set_color(color, Some(hsv));
            }
            PaletteEvent::ResetColor => {
                if let Some(index) = self.active_index() {
                    self.set_color(self.palette.defaults[index], None);
                }
            }
            PaletteEvent::AdjustPosition { index, position } => {
                let lo = (self.positions[index - 1] + MIN_STOP_GAP).max(MIN_STOP_GAP);
                let hi = (self.positions[index + 1] - MIN_STOP_GAP).min(1.0 - MIN_STOP_GAP);
                if lo <= hi {
                    self.positions[index] = position.clamp(lo, hi);
                }
            }
            PaletteEvent::AdjustSpread { index, spread } => self.spreads[index] = spread,
            PaletteEvent::HorizontalScroll(event) => self.scroll.update(event),
            PaletteEvent::Reset => {
                self.active = None;
                self.palette.reset();
                self.positions
                    .copy_from_slice(self.palette.default_positions);
                self.spreads.fill(1.0);
            }
        }
    }

    fn draft_color(&self) -> Option<Color> {
        let active = self.active.as_ref()?;
        let color = self.palette.colors()[active.index];
        let parsed = parse_hex_color(active.hex_draft.as_deref()?.trim()).ok()?;
        // Preserve picker precision when the displayed RGBA8 value is unchanged.
        Some(if parsed.into_rgba8() == color.into_rgba8() {
            color
        } else {
            parsed
        })
    }

    /// Replaces the selected color; `hsv` defaults to one derived from it.
    fn set_color(&mut self, color: Color, hsv: Option<[f32; 3]>) {
        let Some(active) = &mut self.active else {
            return;
        };
        let colors = self.palette.colors();
        if !colors_equal(colors[active.index], color) {
            let mut colors = colors.to_vec();
            colors[active.index] = color;
            self.palette.set_colors(&colors);
        }
        active.hex_draft = None;
        active.hsv = hsv.unwrap_or_else(|| to_hsv(color, active.hsv));
    }

    pub fn colors(&self) -> &[Color] {
        self.palette.colors()
    }

    fn visible_colors(&self) -> &[Color] {
        let colors = self.palette.colors();
        if self.only_first_visible {
            &colors[..1]
        } else {
            colors
        }
    }

    pub fn preview<'a, M: 'a>(&self) -> Element<'a, M> {
        let swatches = self.visible_colors().iter().map(|&color| {
            container(Space::new().width(10).height(10))
                .style(move |_| container::Style::default().background(color))
                .into()
        });
        container(Row::with_children(swatches))
            .padding(2)
            .style(ui_theme::chip)
            .into()
    }

    pub fn view(&self) -> Element<'_, PaletteEvent> {
        let pickers = self.visible_colors().iter().enumerate();
        let row = Row::with_children(pickers.map(|(i, &c)| self.color_picker(i, c))).spacing(12);
        Column::new()
            .spacing(12)
            .extend(self.show_ramp.then(|| Element::new(self)))
            .push(self.scroll.horizontal(row, PaletteEvent::HorizontalScroll))
            .extend(self.color_controls())
            .push(action_button(
                "Reset to defaults",
                self.settings().map(|_| PaletteEvent::Reset),
            ))
            .into()
    }

    fn color_picker(&self, i: usize, c: Color) -> Element<'_, PaletteEvent> {
        let active = self.active_index() == Some(i);
        Button::new(
            column![
                clipped_text(self.label_for(i), 11.0),
                Element::new(Swatch { color: c, active }),
                clipped_text(to_hex(c), 11.0),
            ]
            .spacing(4)
            .align_x(Horizontal::Center),
        )
        .padding([6, 8])
        .style(|theme, status| ui_theme::button_style(theme, false, status))
        .on_press(PaletteEvent::Select((!active).then_some(i)))
        .into()
    }

    fn color_controls(&self) -> Option<Element<'_, PaletteEvent>> {
        let active = self.active.as_ref()?;
        let index = active.index;
        let c = self.palette.colors()[index];
        let canonical = to_hex(c);
        let valid = active.hex_draft.is_none() || self.draft_color().is_some();
        let hex = text_input("", active.hex_draft.as_deref().unwrap_or(&canonical))
            .on_input(|value| PaletteEvent::EditHex(Some(value)))
            .on_submit(PaletteEvent::Commit)
            .size(ui_theme::BODY_TEXT_SIZE)
            .style(move |theme, status| {
                let mut style = text_input::default(theme, status);
                if !valid {
                    style.border.color = theme.extended_palette().danger.base.color;
                }
                style
            });
        let [h, s, v] = active.hsv;
        let hsva = [h, s, v, c.a];
        let area = |channel| Element::new(HsvArea { hsva, channel });
        let reset = !colors_equal(c, self.palette.defaults[index]);
        let content = column![
            row![
                clipped_text(self.label_for(index), ui_theme::BODY_TEXT_SIZE).width(Length::Fill),
                action_button("Reset", reset.then_some(PaletteEvent::ResetColor)),
                action_button("Done", Some(PaletteEvent::Select(None))),
            ]
            .spacing(ui_theme::CONTROL_GAP)
            .align_y(Vertical::Center),
            area(Channel::SatVal),
            area(Channel::Hue),
            column![
                split(
                    clipped_text("Hex", ui_theme::BODY_TEXT_SIZE),
                    value_label("Opacity", format!("{:.0}%", c.a * 100.0)),
                ),
                split(Element::new(HexInput(hex.into())), area(Channel::Alpha))
                    .align_y(Vertical::Center),
            ]
            .spacing(ui_theme::CONTROL_GAP),
        ]
        .spacing(ui_theme::SECTION_GAP);
        Some(
            container(content)
                .padding(12)
                .style(ui_theme::weak_container)
                .into(),
        )
    }
}

type InputState = text_input::State<<iced::Renderer as text::Renderer>::Paragraph>;

// Commits or cancels the hex draft on blur, which Iced has no callback for.
struct HexInput<'a>(Element<'a, PaletteEvent>);

impl Widget<PaletteEvent, iced::Theme, iced::Renderer> for HexInput<'_> {
    fn tag(&self) -> Tag {
        self.0.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.0.as_widget().state()
    }

    fn diff(&self, tree: &mut Tree) {
        self.0.as_widget().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.0.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.0.as_widget_mut().layout(tree, renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.0
            .as_widget_mut()
            .operate(tree, layout, renderer, operation);
    }

    crate::macros::widget_method!(update PaletteEvent; this; tree, event, layout, cursor, renderer, clipboard, shell, viewport => {
        let focused = |tree: &Tree| tree.state.downcast_ref::<InputState>().is_focused();
        let was_focused = focused(tree);
        this.0.as_widget_mut().update(tree, event, layout, cursor, renderer, clipboard, shell, viewport);
        // Leaving the window keeps Iced's focus but also counts as clicking away.
        let left = focused(tree) && matches!(event, Event::Window(window::Event::Unfocused));
        if was_focused && !focused(tree) || left {
            let cancel = matches!(event, Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape), ..
            }));
            shell.publish(if cancel { PaletteEvent::EditHex(None) } else { PaletteEvent::ApplyHex });
        }
    });

    crate::macros::widget_method!(draw this; tree, renderer, theme, style, layout, cursor, viewport => {
        this.0.as_widget().draw(tree, renderer, theme, style, layout, cursor, viewport);
    });

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.0
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }
}

// Checkered underneath so transparency stays visible.
struct Swatch {
    color: Color,
    active: bool,
}

impl Widget<PaletteEvent, iced::Theme, iced::Renderer> for Swatch {
    crate::macros::widget_method!(layout
        Size::new(Length::Fixed(SWATCH_SIZE.0), Length::Fixed(SWATCH_SIZE.1)),
        |limits| limits.resolve(SWATCH_SIZE.0, SWATCH_SIZE.1, Size::ZERO)
    );

    crate::macros::widget_method!(draw this; _, renderer, theme, _, layout, _, _ => {
        let bounds = layout.bounds();
        let patches = checkerboard(bounds, 4)
            .chain([solid(edges(bounds), this.color)])
            .chain(outline(edges(bounds), ui_theme::border(theme, this.active)));
        draw(renderer, bounds, patches);
    });
}

#[derive(Clone, Copy)]
enum Channel {
    SatVal,
    Hue,
    Alpha,
}

struct HsvArea {
    hsva: [f32; 4],
    channel: Channel,
}

impl HsvArea {
    fn height(&self) -> f32 {
        match self.channel {
            Channel::SatVal => SAT_VAL_HEIGHT,
            Channel::Hue | Channel::Alpha => BAR_HEIGHT,
        }
    }

    /// The edit at `point` within `inner`, unless it would keep the current value.
    fn pick(&self, inner: Rectangle, point: Point) -> Option<PaletteEvent> {
        let fraction = |offset: f32, extent: f32| (offset / extent.max(1.0)).clamp(0.0, 1.0);
        let x = fraction(point.x - inner.x, inner.width);
        let [h, s, v, a] = self.hsva;
        let (edit, change) = match self.channel {
            Channel::SatVal => {
                let y = 1.0 - fraction(point.y - inner.y, inner.height);
                (HsvEdit::SatVal(x, y), (x - s).abs().max((y - v).abs()))
            }
            Channel::Hue => (HsvEdit::Hue(x), (x - h).abs()),
            Channel::Alpha => (HsvEdit::Alpha(x), (x - a).abs()),
        };
        (change >= EPSILON).then_some(PaletteEvent::Adjust(edit))
    }
}

impl Widget<PaletteEvent, iced::Theme, iced::Renderer> for HsvArea {
    crate::macros::widget_method!(state Option<()>);

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(self.height()))
    }

    fn layout(
        &mut self,
        _: &mut Tree,
        _: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(Length::Fill, self.height(), Size::ZERO))
    }

    crate::macros::widget_method!(update PaletteEvent; this; tree, event, layout, cursor, _, _, shell, _ => {
        let bounds = layout.bounds();
        let pick = |(), point| this.pick(bounds.shrink(1.0), point);
        drag(tree.state.downcast_mut(), event, cursor, bounds, shell, |_| Some(()), pick);
    });

    crate::macros::widget_method!(draw this; _, renderer, theme, _, layout, _, _ => {
        let bounds = layout.bounds();
        // Inside the outline, so every value has a pixel, as in `pick`.
        let inner = bounds.shrink(1.0);
        let hue = |h: f32| from_hsv([h, 1.0, 1.0], 1.0);
        let area = edges(inner);
        let bar = Size::new(HANDLE_WIDTH * 0.5, BAR_HEIGHT + 6.0);
        let mut patches = Vec::new();
        let [h, s, v, a] = this.hsva;
        let (x, y, size, fill) = match this.channel {
            Channel::SatVal => {
                let fade = [Color::TRANSPARENT, Color::TRANSPARENT, Color::BLACK, Color::BLACK];
                patches.extend([horizontal(area, Color::WHITE, hue(h)), (area, fade)]);
                (s, 1.0 - v, Size::new(HANDLE_WIDTH, HANDLE_WIDTH), from_hsv([h, s, v], 1.0))
            }
            Channel::Hue => {
                patches.extend((0..6).map(|i| {
                    let (start, end) = (i as f32 / 6.0, (i + 1) as f32 / 6.0);
                    horizontal(span(inner, start, end), hue(start), hue(end))
                }));
                (h, 0.5, bar, hue(h))
            }
            Channel::Alpha => {
                let opaque = from_hsv([h, s, v], 1.0);
                patches.extend(checkerboard(bounds, 2));
                patches.push(horizontal(area, with_alpha(opaque, 0.0), opaque));
                (a, 0.5, bar, opaque)
            }
        };
        patches.extend(outline(edges(bounds), ui_theme::border(theme, false)));
        let center = Point::new(inner.x + x * inner.width, inner.y + y * inner.height);
        patches.extend(handle(center, size, fill, ui_theme::border(theme, true)));
        draw(renderer, bounds.expand(HANDLE_WIDTH), patches);
    });
}

/// Edges `[left, top, right, bottom]` and corner colors
/// `[top left, top right, bottom left, bottom right]`.
type Patch = ([f32; 4], [Color; 4]);

/// Draws patches as one mesh. Unlike Iced's quads, which ease between gradient
/// stops, it interpolates linearly without seams. Meshes skip crisp snapping;
/// Iced only snaps the clip origin, so edges round relative to it here to keep
/// outlines one pixel wide.
fn draw(renderer: &mut iced::Renderer, clip: Rectangle, patches: impl IntoIterator<Item = Patch>) {
    let (mut vertices, mut indices) = (Vec::new(), Vec::new());
    let snap = |value: f32, origin: f32| (value - origin).round() + origin;
    for ([left, top, right, bottom], colors) in patches {
        let [left, right] = [left, right].map(|x| snap(x, clip.x));
        let [top, bottom] = [top, bottom].map(|y| snap(y, clip.y));
        let base = vertices.len() as u32;
        let corners = [[left, top], [right, top], [left, bottom], [right, bottom]];
        vertices.extend(
            corners
                .into_iter()
                .zip(colors)
                .map(|(position, c)| SolidVertex2D {
                    position,
                    color: color::pack(c),
                }),
        );
        indices.extend([0, 1, 2, 1, 3, 2].map(|i| base + i));
    }
    renderer.draw_mesh(Mesh::Solid {
        buffers: mesh::Indexed { vertices, indices },
        transformation: Transformation::IDENTITY,
        clip_bounds: clip,
    });
}

fn edges(r: Rectangle) -> [f32; 4] {
    [r.x, r.y, r.x + r.width, r.y + r.height]
}

/// The part of `bounds` between two horizontal fractions.
fn span(bounds: Rectangle, start: f32, end: f32) -> [f32; 4] {
    let x = |t: f32| bounds.x + t * bounds.width;
    [x(start), bounds.y, x(end), bounds.y + bounds.height]
}

fn solid(edges: [f32; 4], color: Color) -> Patch {
    (edges, [color; 4])
}

fn horizontal(edges: [f32; 4], left: Color, right: Color) -> Patch {
    (edges, [left, right, left, right])
}

fn outline([l, t, r, b]: [f32; 4], border: Border) -> [Patch; 4] {
    let w = border.width;
    [
        [l, t, r, t + w],
        [l, b - w, r, b],
        [l, t + w, l + w, b - w],
        [r - w, t + w, r, b - w],
    ]
    .map(|edges| solid(edges, border.color))
}

fn handle(center: Point, size: Size, fill: Color, border: Border) -> impl Iterator<Item = Patch> {
    let area = edges(Rectangle::new(
        center - Vector::new(size.width, size.height) * 0.5,
        size,
    ));
    std::iter::once(solid(area, fill)).chain(outline(area, border))
}

fn checkerboard(bounds: Rectangle, rows: usize) -> impl Iterator<Item = Patch> {
    let cell = bounds.height / rows as f32;
    // Squeezed bounds would otherwise need unbounded columns.
    let columns = (bounds.width / cell.max(1.0)).ceil() as usize;
    let [left, top, right, bottom] = edges(bounds);
    let light = (0..columns).flat_map(move |x| {
        (0..rows).filter(move |y| (x + y) % 2 == 0).map(move |y| {
            let (l, t) = (left + x as f32 * cell, top + y as f32 * cell);
            let edges = [l, t, (l + cell).min(right), (t + cell).min(bottom)];
            solid(edges, Color::from_rgb8(0x80, 0x80, 0x80))
        })
    });
    std::iter::once(solid(edges(bounds), Color::from_rgb8(0x4D, 0x4D, 0x4D))).chain(light)
}

/// Follows a left-button or touch drag that `grab` starts over `bounds`,
/// previewing `pick` at each cursor position and committing on release.
fn drag<T: Copy>(
    dragging: &mut Option<T>,
    event: &Event,
    cursor: mouse::Cursor,
    bounds: Rectangle,
    shell: &mut Shell<'_, PaletteEvent>,
    grab: impl FnOnce(Point) -> Option<T>,
    pick: impl Fn(T, Point) -> Option<PaletteEvent>,
) {
    match event {
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
        | Event::Touch(touch::Event::FingerPressed { .. }) => {
            *dragging = cursor.position_over(bounds).and_then(grab);
        }
        Event::Mouse(mouse::Event::CursorMoved { .. })
        | Event::Touch(touch::Event::FingerMoved { .. }) => {}
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
        | Event::Touch(touch::Event::FingerLifted { .. } | touch::Event::FingerLost { .. })
            if dragging.take().is_some() =>
        {
            shell.publish(PaletteEvent::Commit);
            return shell.capture_event();
        }
        _ => return,
    }
    if let Some(grabbed) = *dragging {
        // The cursor, unlike the event position, accounts for scrolling, and
        // landing it keeps the drag going outside the scrollable.
        if let Some(event) = cursor.land().position().and_then(|p| pick(grabbed, p)) {
            shell.publish(event);
        }
        shell.capture_event();
    }
}

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
    crate::macros::widget_method!(state Option<(usize, f32)>);

    crate::macros::widget_method!(layout
        Size::new(Length::Fill, Length::Fixed(TOTAL_HEIGHT)),
        |limits| limits.resolve(Length::Fill, Length::Fixed(TOTAL_HEIGHT), Size::ZERO)
    );

    crate::macros::widget_method!(update PaletteEvent; this; tree, event, layout, cursor, _, _, shell, _ => {
        let editor = *this;
        let n = editor.positions.len();
        let bounds = layout.bounds();
        let at = |point: Point| (point.x - bounds.x) / bounds.width;
        match event {
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if let Some(pos) = cursor.position_over(bounds)
                    && let Some(i) = nearest_handle(0..n, &editor.positions, &bounds, pos.x)
                {
                    let current = editor.spreads[i];
                    let spread = (current + scroll_delta_lines(*delta) * 0.2)
                        .clamp(STOP_SPREAD_MIN, STOP_SPREAD_MAX);
                    if (spread - current).abs() >= EPSILON {
                        shell.publish(PaletteEvent::AdjustSpread { index: i, spread });
                    }
                    shell.capture_event();
                }
            }
            // Holding the grab offset keeps the stop from jumping to the cursor.
            event => drag(
                tree.state.downcast_mut(),
                event,
                cursor,
                bounds,
                shell,
                |point| {
                    let i = nearest_handle(1..n - 1, &editor.positions, &bounds, point.x)?;
                    Some((i, editor.positions[i] - at(point)))
                },
                // Unmoved stops skip the gap clamp, which loaded positions may not meet.
                |(index, offset), point| {
                    let position = at(point) + offset;
                    ((position - editor.positions[index]).abs() >= EPSILON)
                        .then_some(PaletteEvent::AdjustPosition { index, position })
                },
            ),
        }
    });

    crate::macros::widget_method!(draw this; _, renderer, theme, _, layout, _, _ => {
        let editor = *this;
        let colors = editor.palette.colors();
        let bounds = layout.bounds();
        let bar = Rectangle::new(bounds.position(), Size::new(bounds.width, GRADIENT_BAR_HEIGHT));
        let (positions, spreads) = (&editor.positions, &editor.spreads);
        let sample = |t| {
            let (lo, hi, f) = find_segment(positions, spreads, t);
            (t, lerp_color(colors[lo], colors[hi], f))
        };
        // Meshes interpolate premultiplied colors, unlike the visuals, so spans
        // are only drawn linearly when neither spread nor alpha varies.
        let samples: Vec<_> = positions.windows(2).enumerate().flat_map(|(i, span)| {
            let (start, end) = (span[0], span[1]);
            let varies = (spreads[i] - spreads[i + 1]).abs() >= EPSILON
                || (colors[i].a - colors[i + 1].a).abs() >= EPSILON;
            let count = if varies {
                ((end - start) * bar.width / RAMP_SAMPLE_WIDTH).ceil().max(1.0) as usize
            } else {
                1
            };
            (0..count).map(move |k| sample(start + (end - start) * k as f32 / count as f32))
        }).chain([sample(1.0)]).collect();
        let ramp = samples.windows(2).map(|w| horizontal(span(bar, w[0].0, w[1].0), w[0].1, w[1].1));
        let handle_y = bar.y + bar.height + (MARKER_HEIGHT + 1.0) * 0.5;
        let handles = positions.iter().zip(colors).enumerate().flat_map(|(i, (&pos, &c))| {
            let x = bar.x + pos * bar.width;
            let active = editor.active_index() == Some(i);
            let line_alpha = if active { 1.0 } else { 0.5 };
            let line = [x - INDICATOR_WIDTH * 0.5, bar.y, x + INDICATOR_WIDTH * 0.5, bar.y + bar.height];
            let width = if active { HANDLE_WIDTH } else { HANDLE_WIDTH - 2.0 };
            let fill = Color::from_rgb(c.r.max(0.12), c.g.max(0.12), c.b.max(0.12));
            let size = Size::new(width, MARKER_HEIGHT - 1.0);
            std::iter::once(solid(line, with_alpha(Color::WHITE, line_alpha)))
                .chain(handle(Point::new(x, handle_y), size, fill, ui_theme::border(theme, active)))
        });
        let patches = checkerboard(bar, 4)
            .chain(ramp)
            .chain(outline(edges(bar), ui_theme::border(theme, false)))
            .chain(handles);
        draw(renderer, bounds.expand(HANDLE_WIDTH), patches);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visuals::palettes::waveform;

    fn draft(editor: &PaletteEditor) -> Option<&str> {
        editor.active.as_ref()?.hex_draft.as_deref()
    }

    #[test]
    fn hex_and_picker_edits_preserve_precision_and_other_slots() {
        use {HsvEdit::*, PaletteChange::*, PaletteEvent::*};
        let mut editor = PaletteEditor::new(waveform::PALETTE);
        let mut colors = waveform::COLORS.to_vec();
        colors[0] = Color::from_rgba(0.12345, 0.54321, 0.67891, 0.45678);
        editor.set_colors(&colors);
        editor.update(Select(Some(0)));
        assert_eq!(editor.update(Commit), None);
        for input in [to_hex(colors[0]), "#".into()] {
            editor.update(EditHex(Some(input)));
            assert_eq!(editor.update(Commit), None);
            assert_eq!(editor.colors(), colors);
        }
        assert_eq!(draft(&editor), Some("#"));
        editor.update(EditHex(Some("#fff".into())));
        editor.update(EditHex(None));
        assert_eq!(editor.update(Commit), None);
        assert_eq!(editor.colors(), colors);
        for (input, expected) in [(" #aBc8 ", "#AABBCC88"), ("abc", "#AABBCC")] {
            editor.update(EditHex(Some(input.into())));
            assert_eq!(editor.colors(), colors);
            assert_eq!(editor.update(Commit), Some(Save));
            colors[0] = parse_hex_color(expected).unwrap();
            assert_eq!(editor.colors(), colors);
            assert_eq!(to_hex(colors[0]), expected);
        }
        let half = |color| with_alpha(color, 0.5);
        let (white, black) = (half(Color::WHITE), half(Color::BLACK));
        let (cyan, red) = (
            half(Color::from_rgb(0.0, 1.0, 1.0)),
            half(Color::from_rgb(1.0, 0.0, 0.0)),
        );
        for (input, edit, expected) in [
            (Some("#"), Alpha(0.5), half(colors[0])),
            (Some("#fff"), Alpha(0.5), white),
            // Hue survives gray and black.
            (None, Hue(0.5), white),
            (None, SatVal(1.0, 0.0), black),
            (None, SatVal(1.0, 1.0), cyan),
            // Full hue stays at the end of the bar.
            (None, Hue(1.0), red),
        ] {
            editor.update(EditHex(input.map(str::to_owned)));
            assert_eq!(editor.update(Adjust(edit)), Some(Preview));
            colors[0] = expected;
            assert!(draft(&editor).is_none());
            assert_eq!(editor.colors(), colors);
        }
        assert_eq!(editor.active.as_ref().unwrap().hsv, [1.0, 1.0, 1.0]);
        assert_eq!(editor.update(ApplyHex), None);
        assert_eq!(editor.update(Commit), Some(Save));
    }

    #[test]
    fn edits_save_once_they_end_and_differ() {
        use {HsvEdit::*, PaletteChange::*, PaletteEvent::*};
        let mut editor = PaletteEditor::new(waveform::PALETTE);
        let white = [Color::WHITE; 3];
        editor.update(Select(Some(0)));
        editor.update(Adjust(Alpha(0.5)));
        editor.update(EditHex(Some("#abc".into())));
        editor.set_colors(&white);
        assert_eq!(editor.update(Commit), None);
        assert_eq!(editor.colors(), white);
        for alpha in [0.5, 1.0] {
            assert_eq!(editor.update(Adjust(Alpha(alpha))), Some(Preview));
        }
        assert_eq!(editor.update(Commit), None);
        editor.update(Adjust(Alpha(0.5)));
        assert_eq!(editor.update(ResetColor), Some(Save));
        assert_eq!(editor.update(ResetColor), None);
        assert_eq!(editor.colors()[0], waveform::COLORS[0]);
        editor.update(EditHex(Some("#abc".into())));
        assert_eq!(editor.update(Reset), Some(Save));
        assert_eq!(editor.update(Commit), None);
        assert_eq!(editor.colors(), waveform::COLORS);
    }
}
