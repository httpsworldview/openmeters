// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{
    ColumnKind, SpectrogramColumn, SpectrogramConfig, SpectrogramUpdate, history_columns,
};
use super::render::{RingCopyPlan, SPECTROGRAM_PALETTE_SIZE, SpectrogramParams};
use crate::persistence::settings::SpectrogramSettings;
use crate::ui::{scroll_delta_lines, theme};
use crate::util::{
    audio::musical::{MusicalNote, NoteInfo},
    audio::{DB_FLOOR, fmt_duration, fmt_freq, sanitize_negative_db},
    color::{color_to_rgba, lerp_color, rgba_with_alpha, with_alpha},
};
use crate::visuals::options::PianoRollOverlay;
use crate::visuals::palettes;
use crate::visuals::render::common::{fill_bordered_rect, fill_rect, text as raw_text};
use iced::advanced::graphics::text::Paragraph;
use iced::advanced::text::{Paragraph as _, Renderer as _};
use iced::advanced::widget::Tree;
use iced::advanced::{Layout, Renderer as _, Widget, mouse};
use iced::{Color, Element, Length, Point, Rectangle, Size, keyboard};
use iced_wgpu::primitive::Renderer as _;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Arc;

const SPECTROGRAM_DB_CEILING: f32 = 0.0;
const SPECTROGRAM_OPACITY: f32 = 0.95;
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

// Display floor for the frequency axis. Reassignment can localize energy far
// below the FFT bin spacing, so this is intentionally decoupled from fft_size.
// 1 Hz is about as low as the log axis stays useful before stretching swallows
// the whole display; ERB and linear scales handle it cleanly either way.
const DISPLAY_MIN_HZ: f32 = 1.0;

fn display_axis(sample_rate: f32) -> (f32, f32) {
    let nyq = (sample_rate / 2.0).max(1.0);
    (DISPLAY_MIN_HZ.min(nyq * 0.5), nyq)
}

crate::macros::default_struct! {
    struct SpectrogramHistory {
        col_kind: ColumnKind = ColumnKind::Reassigned,
        reassigned_points_per_slot: u32 = 1,
        ring_capacity: u32 = 0,
        write_slot: u32 = 0,
        col_count: u32 = 0,
        slot_counts: Arc<[u32]> = Arc::from([]),
        pending: VecDeque<SpectrogramColumn> = VecDeque::new(),
        pending_copy: Option<RingCopyPlan> = None,
    }
}

impl SpectrogramHistory {
    fn apply_update(&mut self, snap: SpectrogramUpdate) {
        let ppc = snap.fft_size / 2 + 1;
        if ppc == 0 { return; }
        let new_kind = snap
            .new_columns
            .first()
            .map_or(self.col_kind, SpectrogramColumn::kind);
        let capacity = history_columns(new_kind, ppc as u32, snap.history_length) as u32;
        if capacity == 0 { return; }

        if snap.reset {
            *self = Self {
                col_kind: new_kind,
                ring_capacity: capacity,
                slot_counts: if new_kind == ColumnKind::Reassigned {
                    vec![0; capacity as usize].into()
                } else {
                    Arc::from([])
                },
                ..Default::default()
            };
        } else if capacity != self.ring_capacity {
            self.ensure_pending_copy();
            if capacity > self.ring_capacity && self.col_count >= self.ring_capacity {
                self.remap_retained(self.write_slot, self.col_count);
                self.write_slot = self.col_count % capacity;
            } else if capacity < self.ring_capacity && self.col_count >= capacity {
                let oldest_kept =
                    (self.write_slot + self.ring_capacity - capacity) % self.ring_capacity;
                self.remap_retained(oldest_kept, capacity);
                self.col_count = capacity;
                self.write_slot = 0;
            }
            self.ring_capacity = capacity;
            if self.col_kind == ColumnKind::Reassigned
                && self.slot_counts.len() != capacity as usize
            {
                let mut counts = self.slot_counts.to_vec();
                counts.resize(capacity as usize, 0);
                self.slot_counts = counts.into();
            }
        }

        for column in snap.new_columns {
            let slot = self.write_slot;
            if let SpectrogramColumn::Reassigned(points) = &column
                && let Some(count) = Arc::make_mut(&mut self.slot_counts).get_mut(slot as usize)
            {
                *count = points.len() as u32;
            }
            if self.pending.len() as u32 >= self.ring_capacity { self.pending.pop_front(); }
            self.pending.push_back(column);
            self.write_slot = (self.write_slot + 1) % self.ring_capacity;
            if self.col_count < self.ring_capacity { self.col_count += 1; }
        }
        self.fit_reassigned_slot_capacity();
    }

