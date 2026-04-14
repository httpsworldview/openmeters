// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{
    FrequencyScale, SpectrogramColumn, SpectrogramConfig,
    SpectrogramProcessor as CoreSpectrogramProcessor, SpectrogramUpdate,
};
use super::render::{
    ColumnKind, PendingUpload, SPECTROGRAM_PALETTE_SIZE, SpectrogramParams, SpectrogramPrimitive,
    col_byte_stride,
};
use crate::persistence::settings::PianoRollOverlay;
use crate::ui::theme::BORDER_SUBTLE;
use crate::util::audio::musical::{MusicalNote, NoteInfo};
use crate::util::audio::{DB_FLOOR, DEFAULT_SAMPLE_RATE, fmt_duration, fmt_freq};
use crate::util::color::{color_to_rgba, lerp_color, rgba_with_alpha, with_alpha};
use crate::vis_processor;
use crate::visuals::palettes;
use iced::advanced::renderer::{self, Quad};
use iced::advanced::text::Renderer as _;
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Layout, Renderer as _, Widget, layout, mouse};
use iced::{Background, Color, Element, Length, Point, Rectangle, Size, keyboard};
use iced_wgpu::primitive::Renderer as _;
use std::cell::RefCell;

const DB_CEILING: f32 = 0.0;
const TOOLTIP_SIZE: f32 = 14.0;
const TOOLTIP_PAD: f32 = 8.0;
const TOOLTIP_GAP: f32 = 2.0;
const TOOLTIP_OFFSET: f32 = 12.0;
const TOOLTIP_BG_ALPHA: f32 = 0.85;
const TOOLTIP_BORDER_ALPHA: f32 = 0.4;
const PIANO_ROLL_WIDTH: f32 = 18.0;
const PIANO_BLACK_KEY_RATIO: f32 = 0.6;
const PIANO_LABEL_SIZE: f32 = 9.0;
const PIANO_MIDI_LO: i32 = 21; // A0
const PIANO_MIDI_HI: i32 = 119; // C8

fn spec_freq_min(_scale: FrequencyScale, min_freq: f32) -> f32 {
    min_freq
}

fn norm_to_freq(inv: f32, nyquist: f32, min_freq: f32, scale: FrequencyScale) -> f32 {
    scale.freq_at(spec_freq_min(scale, min_freq), nyquist, inv)
}

fn freq_to_norm(freq: f32, nyquist: f32, min_freq: f32, scale: FrequencyScale) -> f32 {
    scale.pos_of(spec_freq_min(scale, min_freq), nyquist, freq)
}

vis_processor!(
    SpectrogramProcessor,
    CoreSpectrogramProcessor,
    SpectrogramConfig,
    SpectrogramUpdate,
    sync_rate
);

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SpectrogramStyle {
    pub background: Color,
    pub(crate) floor_db: f32,
    pub ceiling_db: f32,
    pub opacity: f32,
    pub contrast: f32,
    pub(crate) tilt_db: f32,
}

impl Default for SpectrogramStyle {
    fn default() -> Self {
        Self {
            background: with_alpha(palettes::BG_BASE, 0.0),
            floor_db: DB_FLOOR,
            ceiling_db: DB_CEILING,
            opacity: 0.95,
            contrast: 1.0,
            tilt_db: 0.0,
        }
    }
}

#[derive(Debug)]
pub(crate) struct SpectrogramState {
    pub(crate) style: SpectrogramStyle,
    pub(crate) palette: [Color; SPECTROGRAM_PALETTE_SIZE],
    pub(crate) stop_positions: [f32; SPECTROGRAM_PALETTE_SIZE],
    pub(crate) stop_spreads: [f32; SPECTROGRAM_PALETTE_SIZE],
    key: u64,
    pub(crate) piano_roll_overlay: PianoRollOverlay,
    pub(crate) rotation: i8,
    sample_rate: f32,
    fft_size: usize,
    hop_size: usize,
    freq_scale: FrequencyScale,
    col_kind: ColumnKind,
    zoom: f32,
    pan: f32,
    pub(crate) view_width: u32,
    pub(crate) view_height: u32,
    points_per_column: usize,
    ring_capacity: u32,
    write_slot: u32,
    col_count: u32,
    pending: Vec<PendingUpload>,
    linearize_from: Option<u32>,
}

impl SpectrogramState {
    pub fn new() -> Self {
        Self {
            style: SpectrogramStyle::default(),
            palette: palettes::spectrogram::COLORS,
            stop_positions: palettes::spectrogram::DEFAULT_POSITIONS,
            stop_spreads: [1.0; SPECTROGRAM_PALETTE_SIZE],
            key: crate::visuals::next_key(),
            piano_roll_overlay: PianoRollOverlay::Off,
            rotation: 0,
            sample_rate: DEFAULT_SAMPLE_RATE,
            fft_size: 4096,
            hop_size: 1024,
            freq_scale: FrequencyScale::default(),
            col_kind: ColumnKind::Reassigned,
            zoom: 1.0,
            pan: 0.5,
            view_width: 0,
            view_height: 0,
            points_per_column: 0,
            ring_capacity: 0,
            write_slot: 0,
            col_count: 0,
            pending: Vec::new(),
            linearize_from: None,
        }
    }

    pub fn set_palette(&mut self, palette: &[Color; SPECTROGRAM_PALETTE_SIZE]) {
        self.palette = *palette;
    }

    pub fn set_stop_positions(&mut self, positions: &[f32]) {
        if let Ok(arr) = <[f32; SPECTROGRAM_PALETTE_SIZE]>::try_from(positions) {
            self.stop_positions = arr;
        }
    }

    pub fn set_stop_spreads(&mut self, spreads: &[f32]) {
        if let Ok(arr) = <[f32; SPECTROGRAM_PALETTE_SIZE]>::try_from(spreads) {
            self.stop_spreads = arr;
        }
    }

    pub fn set_floor_db(&mut self, floor_db: f32) {
        let mut floor = if floor_db.is_finite() {
            floor_db
        } else {
            DB_FLOOR
        };
        if floor >= self.style.ceiling_db - 1.0 {
            floor = self.style.ceiling_db - 1.0;
        }
        self.style.floor_db = floor;
    }

    pub fn set_tilt_db(&mut self, tilt_db: f32) {
        self.style.tilt_db = if tilt_db.is_finite() { tilt_db } else { 0.0 };
    }

    pub fn set_rotation(&mut self, rotation: i8) {
        self.rotation = rotation.clamp(-1, 2);
    }

    pub fn apply_snapshot(&mut self, snap: SpectrogramUpdate) {
        if snap.new_columns.is_empty() && !snap.reset {
            return;
        }
        self.sample_rate = snap.sample_rate;
        self.fft_size = snap.fft_size;
        self.hop_size = snap.hop_size;
        self.freq_scale = snap.frequency_scale;

        let ppc = snap.points_per_column;
        if ppc == 0 {
            return;
        }
        let new_kind = match snap.new_columns.first() {
            Some(SpectrogramColumn::Reassigned(_)) => ColumnKind::Reassigned,
            Some(SpectrogramColumn::Classic(_)) => ColumnKind::Classic,
            None => self.col_kind,
        };
        const GPU_MAX_BUFFER: u64 = 256 * 1024 * 1024; // wgpu guaranteed minimum
        let stride = col_byte_stride(new_kind, ppc as u32);
        let max_cols = (GPU_MAX_BUFFER / stride) as u32;
        let capacity = (snap.history_length as u32).clamp(1, 8192).min(max_cols);

        if snap.reset || self.points_per_column != ppc || new_kind != self.col_kind {
            self.points_per_column = ppc;
            self.ring_capacity = capacity;
            self.col_kind = new_kind;
            self.write_slot = 0;
            self.col_count = 0;
            self.linearize_from = None;
            self.pending.clear();
        } else if capacity > self.ring_capacity {
            if self.col_count >= self.ring_capacity {
                self.linearize_from = (self.write_slot != 0).then_some(self.write_slot);
                self.write_slot = self.col_count % capacity;
            }
            self.ring_capacity = capacity;
        } else if capacity < self.ring_capacity {
            if self.col_count >= capacity {
                let oldest_kept =
                    (self.write_slot + self.ring_capacity - capacity) % self.ring_capacity;
                self.linearize_from = (oldest_kept != 0).then_some(oldest_kept);
                self.col_count = capacity;
                self.write_slot = 0;
            }
            self.ring_capacity = capacity;
        }

        for col in snap.new_columns {
            let upload = match col {
                SpectrogramColumn::Reassigned(points) => PendingUpload::Reassigned {
                    slot: self.write_slot,
                    points,
                },
                SpectrogramColumn::Classic(mags) => PendingUpload::Classic {
                    slot: self.write_slot,
                    mags,
                },
            };
            self.pending.push(upload);
            self.write_slot = (self.write_slot + 1) % self.ring_capacity;
            if self.col_count < self.ring_capacity {
                self.col_count += 1;
            }
        }
    }