    fn ensure_pending_copy(&mut self) {
        if self.pending_copy.is_none() && self.col_count as usize > self.pending.len() {
            self.pending_copy = Some((0..self.col_count.min(self.ring_capacity)).collect());
        }
    }

    fn fit_reassigned_slot_capacity(&mut self) {
        if self.col_kind != ColumnKind::Reassigned {
            self.reassigned_points_per_slot = 1;
            return;
        }
        let needed = self
            .slot_counts
            .iter()
            .take(self.ring_capacity as usize)
            .copied()
            .fold(1, u32::max);
        let current = self.reassigned_points_per_slot;
        if needed > current || current > needed.saturating_mul(4).max(1) {
            self.ensure_pending_copy();
            self.reassigned_points_per_slot = needed;
        }
    }

    fn remap_retained(&mut self, start: u32, keep: u32) {
        let old_cap = self.ring_capacity.max(1);
        let remap = |slot: &mut u32| {
            if *slot < old_cap {
                *slot = (*slot + old_cap - start) % old_cap;
            }
            *slot < keep
        };
        if self.col_kind == ColumnKind::Reassigned {
            let mut counts = vec![0; keep as usize];
            for (src, &count) in self.slot_counts.iter().enumerate().take(old_cap as usize) {
                let mut dst = src as u32;
                if remap(&mut dst) {
                    counts[dst as usize] = count;
                }
            }
            self.slot_counts = counts.into();
        }
        let discard = self.pending.len().saturating_sub(keep as usize);
        self.pending.drain(..discard);
        if let Some(copies) = &mut self.pending_copy {
            for dst in copies {
                if !remap(dst) { *dst = u32::MAX; }
            }
        }
    }
}

pub(crate) struct SpectrogramState {
    pub(in crate::visuals) palette: [Color; SPECTROGRAM_PALETTE_SIZE],
    pub(in crate::visuals) stop_positions: [f32; SPECTROGRAM_PALETTE_SIZE],
    pub(in crate::visuals) stop_spreads: [f32; SPECTROGRAM_PALETTE_SIZE],
    key: u64,
    pub(in crate::visuals) settings: SpectrogramSettings,
    sample_rate: f32,
    fft_size: usize,
    hop_size: usize,
    reassigned_power_scale: f32,
    zoom: f32,
    pan: f32,
    pub(in crate::visuals) view_width: u32,
    history: SpectrogramHistory,
}

impl SpectrogramState {
    pub fn new() -> Self {
        let cfg = SpectrogramConfig::default();
        Self {
            palette: palettes::spectrogram::COLORS,
            stop_positions: palettes::spectrogram::DEFAULT_POSITIONS,
            stop_spreads: [1.0; SPECTROGRAM_PALETTE_SIZE],
            key: crate::visuals::next_key(),
            settings: SpectrogramSettings {
                floor_db: DB_FLOOR,
                ..SpectrogramSettings::default()
            },
            sample_rate: cfg.sample_rate,
            fft_size: cfg.fft_size * cfg.zero_padding_factor,
            hop_size: cfg.hop_size,
            reassigned_power_scale: 1.0,
            zoom: 1.0,
            pan: 0.5,
            view_width: 0,
            history: SpectrogramHistory::default(),
        }
    }