    pub fn visual_params(
        &mut self,
        bounds: Rectangle,
        uv_y_range: [f32; 2],
    ) -> Option<SpectrogramParams> {
        if self.col_count == 0 && self.pending.is_empty() {
            return None;
        }
        let op = self.style.opacity.clamp(0.0, 1.0);
        let to_rgba = |c: Color| rgba_with_alpha(color_to_rgba(c), c.a * op);
        let freq_min = self.sample_rate / (self.fft_size.max(1) as f32);
        let freq_max = (self.sample_rate / 2.0).max(freq_min + 1.0);

        Some(SpectrogramParams {
            key: self.key,
            bounds,
            ring_capacity: self.ring_capacity,
            points_per_column: self.points_per_column as u32,
            col_count: self.col_count,
            write_slot: self.write_slot,
            pending_uploads: std::mem::take(&mut self.pending),
            col_kind: self.col_kind,
            freq_min,
            freq_max,
            freq_scale: self.freq_scale,
            palette: self.palette.map(to_rgba),
            stop_positions: self.stop_positions,
            stop_spreads: self.stop_spreads,
            contrast: self.style.contrast,
            floor_db: self.style.floor_db,
            ceiling_db: self.style.ceiling_db,
            tilt_db: self.style.tilt_db,
            uv_y_range,
            rotation: self.rotation,
            linearize_old_write_slot: self.linearize_from.take(),
        })
    }

    fn frequency_at_cursor(
        &self,
        cursor: Point,
        bounds: Rectangle,
        uv_range: [f32; 2],
    ) -> Option<f32> {
        let freq_norm = self.freq_axis_norm(cursor, bounds)?;
        let tex_uv = uv_range[0] + freq_norm * (uv_range[1] - uv_range[0]);
        if self.fft_size == 0 || self.sample_rate <= 0.0 {
            return None;
        }
        let nyq = (self.sample_rate / 2.0).max(1.0);
        let min_f = self.sample_rate / self.fft_size as f32;
        let freq = norm_to_freq(tex_uv, nyq, min_f, self.freq_scale);
        (freq.is_finite() && freq > 0.0).then_some(freq)
    }

    // Normalized rotation (0..3) matching the shader's rotate_uv convention
    fn rotation_index(&self) -> u32 {
        ((self.rotation as i32 % 4) + 4) as u32 % 4
    }

    fn freq_axis_is_horizontal(&self) -> bool {
        matches!(self.rotation_index(), 1 | 3)
    }

    // Maps a screen point to the frequency-axis UV (0..1), matching
    // the shader's rotate_uv so CPU-side interactions stay consistent.
    fn freq_axis_norm(&self, cursor: Point, bounds: Rectangle) -> Option<f32> {
        if !bounds.contains(cursor) {
            return None;
        }
        let norm = match self.rotation_index() {
            1 => (cursor.x - bounds.x) / bounds.width,
            2 => (cursor.y - bounds.y) / bounds.height,
            3 => 1.0 - (cursor.x - bounds.x) / bounds.width,
            _ => 1.0 - (cursor.y - bounds.y) / bounds.height,
        };
        norm.is_finite().then(|| norm.clamp(0.0, 1.0))
    }

    // 1 column = 1 logical pixel on the time axis, matching the shader.
    fn time_ago_at_cursor(&self, cursor: Point, bounds: Rectangle) -> Option<f32> {
        if !bounds.contains(cursor)
            || self.col_count == 0
            || self.hop_size == 0
            || self.sample_rate <= 0.0
        {
            return None;
        }
        let age = match self.rotation_index() {
            0 => bounds.x + bounds.width - cursor.x,
            1 => bounds.y + bounds.height - cursor.y,
            2 => cursor.x - bounds.x,
            3 => cursor.y - bounds.y,
            _ => return None,
        };
        if age < 0.0 || age >= self.col_count as f32 {
            return None;
        }
        let secs = age * (self.hop_size as f32 / self.sample_rate);
        secs.is_finite().then_some(secs)
    }
}

const MIN_ZOOM: f32 = 1.0;
const MAX_ZOOM: f32 = 32.0;
const ZOOM_STEP: f32 = 1.15;

#[derive(Default)]
struct InteractionState {
    cursor: Option<Point>,
    modifiers: keyboard::Modifiers,
    drag: Option<(f32, f32)>, // (freq_axis_origin, start_pan)
    left_held: bool,
}

impl SpectrogramState {
    fn uv_y_range(&self) -> [f32; 2] {
        let h = 0.5 / self.zoom.max(MIN_ZOOM);
        let min = (self.pan - h).clamp(0.0, 1.0 - 2.0 * h);
        [min, (min + 2.0 * h).min(1.0)]
    }

    fn zoom_at(&mut self, y_norm: f32, factor: f32) {
        let (old_h, old_min) = (
            0.5 / self.zoom,
            (self.pan - 0.5 / self.zoom).clamp(0.0, 1.0),
        );
        let cursor_uv = old_min + y_norm * 2.0 * old_h;
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let new_h = 0.5 / self.zoom;
        self.pan = (cursor_uv - new_h * (2.0 * y_norm - 1.0)).clamp(new_h, 1.0 - new_h);
    }
}

pub(crate) struct Spectrogram<'a> {
    state: &'a RefCell<SpectrogramState>,
}

// Places the tooltip adjacent to the cursor on the side opposite the freq
// axis, flipping when it would clip the widget bounds.
fn place_tooltip(bounds: Rectangle, cursor: Point, sz: Size, horizontal: bool) -> Rectangle {
    let max_x = (bounds.x + bounds.width - sz.width).max(bounds.x);
    let max_y = (bounds.y + bounds.height - sz.height).max(bounds.y);
    let (x, y) = if horizontal {
        let x = (cursor.x - sz.width * 0.5).clamp(bounds.x, max_x);
        let y = if cursor.y - TOOLTIP_OFFSET - sz.height >= bounds.y {
            cursor.y - TOOLTIP_OFFSET - sz.height
        } else {
            (cursor.y + TOOLTIP_OFFSET).min(max_y)
        };
        (x, y)
    } else {
        let x = if cursor.x + TOOLTIP_OFFSET + sz.width <= bounds.x + bounds.width {
            cursor.x + TOOLTIP_OFFSET
        } else {
            (cursor.x - TOOLTIP_OFFSET - sz.width).max(bounds.x)
        };
        let y = (cursor.y - sz.height * 0.5).clamp(bounds.y, max_y);
        (x, y)
    };
    Rectangle::new(Point::new(x, y), sz)
}

impl<'a> Spectrogram<'a> {
    pub fn new(state: &'a RefCell<SpectrogramState>) -> Self {
        Self { state }
    }

    fn draw_crosshair(&self, renderer: &mut iced::Renderer, bounds: Rectangle, cursor: Point) {
        let c = BORDER_SUBTLE;
        for rect in [
            Rectangle::new(
                Point::new(cursor.x, bounds.y),
                Size::new(1.0, bounds.height),
            ),
            Rectangle::new(Point::new(bounds.x, cursor.y), Size::new(bounds.width, 1.0)),
        ] {
            renderer.fill_quad(
                Quad {
                    bounds: rect,
                    ..Default::default()
                },
                c,
            );
        }
    }