    crate::visuals::palette_setter!(SPECTROGRAM_PALETTE_SIZE);

    pub fn set_stops(&mut self, positions: &[f32], spreads: &[f32]) {
        if let (Ok(positions), Ok(spreads)) = (positions.try_into(), spreads.try_into()) {
            (self.stop_positions, self.stop_spreads) = (positions, spreads);
        }
    }

    pub fn update_view_settings(&mut self, settings: &SpectrogramSettings) {
        self.settings = settings.clone();
        self.settings.floor_db = sanitize_negative_db(settings.floor_db, DB_FLOOR)
            .min(SPECTROGRAM_DB_CEILING - 1.0);
        self.settings.tilt_db = if settings.tilt_db.is_finite() { settings.tilt_db } else { 0.0 };
        self.settings.rotation = settings.rotation.clamp(-1, 2);
    }

    pub fn reset_audio(&mut self) {
        self.history = SpectrogramHistory::default();
    }

    pub fn apply_snapshot(&mut self, snap: SpectrogramUpdate) {
        self.sample_rate = snap.sample_rate;
        self.fft_size = snap.fft_size;
        self.hop_size = snap.hop_size;
        self.reassigned_power_scale = snap.reassigned_power_scale;
        self.history.apply_update(snap);
    }

    pub fn visual_params(
        &mut self,
        bounds: Rectangle,
        uv_y_range: [f32; 2],
    ) -> Option<SpectrogramParams> {
        let history = &mut self.history;
        if history.col_count == 0 && history.pending.is_empty() { return None; }
        let copy_plan = history.pending_copy.take();
        let slot_counts = Arc::clone(&history.slot_counts);
        let to_rgba = |c: Color| {
            rgba_with_alpha(color_to_rgba(c), c.a * SPECTROGRAM_OPACITY)
        };
        let bin_hz = self.sample_rate / self.fft_size as f32;
        let (freq_min, freq_max) = display_axis(self.sample_rate);

        Some(SpectrogramParams {
            key: self.key,
            bounds,
            ring_capacity: history.ring_capacity,
            points_per_column: (self.fft_size / 2 + 1) as u32,
            reassigned_points_per_slot: history.reassigned_points_per_slot,
            col_count: history.col_count,
            write_slot: history.write_slot,
            pending_uploads: std::mem::take(&mut history.pending),
            copy_plan,
            slot_counts,
            col_kind: history.col_kind,
            freq_min,
            freq_max,
            bin_hz,
            reassigned_power_scale: self.reassigned_power_scale,
            freq_scale: self.settings.frequency_scale,
            palette: self.palette.map(to_rgba),
            stop_positions: self.stop_positions,
            stop_spreads: self.stop_spreads,
            floor_db: self.settings.floor_db,
            tilt_db: self.settings.tilt_db,
            uv_y_range,
            rotation: self.settings.rotation,
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
        let (min_f, nyq) = display_axis(self.sample_rate);
        crate::util::finite_positive(self.settings.frequency_scale.freq_at(min_f, nyq, tex_uv))
    }

    // Normalized rotation (0..3) matching the shader's rotate_uv convention
    fn rotation_index(&self) -> u32 {
        (self.settings.rotation as i32).rem_euclid(4) as u32
    }

    fn freq_axis_is_horizontal(&self) -> bool {
        matches!(self.rotation_index(), 1 | 3)
    }

    // Maps a screen point to the frequency-axis UV (0..1), matching
    // the shader's rotate_uv so CPU-side interactions stay consistent.
    fn freq_axis_norm(&self, cursor: Point, bounds: Rectangle) -> Option<f32> {
        if !bounds.contains(cursor) { return None; }
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
        if !bounds.contains(cursor) || self.history.col_count == 0 {
            return None;
        }
        let age = match self.rotation_index() {
            1 => bounds.y + bounds.height - cursor.y,
            2 => cursor.x - bounds.x,
            3 => cursor.y - bounds.y,
            _ => bounds.x + bounds.width - cursor.x,
        };
        if age < 0.0 || age >= self.history.col_count as f32 { return None; }
        let secs = age * (self.hop_size as f32 / self.sample_rate);
        secs.is_finite().then_some(secs)
    }
}

const MIN_ZOOM: f32 = 1.0;
const MAX_ZOOM: f32 = f32::MAX;
const ZOOM_STEP: f32 = 1.15;

#[derive(Default)]
struct InteractionState {
    modifiers: keyboard::Modifiers,
    drag: Option<(f32, f32)>,
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

struct Spectrogram<'a> {
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

impl Spectrogram<'_> {
    fn draw_crosshair(
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        bounds: Rectangle,
        cursor: Point,
    ) {
        let color = theme::border_color(theme, false);
        for rect in [
            Rectangle::new(
                Point::new(cursor.x, bounds.y),
                Size::new(1.0, bounds.height),
            ),
            Rectangle::new(Point::new(bounds.x, cursor.y), Size::new(bounds.width, 1.0)),
        ] {
            fill_rect(renderer, rect, color);
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
            .map_or_else(|| String::from("--"), NoteInfo::fmt_note_cents);
        let time_text = time_ago.map_or_else(|| String::from("--"), fmt_duration);

        let texts = [freq_text, note_text, time_text];
        let [fsz, nsz, tsz] = texts.each_ref().map(|text| {
            Paragraph::with_text(raw_text(text.as_str(), TOOLTIP_SIZE, Size::INFINITE)).min_bounds()
        });
        let line_h = fsz.height;
        let content_w = fsz.width.max(nsz.width).max(tsz.width);
        let content_h = line_h * 3.0 + TOOLTIP_GAP * 2.0;
        let sz = Size::new(content_w + TOOLTIP_PAD * 2.0, content_h + TOOLTIP_PAD * 2.0);
        let tb = place_tooltip(bounds, cursor, sz, horizontal);

        let pal = theme.extended_palette();
        fill_bordered_rect(
            renderer,
            tb,
            with_alpha(pal.background.strong.color, TOOLTIP_BG_ALPHA),
            iced::Border {
                color: with_alpha(theme::border_color(theme, false), TOOLTIP_BORDER_ALPHA),
                width: 1.0,
                ..Default::default()
            },
            false,
        );

        let text_color = pal.background.base.text;
        let tx = tb.x + TOOLTIP_PAD;
        let mut ty = tb.y + TOOLTIP_PAD;
        for (text, sz) in texts.into_iter().zip([fsz, nsz, tsz]) {
            let pt = Point::new(tx, ty);
            renderer.fill_text(
                raw_text(text, TOOLTIP_SIZE, sz),
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
        let (min_f, nyq) = display_axis(state.sample_rate);
        let (scale, rot) = (state.settings.frequency_scale, state.rotation_index());
        drop(state);
        let horizontal = matches!(rot, 1 | 3);

        let (freq_top, freq_bot) = (
            scale.freq_at(min_f, nyq, uv_range[1]),
            scale.freq_at(min_f, nyq, uv_range[0]),
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
            let uv = scale.pos_of(min_f, nyq, f);
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

        let semi = (0.5_f32 / 12.0).exp2();
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
                    (black, iced::Border::default(), black_key_width)
                } else {
                    (white, wborder, PIANO_ROLL_WIDTH)
                };
                let anchor = if is_blk && right {
                    strip + PIANO_ROLL_WIDTH - black_key_width
                } else {
                    strip
                };
                fill_bordered_rect(renderer, orient_rect(lo, key_len, anchor, w), fill, brd, false);
                if note.midi_number % 12 == 0 && key_len >= PIANO_LABEL_SIZE {
                    let s = format!("C{}", note.octave());
                    let tsz = Paragraph::with_text(raw_text(
                        s.as_str(),
                        PIANO_LABEL_SIZE,
                        Size::INFINITE,
                    ))
                    .min_bounds();
                    let fp = lo + (key_len - if horizontal { tsz.width } else { tsz.height }) * 0.5;
                    let tp = strip
                        + (PIANO_ROLL_WIDTH - if horizontal { tsz.height } else { tsz.width })
                            * 0.5;
                    let pt = orient_point(fp, tp);
                    renderer.fill_text(
                        raw_text(s, PIANO_LABEL_SIZE, tsz),
                        pt,
                        black,
                        Rectangle::new(pt, tsz),
                    );
                }
            }
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for Spectrogram<'_> {
    crate::macros::widget_method!(state InteractionState);
    crate::macros::widget_method!(layout
        Size::new(Length::Fill, Length::Fill),
        |limits| limits.resolve(Length::Fill, Length::Fill, Size::ZERO)
    );

    crate::macros::widget_method!(update Message; this; tree, event, layout, cursor, _, _, shell, _ => {
        let st = tree.state.downcast_mut::<InteractionState>();
        let b = layout.bounds();
        match event {
            iced::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if st.left_held || st.drag.is_some() {
                    shell.request_redraw();
                }
                if let Some((origin, start_pan)) = st.drag {
                    let mut state = this.state.borrow_mut();
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
                }
            }
            iced::Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => st.modifiers = *m,
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) if st.modifiers.control() => {
                if let Some(pos) = cursor.position().filter(|p| b.contains(*p)) {
                    let freq_norm = this.state.borrow().freq_axis_norm(pos, b).unwrap_or(0.5);
                    this.state
                        .borrow_mut()
                        .zoom_at(freq_norm, ZOOM_STEP.powf(scroll_delta_lines(*delta)));
                    shell.request_redraw();
                    shell.capture_event();
                }
            }
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Middle)) => {
                let state = this.state.borrow();
                if let Some(pos) = cursor
                    .position()
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
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                if cursor.position().is_some_and(|p| b.contains(p)) => {
                    st.left_held = true;
                    shell.request_redraw();
                }
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if st.left_held => {
                    st.left_held = false;
                    shell.request_redraw();
                }
            _ => {}
        }
    });

    crate::macros::widget_method!(draw this; tree, renderer, theme, _, layout, cursor, _ => {
        let bounds = layout.bounds();
        let (uv_y_range, piano_roll, params);
        {
            let mut state = this.state.borrow_mut();
            let (bw, bh) = (
                bounds.width.round().max(1.0) as u32,
                bounds.height.round().max(1.0) as u32,
            );
            state.view_width = if matches!(state.rotation_index(), 1 | 3) {
                bh
            } else {
                bw
            };
            uv_y_range = state.uv_y_range();
            piano_roll = state.settings.piano_roll_overlay;
            params = state.visual_params(bounds, uv_y_range);
        }
        let interaction = tree.state.downcast_ref::<InteractionState>();
        if let Some(p) = params {
            renderer.draw_primitive(bounds, p);
        }
        if piano_roll != PianoRollOverlay::Off {
            renderer.with_layer(bounds, |r| {
                this.draw_piano_roll(r, theme, bounds, piano_roll, uv_y_range);
            });
        }
        if interaction.left_held
            && let Some(c) = cursor.position().filter(|p| bounds.contains(*p))
        {
            renderer.with_layer(bounds, |r| {
                Self::draw_crosshair(r, theme, bounds, c);
                this.draw_tooltip(r, theme, bounds, c, uv_y_range);
            });
        }
    });

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

pub(in crate::visuals) fn widget<'a, Message: 'a>(
    state: &'a RefCell<SpectrogramState>,
) -> Element<'a, Message> {
    Element::new(Spectrogram { state })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update<T: Copy>(
        history_length: usize,
        reset: bool,
        values: &[T],
        column: impl Fn(T) -> SpectrogramColumn,
    ) -> SpectrogramUpdate {
        let new_columns: Vec<_> = values.iter().copied().map(column).collect();
        let (points_per_column, reassigned_power_scale) = match new_columns.first().map(SpectrogramColumn::kind) {
            Some(ColumnKind::Reassigned) => (8, 0.25),
            _ => (2, 1.0),
        };
        SpectrogramUpdate {
            fft_size: (points_per_column - 1) * 2,
            hop_size: 1,
            sample_rate: 48_000.0,
            history_length,
            reset,
            reassigned_power_scale,
            new_columns,
        }
    }

    fn classic_update(history_length: usize, reset: bool, values: &[f32]) -> SpectrogramUpdate {
        update(history_length, reset, values, |value| {
            SpectrogramColumn::Classic(vec![super::super::processor::pack_classic_db(value); 2])
        })
    }

    fn reassigned_update(history_length: usize, reset: bool, counts: &[usize]) -> SpectrogramUpdate {
        let point = super::super::processor::SpectrogramPoint {
            time_offset: 0.0,
            freq_hz: 100.0,
            power: 0.01,
        };
        update(history_length, reset, counts, |count| {
            SpectrogramColumn::Reassigned(vec![point; count])
        })
    }

    fn visual_params(state: &mut SpectrogramState) -> SpectrogramParams {
        state
            .visual_params(
                Rectangle::new(Point::new(0.0, 0.0), Size::new(10.0, 10.0)),
                [0.0, 1.0],
            )
            .expect("expected spectrogram params")
    }

    fn upload_slots(params: &SpectrogramParams) -> Vec<u32> {
        let count = params.pending_uploads.len() as u32;
        (0..count)
            .map(|offset| (params.write_slot + params.ring_capacity - count + offset) % params.ring_capacity)
            .collect()
    }

    fn seeded_ring() -> SpectrogramState {
        let mut state = SpectrogramState::new();
        state.apply_snapshot(classic_update(4, true, &[0.0, 1.0, 2.0, 3.0]));
        assert_eq!(upload_slots(&visual_params(&mut state)), vec![0, 1, 2, 3]);
        state
    }

    #[test]
    fn resize_copy_plans_preserve_visible_columns() {
        let mut state = seeded_ring();
        state.apply_snapshot(classic_update(4, false, &[4.0, 5.0]));
        assert_eq!(upload_slots(&visual_params(&mut state)), vec![0, 1]);

        state.apply_snapshot(classic_update(6, false, &[6.0]));
        let params = visual_params(&mut state);
        assert_eq!((params.ring_capacity, params.col_count, params.write_slot), (6, 5, 5));
        assert!(params.slot_counts.is_empty());
        assert_eq!(upload_slots(&params), vec![4]);
        assert_eq!(params.copy_plan, Some(vec![2, 3, 0, 1]));

        let mut state = seeded_ring();
        state.apply_snapshot(classic_update(2, false, &[4.0]));
        let params = visual_params(&mut state);
        assert_eq!((params.ring_capacity, params.col_count, params.write_slot), (2, 2, 1));
        assert_eq!(upload_slots(&params), vec![0]);
        assert_eq!(params.copy_plan, Some(vec![u32::MAX, u32::MAX, 0, 1]));
    }

    #[test]
    fn reassigned_params_track_sparse_slot_counts() {
        let mut state = SpectrogramState::new();
        state.apply_snapshot(reassigned_update(4, true, &[0, 2, 1]));

        let params = visual_params(&mut state);

        assert_eq!(params.reassigned_points_per_slot, 2);
        assert_eq!(params.reassigned_power_scale, 0.25);
        assert_eq!(&params.slot_counts[..4], &[0, 2, 1, 0]);
        assert_eq!(
            params
                .pending_uploads
                .iter()
                .map(|upload| match upload {
                    SpectrogramColumn::Reassigned(points) => points.len(),
                    SpectrogramColumn::Classic(_) => panic!("expected reassigned upload"),
                })
                .collect::<Vec<_>>(),
            vec![0, 2, 1]
        );
    }
}