    fn draw_tooltip(
        &self,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        bounds: Rectangle,
        cursor: Point,
        uv_range: [f32; 2],
    ) {
        let state = self.state.borrow();
        let Some(freq) = state.frequency_at_cursor(cursor, bounds, uv_range) else {
            return;
        };
        let horizontal = state.freq_axis_is_horizontal();
        let time_ago = state.time_ago_at_cursor(cursor, bounds);
        drop(state);

        let freq_text = fmt_freq(freq);
        let note_text = NoteInfo::from_frequency(freq)
            .map_or_else(|| String::from("--"), |ni| ni.fmt_note_cents());
        let time_text = time_ago.map_or_else(|| String::from("--"), fmt_duration);

        let fsz = crate::visuals::measure_text(&freq_text, TOOLTIP_SIZE);
        let nsz = crate::visuals::measure_text(&note_text, TOOLTIP_SIZE);
        let tsz = crate::visuals::measure_text(&time_text, TOOLTIP_SIZE);
        let line_h = fsz.height;
        let content_w = fsz.width.max(nsz.width).max(tsz.width);
        let content_h = line_h * 3.0 + TOOLTIP_GAP * 2.0;
        let sz = Size::new(content_w + TOOLTIP_PAD * 2.0, content_h + TOOLTIP_PAD * 2.0);
        let tb = place_tooltip(bounds, cursor, sz, horizontal);

        let pal = theme.extended_palette();
        renderer.fill_quad(
            Quad {
                bounds: tb,
                border: iced::Border {
                    color: with_alpha(crate::ui::theme::BORDER_SUBTLE, TOOLTIP_BORDER_ALPHA),
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            },
            Background::Color(with_alpha(pal.background.strong.color, TOOLTIP_BG_ALPHA)),
        );

        let text_color = pal.background.base.text;
        let tx = tb.x + TOOLTIP_PAD;
        let mut ty = tb.y + TOOLTIP_PAD;
        for (text, sz) in [(&freq_text, fsz), (&note_text, nsz), (&time_text, tsz)] {
            let pt = Point::new(tx, ty);
            renderer.fill_text(
                crate::visuals::make_text(text, TOOLTIP_SIZE, sz),
                pt,
                text_color,
                Rectangle::new(pt, sz),
            );
            ty += line_h + TOOLTIP_GAP;
        }
    }

    fn draw_piano_roll(
        &self,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        bounds: Rectangle,
        overlay: PianoRollOverlay,
        uv_range: [f32; 2],
    ) {
        let state = self.state.borrow();
        if state.fft_size == 0 || state.sample_rate <= 0.0 {
            return;
        }
        let (nyq, min_f, scale, rot) = (
            (state.sample_rate / 2.0).max(1.0),
            (state.sample_rate / state.fft_size as f32),
            state.freq_scale,
            state.rotation_index(),
        );
        drop(state);
        let horizontal = matches!(rot, 1 | 3);

        let (freq_top, freq_bot) = (
            norm_to_freq(uv_range[1], nyq, min_f, scale),
            norm_to_freq(uv_range[0], nyq, min_f, scale),
        );
        let midi_lo = MusicalNote::from_frequency(freq_bot.max(16.0))
            .map_or(PIANO_MIDI_LO, |n| (n.midi_number - 1).max(PIANO_MIDI_LO));
        let midi_hi = MusicalNote::from_frequency(freq_top)
            .map_or(PIANO_MIDI_HI, |n| (n.midi_number + 1).min(PIANO_MIDI_HI));

        let pal = theme.extended_palette();
        let (white, black) = (
            lerp_color(pal.background.weak.color, Color::WHITE, 0.5),
            Color::from_rgb(0.1, 0.1, 0.1),
        );
        let (freq_org, freq_ext, time_org, time_ext) = if horizontal {
            (bounds.x, bounds.width, bounds.y, bounds.height)
        } else {
            (bounds.y, bounds.height, bounds.x, bounds.width)
        };

        // Must mirror frequency_at_cursor so keys align with the tooltip.
        let freq_to_px = |f: f32| -> f32 {
            let uv = freq_to_norm(f, nyq, min_f, scale);
            let t = ((uv - uv_range[0]) / (uv_range[1] - uv_range[0])).clamp(0.0, 1.0);
            freq_org + freq_ext * if matches!(rot, 1 | 2) { t } else { 1.0 - t }
        };

        let strip = match overlay {
            PianoRollOverlay::Left => time_org,
            PianoRollOverlay::Right => time_org + time_ext - PIANO_ROLL_WIDTH,
            PianoRollOverlay::Off => return,
        };
        let wborder = iced::Border {
            color: with_alpha(black, 0.4),
            width: 0.5,
            radius: 0.0.into(),
        };
        let black_key_width = PIANO_ROLL_WIDTH * PIANO_BLACK_KEY_RATIO;
        let right = matches!(overlay, PianoRollOverlay::Right);

        let semi = 2.0f32.powf(0.5 / 12.0);
        let (inv_s, whole, inv_w) = (1.0 / semi, semi * semi, 1.0 / (semi * semi));

        let orient_rect = |pos: f32, len: f32, cross: f32, cw: f32| -> Rectangle {
            if horizontal {
                Rectangle::new(Point::new(pos, cross), Size::new(len, cw))
            } else {
                Rectangle::new(Point::new(cross, pos), Size::new(cw, len))
            }
        };
        let orient_point = |fp: f32, tp: f32| -> Point {
            if horizontal {
                Point::new(fp, tp)
            } else {
                Point::new(tp, fp)
            }
        };

        // Key boundaries sit at the midpoint of the intervening black key,
        // or at the semitone midpoint where no black key exists (E-F, B-C).
        let key_extent = |midi: i32, freq: f32, is_blk: bool| -> (f32, f32) {
            let (ml, mh) = if is_blk {
                (inv_s, semi)
            } else {
                match midi % 12 {
                    0 | 5 => (inv_s, whole),
                    4 | 11 => (inv_w, semi),
                    _ => (inv_w, whole),
                }
            };
            let (a, b) = (freq_to_px(freq * mh), freq_to_px(freq * ml));
            if a < b { (a, b) } else { (b, a) }
        };

        for pass in 0..2u8 {
            for midi in midi_lo..=midi_hi {
                let note = MusicalNote::from_midi(midi);
                let is_blk = note.is_black();
                if is_blk != (pass == 1) {
                    continue;
                }
                let (lo, hi) = key_extent(midi, note.to_frequency(), is_blk);
                if hi < freq_org || lo > freq_org + freq_ext {
                    continue;
                }
                let key_len = (hi - lo).max(1.0);
                let (fill, brd, w) = if is_blk {
                    (black, Default::default(), black_key_width)
                } else {
                    (white, wborder, PIANO_ROLL_WIDTH)
                };
                let anchor = if is_blk && right {
                    strip + PIANO_ROLL_WIDTH - black_key_width
                } else {
                    strip
                };
                renderer.fill_quad(
                    Quad {
                        bounds: orient_rect(lo, key_len, anchor, w),
                        border: brd,
                        ..Default::default()
                    },
                    Background::Color(fill),
                );
                if note.midi_number % 12 == 0 && key_len >= PIANO_LABEL_SIZE {
                    let s = format!("C{}", note.octave);
                    let tsz = crate::visuals::measure_text(&s, PIANO_LABEL_SIZE);
                    let fp = lo + (key_len - if horizontal { tsz.width } else { tsz.height }) * 0.5;
                    let tp = strip
                        + (PIANO_ROLL_WIDTH - if horizontal { tsz.height } else { tsz.width })
                            * 0.5;
                    let pt = orient_point(fp, tp);
                    renderer.fill_text(
                        crate::visuals::make_text(&s, PIANO_LABEL_SIZE, tsz),
                        pt,
                        black,
                        Rectangle::new(pt, tsz),
                    );
                }
            }
        }
    }
}

impl<'a, Message> Widget<Message, iced::Theme, iced::Renderer> for Spectrogram<'a> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<InteractionState>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(InteractionState::default())
    }
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(
        &mut self,
        _: &mut Tree,
        _: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(Length::Fill, Length::Fill, Size::ZERO))
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &iced::Event,
        layout: Layout<'_>,
        _: mouse::Cursor,
        _: &iced::Renderer,
        _: &mut dyn iced::advanced::Clipboard,
        shell: &mut iced::advanced::Shell<'_, Message>,
        _: &Rectangle,
    ) {
        let st = tree.state.downcast_mut::<InteractionState>();
        let b = layout.bounds();
        match event {
            iced::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                st.cursor = b.contains(*position).then_some(*position);
                if let Some((origin, start_pan)) = st.drag {
                    let mut state = self.state.borrow_mut();
                    let h = 0.5 / state.zoom;
                    let horiz = state.freq_axis_is_horizontal();
                    let extent = if horiz { b.width } else { b.height };
                    let current = if horiz { position.x } else { position.y };
                    let sign = if matches!(state.rotation_index(), 1 | 2) {
                        -1.0
                    } else {
                        1.0
                    };
                    state.pan = (start_pan + sign * (current - origin) / extent / state.zoom)
                        .clamp(h, 1.0 - h);
                    shell.request_redraw();
                }
            }
            iced::Event::Mouse(mouse::Event::CursorLeft) => st.cursor = None,
            iced::Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => st.modifiers = *m,
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) if st.modifiers.control() => {
                if let Some(pos) = st.cursor.filter(|p| b.contains(*p)) {
                    let freq_norm = self.state.borrow().freq_axis_norm(pos, b).unwrap_or(0.5);
                    let scroll_y = match *delta {
                        mouse::ScrollDelta::Lines { y, .. } => y,
                        mouse::ScrollDelta::Pixels { y, .. } => y / 50.0,
                    };
                    self.state
                        .borrow_mut()
                        .zoom_at(freq_norm, ZOOM_STEP.powf(scroll_y));
                    shell.request_redraw();
                    shell.capture_event();
                }
            }
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Middle)) => {
                let state = self.state.borrow();
                if let Some(pos) = st
                    .cursor
                    .filter(|p| b.contains(*p) && state.zoom > MIN_ZOOM)
                {
                    let origin = if state.freq_axis_is_horizontal() {
                        pos.x
                    } else {
                        pos.y
                    };
                    st.drag = Some((origin, state.pan));
                    shell.capture_event();
                }
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Middle)) => {
                st.drag = None;
            }
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if st.cursor.is_some_and(|p| b.contains(p)) {
                    st.left_held = true;
                    shell.request_redraw();
                }
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if st.left_held {
                    st.left_held = false;
                    shell.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        _: &renderer::Style,
        layout: Layout<'_>,
        _: mouse::Cursor,
        _: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let (uv_y_range, piano_roll, bg, params);
        {
            let mut state = self.state.borrow_mut();
            let (bw, bh) = (
                bounds.width.round().max(1.0) as u32,
                bounds.height.round().max(1.0) as u32,
            );
            let swapped = matches!(state.rotation_index(), 1 | 3);
            let (pw, ph) = if swapped { (bh, bw) } else { (bw, bh) };
            state.view_width = pw;
            state.view_height = ph;
            uv_y_range = state.uv_y_range();
            piano_roll = state.piano_roll_overlay;
            bg = state.style.background;
            params = state.visual_params(bounds, uv_y_range);
        }
        let interaction = tree.state.downcast_ref::<InteractionState>();
        renderer.fill_quad(
            Quad {
                bounds,
                border: Default::default(),
                shadow: Default::default(),
                snap: true,
            },
            Background::Color(bg),
        );
        if let Some(p) = params {
            renderer.draw_primitive(bounds, SpectrogramPrimitive::new(p));
        }
        if piano_roll != PianoRollOverlay::Off {
            renderer.with_layer(bounds, |r| {
                self.draw_piano_roll(r, theme, bounds, piano_roll, uv_y_range);
            });
        }
        if interaction.left_held
            && let Some(c) = interaction.cursor
            && bounds.contains(c)
        {
            renderer.with_layer(bounds, |r| {
                self.draw_crosshair(r, bounds, c);
                self.draw_tooltip(r, theme, bounds, c, uv_y_range);
            });
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _: &Rectangle,
        _: &iced::Renderer,
    ) -> mouse::Interaction {
        let interaction = tree.state.downcast_ref::<InteractionState>();
        if interaction.drag.is_some() {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(layout.bounds()) {
            mouse::Interaction::Crosshair
        } else {
            mouse::Interaction::default()
        }
    }
}

pub(crate) fn widget<'a, Message: 'a>(
    state: &'a RefCell<SpectrogramState>,
) -> Element<'a, Message> {
    Element::new(Spectrogram::new(state))
}
