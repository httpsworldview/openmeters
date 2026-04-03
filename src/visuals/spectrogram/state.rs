// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{
    FrequencyScale, SpectrogramColumn, SpectrogramConfig,
    SpectrogramProcessor as CoreSpectrogramProcessor, SpectrogramUpdate,
};
use super::render::{
    ColumnBuffer, ColumnBufferPool, SPECTROGRAM_PALETTE_SIZE, SpectrogramColumnUpdate,
    SpectrogramParams, SpectrogramPrimitive,
};
use crate::persistence::settings::PianoRollOverlay;
use crate::util::audio::musical::MusicalNote;
use crate::util::audio::{DB_FLOOR, DEFAULT_SAMPLE_RATE};
use crate::util::color;
use crate::vis_processor;
use crate::visuals::palettes;
use iced::advanced::renderer::{self, Quad};
use iced::advanced::text::Renderer as _;
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Layout, Renderer as _, Widget, layout, mouse};
use iced::{Background, Color, Element, Length, Point, Rectangle, Size, keyboard};
use iced_wgpu::primitive::Renderer as _;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Arc;

const DB_CEILING: f32 = 0.0;
const MAX_TEXTURE_BINS: usize = 8_192;
const MAX_TEXTURE_COLS: usize = 8_192;
const TOOLTIP_SIZE: f32 = 14.0;
const TOOLTIP_PAD: f32 = 8.0;
const PIANO_ROLL_WIDTH: f32 = 18.0;
const PIANO_BLACK_KEY_RATIO: f32 = 0.6;
const PIANO_LABEL_SIZE: f32 = 9.0;
const PIANO_MIDI_LO: i32 = 21; // A0
const PIANO_MIDI_HI: i32 = 119; // C8

/// Interpolated position of `f` within a descending slice, normalised to `[0, 1)`.
fn search_descending(freqs: &[f32], f: f32) -> f32 {
    let n = freqs.len();
    if n < 2 {
        return 0.5;
    }
    let i = freqs.partition_point(|&x| x > f);
    if i == 0 {
        return 0.0;
    }
    if i >= n {
        return (n - 1) as f32 / n as f32;
    }
    let (hi, lo) = (freqs[i - 1], freqs[i]);
    let t = if (hi - lo).abs() > f32::EPSILON {
        (hi - f) / (hi - lo)
    } else {
        0.5
    };
    ((i - 1) as f32 + t) / n as f32
}

fn spec_freq_min(scale: FrequencyScale, min_freq: f32) -> f32 {
    if matches!(scale, FrequencyScale::Linear) {
        0.0
    } else {
        min_freq
    }
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
    pub floor_db: f32,
    pub ceiling_db: f32,
    pub opacity: f32,
    pub contrast: f32,
    pub tilt_db: f32,
}

impl Default for SpectrogramStyle {
    fn default() -> Self {
        Self {
            background: color::with_alpha(palettes::BG_BASE, 0.0),
            floor_db: DB_FLOOR,
            ceiling_db: DB_CEILING,
            opacity: 0.95,
            contrast: 1.0,
            tilt_db: 0.0,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct BinMapping {
    lower: Vec<usize>,
    weight: Vec<f32>,
}

impl BinMapping {
    fn new(
        height: usize,
        fft_size: usize,
        sample_rate: f32,
        scale: FrequencyScale,
        passthrough: bool,
    ) -> Self {
        if height == 0 {
            return Self::default();
        }
        if passthrough {
            return Self {
                lower: (0..height).collect(),
                weight: vec![0.0; height],
            };
        }
        let (max_bin, denom) = ((fft_size / 2) as f32, (height - 1).max(1) as f32);
        let (nyq, min_f) = (
            (sample_rate / 2.0).max(1.0),
            (sample_rate / fft_size as f32).max(20.0),
        );
        let mut res = Self {
            lower: Vec::with_capacity(height),
            weight: Vec::with_capacity(height),
        };
        for row in 0..height {
            let pos = ((norm_to_freq(1.0 - row as f32 / denom, nyq, min_f, scale)
                * fft_size as f32)
                / sample_rate)
                .clamp(0.0, max_bin);
            res.lower.push(pos.floor() as usize);
            res.weight.push(pos - pos.floor());
        }
        res
    }
}

#[derive(Clone, Debug, Default)]
struct SpectrogramBuffer {
    values: Vec<f32>,
    capacity: u32,
    height: u32,
    write_idx: u32,
    col_count: u32,
    pending_base: Option<Arc<[f32]>>,
    pending_cols: Vec<SpectrogramColumnUpdate>,
    mapping: BinMapping,
    display_freqs: Arc<[f32]>,
    tilt_offsets: Option<Arc<[f32]>>,
    pool: ColumnBufferPool,
}

impl SpectrogramBuffer {
    fn new() -> Self {
        Self::default()
    }

    fn rebuild(
        &mut self,
        history: &VecDeque<SpectrogramColumn>,
        upd: &SpectrogramUpdate,
        style: &SpectrogramStyle,
    ) {
        (self.pending_base, self.pending_cols) = (None, Vec::new());
        self.capacity = (upd.history_length as u32).min(MAX_TEXTURE_COLS as u32);
        self.height = (upd.display_height as u32).min(MAX_TEXTURE_BINS as u32);
        if self.capacity == 0 || self.height == 0 {
            (self.values, self.write_idx, self.col_count, self.mapping) =
                (vec![], 0, 0, BinMapping::default());
            return;
        }
        let passthrough = upd.display_bins_hz.as_ref().is_some_and(|b| {
            !b.is_empty() && history.iter().all(|c| c.magnitudes_db.len() == b.len())
        });
        self.update_mapping(upd, passthrough, style.tilt_db);
        self.values = vec![0.0; self.capacity as usize * self.height as usize];
        (self.write_idx, self.col_count) = (0, 0);
        for col in history {
            if col.magnitudes_db.len() >= 2 {
                self.push_column(&col.magnitudes_db);
            }
        }
        if self.col_count > 0 {
            self.pending_base = Some(Arc::from(self.values.clone()));
        }
    }

    fn append(&mut self, columns: &[SpectrogramColumn]) {
        if self.capacity == 0 || self.height == 0 {
            return;
        }
        let h = self.height as usize;
        for col in columns.iter().filter(|c| c.magnitudes_db.len() >= 2) {
            let idx = self.push_column(&col.magnitudes_db);
            let start = idx as usize * h;
            let mut buf = self.pool.acquire(h);
            buf.copy_from_slice(&self.values[start..start + h]);
            self.pending_cols.push(SpectrogramColumnUpdate {
                column_index: idx,
                values: Arc::new(ColumnBuffer::new(buf, self.pool.clone())),
            });
        }
        if self.pending_cols.len() as u32 >= (self.capacity / 2).max(16) {
            self.pending_cols.clear();
            self.pending_base = Some(Arc::from(self.values.clone()));
        }
    }

    fn push_column(&mut self, mags: &[f32]) -> u32 {
        let (h, col, n) = (self.height as usize, self.write_idx, mags.len());
        let out = &mut self.values[col as usize * h..(col as usize + 1) * h];
        for (i, v) in out.iter_mut().enumerate() {
            let lo = self.mapping.lower[i].min(n - 1);
            let hi = (self.mapping.lower[i] + 1).min(n - 1);
            *v = mags[lo] + self.mapping.weight[i] * (mags[hi] - mags[lo]);
        }
        if self.col_count < self.capacity {
            self.col_count += 1;
        }
        self.write_idx = (self.write_idx + 1) % self.capacity;
        col
    }

    fn update_mapping(&mut self, upd: &SpectrogramUpdate, passthrough: bool, tilt_db: f32) {
        let h = self.height as usize;
        self.display_freqs = upd
            .display_bins_hz
            .clone()
            .filter(|b| passthrough && b.len() >= h)
            .map(|b| Arc::from(&b[..h]))
            .unwrap_or_else(|| Arc::from([]));
        self.mapping = BinMapping::new(
            h,
            upd.fft_size,
            upd.sample_rate,
            upd.frequency_scale,
            passthrough,
        );
        self.recompute_tilt(upd.fft_size, upd.sample_rate, tilt_db);
    }

    fn recompute_tilt(&mut self, fft_size: usize, sample_rate: f32, tilt_db: f32) {
        let h = self.height as usize;
        if h == 0 || tilt_db.abs() < f32::EPSILON {
            self.tilt_offsets = None;
            return;
        }
        let bin_hz = if fft_size > 0 && sample_rate > 0.0 {
            sample_rate / fft_size as f32
        } else {
            1.0
        };
        let has_freqs = !self.display_freqs.is_empty();
        let mut offsets = vec![0.0_f32; h];
        for (i, offset) in offsets.iter_mut().enumerate() {
            let freq = if has_freqs {
                self.display_freqs.get(i).copied().unwrap_or(1.0)
            } else {
                (self.mapping.lower[i] as f32 + self.mapping.weight[i]) * bin_hz
            };
            *offset = tilt_db * (freq / 1000.0).max(1e-6).log10();
        }
        self.tilt_offsets = Some(Arc::from(offsets));
    }

    fn needs_rebuild(&self, upd: &SpectrogramUpdate) -> bool {
        let dw = (upd.history_length as u32).min(MAX_TEXTURE_COLS as u32);
        let dh = (upd.display_height as u32).min(MAX_TEXTURE_BINS as u32);
        self.capacity == 0
            || self.height == 0
            || self.capacity != dw
            || (dh > 0 && dh != self.height)
    }

    fn latest_column(&self) -> u32 {
        if self.col_count == 0 {
            0
        } else {
            (self.write_idx + self.capacity - 1) % self.capacity
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SpectrogramState {
    buffer: RefCell<SpectrogramBuffer>,
    style: SpectrogramStyle,
    palette: [Color; SPECTROGRAM_PALETTE_SIZE],
    stop_positions: [f32; SPECTROGRAM_PALETTE_SIZE],
    stop_spreads: [f32; SPECTROGRAM_PALETTE_SIZE],
    history: VecDeque<SpectrogramColumn>,
    key: u64,
    pub(crate) piano_roll_overlay: PianoRollOverlay,
    rotation: i8,
    sample_rate: f32,
    fft_size: usize,
    freq_scale: FrequencyScale,
    zoom: f32,
    pan: f32,
    view_width: u32,
    view_height: u32,
}

impl SpectrogramState {
    pub fn new() -> Self {
        Self {
            buffer: RefCell::new(SpectrogramBuffer::new()),
            style: SpectrogramStyle::default(),
            palette: palettes::spectrogram::COLORS,
            stop_positions: std::array::from_fn(|i| {
                i as f32 / (SPECTROGRAM_PALETTE_SIZE - 1) as f32
            }),
            stop_spreads: [1.0; SPECTROGRAM_PALETTE_SIZE],
            history: VecDeque::new(),
            key: crate::visuals::next_key(),
            piano_roll_overlay: PianoRollOverlay::Off,
            rotation: 0,
            sample_rate: DEFAULT_SAMPLE_RATE,
            fft_size: 4096,
            freq_scale: FrequencyScale::default(),
            zoom: 1.0,
            pan: 0.5,
            view_width: 0,
            view_height: 0,
        }
    }

    pub fn set_palette(&mut self, palette: &[Color; SPECTROGRAM_PALETTE_SIZE]) {
        if !color::palettes_equal(&self.palette, palette) {
            self.palette = *palette;
        }
    }

    pub fn set_stop_positions(&mut self, positions: &[f32]) {
        if let Ok(arr) = <[f32; SPECTROGRAM_PALETTE_SIZE]>::try_from(positions) {
            self.stop_positions = arr;
        }
    }

    pub fn stop_positions(&self) -> &[f32; SPECTROGRAM_PALETTE_SIZE] {
        &self.stop_positions
    }

    pub fn set_stop_spreads(&mut self, spreads: &[f32]) {
        if let Ok(arr) = <[f32; SPECTROGRAM_PALETTE_SIZE]>::try_from(spreads) {
            self.stop_spreads = arr;
        }
    }

    pub fn stop_spreads(&self) -> &[f32; SPECTROGRAM_PALETTE_SIZE] {
        &self.stop_spreads
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

    pub fn palette(&self) -> &[Color; SPECTROGRAM_PALETTE_SIZE] {
        &self.palette
    }

    pub fn floor_db(&self) -> f32 {
        self.style.floor_db
    }

    pub fn set_tilt_db(&mut self, tilt_db: f32) {
        let tilt = if tilt_db.is_finite() { tilt_db } else { 0.0 };
        if (self.style.tilt_db - tilt).abs() > f32::EPSILON {
            self.style.tilt_db = tilt;
            self.buffer
                .borrow_mut()
                .recompute_tilt(self.fft_size, self.sample_rate, tilt);
        }
    }

    pub fn tilt_db(&self) -> f32 {
        self.style.tilt_db
    }

    pub fn set_rotation(&mut self, rotation: i8) {
        self.rotation = rotation.clamp(-1, 2);
    }

    pub fn rotation(&self) -> i8 {
        self.rotation
    }

    pub fn view_dimensions(&self) -> (u32, u32) {
        (self.view_width, self.view_height)
    }

    pub fn apply_snapshot(&mut self, snap: &SpectrogramUpdate) {
        if snap.new_columns.is_empty() && !snap.reset {
            return;
        }
        self.sample_rate = snap.sample_rate;
        self.fft_size = snap.fft_size;
        self.freq_scale = snap.frequency_scale;

        self.history.extend(snap.new_columns.iter().cloned());
        if self.history.len() > snap.history_length {
            self.history
                .drain(0..self.history.len() - snap.history_length);
        }

        // Purge history if source column bin count changed (e.g. reassignment toggled,
        // fft_size changed). Compare new columns against existing history.
        if let Some(new_len) = snap
            .new_columns
            .iter()
            .map(|c| c.magnitudes_db.len())
            .find(|&l| l > 0)
        {
            let old_len = self
                .history
                .iter()
                .rev()
                .skip(snap.new_columns.len())
                .map(|c| c.magnitudes_db.len())
                .find(|&l| l > 0);
            if old_len.is_some_and(|ol| ol != new_len) {
                self.history.retain(|c| c.magnitudes_db.len() == new_len);
            }
        }

        let mut buf = self.buffer.borrow_mut();
        if snap.reset || buf.needs_rebuild(snap) {
            buf.rebuild(&self.history, snap, &self.style);
        } else if !snap.new_columns.is_empty() {
            buf.append(&snap.new_columns);
        }
    }

    pub fn visual_params(
        &self,
        bounds: Rectangle,
        uv_y_range: [f32; 2],
    ) -> Option<SpectrogramParams> {
        let buf = self.buffer.borrow();
        if buf.capacity == 0 || buf.height == 0 || buf.col_count == 0 {
            return None;
        }
        let op = self.style.opacity.clamp(0.0, 1.0);
        let to_rgba = |c: Color| [c.r, c.g, c.b, c.a * op];
        Some(SpectrogramParams {
            key: self.key,
            bounds,
            texture_width: buf.capacity,
            texture_height: buf.height,
            column_count: buf.col_count,
            latest_column: buf.latest_column(),
            base_data: buf.pending_base.clone(),
            column_updates: buf.pending_cols.clone(),
            palette: self.palette.map(to_rgba),
            stop_positions: self.stop_positions,
            stop_spreads: self.stop_spreads,
            background: to_rgba(self.style.background),
            contrast: self.style.contrast,
            floor_db: self.style.floor_db,
            ceiling_db: self.style.ceiling_db,
            tilt_offsets: buf.tilt_offsets.clone(),
            uv_y_range,
            rotation: self.rotation,
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
        let buf = self.buffer.borrow();
        if !buf.display_freqs.is_empty() {
            return buf
                .display_freqs
                .get((tex_uv * buf.display_freqs.len() as f32) as usize)
                .copied();
        }
        drop(buf);
        if self.fft_size == 0 || self.sample_rate <= 0.0 {
            return None;
        }
        let (nyq, min_f) = (
            (self.sample_rate / 2.0).max(1.0),
            (self.sample_rate / self.fft_size as f32).max(20.0),
        );
        let freq = norm_to_freq(1.0 - tex_uv, nyq, min_f, self.freq_scale);
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
            1 => 1.0 - (cursor.x - bounds.x) / bounds.width,
            2 => 1.0 - (cursor.y - bounds.y) / bounds.height,
            3 => (cursor.x - bounds.x) / bounds.width,
            _ => (cursor.y - bounds.y) / bounds.height,
        };
        norm.is_finite().then(|| norm.clamp(0.0, 1.0))
    }

    fn clear_pending(&self) {
        let mut buf = self.buffer.borrow_mut();
        (buf.pending_base, buf.pending_cols) = (None, vec![]);
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

impl<'a> Spectrogram<'a> {
    pub fn new(state: &'a RefCell<SpectrogramState>) -> Self {
        Self { state }
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
        drop(state);
        let content = MusicalNote::format_with_hz(freq);

        let tsz = crate::visuals::measure_text(&content, TOOLTIP_SIZE);

        let sz = Size::new(
            tsz.width + TOOLTIP_PAD * 2.0,
            tsz.height + TOOLTIP_PAD * 2.0,
        );
        let max_x = (bounds.x + bounds.width - sz.width).max(bounds.x);
        let max_y = (bounds.y + bounds.height - sz.height).max(bounds.y);

        let (x, y) = if horizontal {
            let x = (cursor.x - sz.width * 0.5).clamp(bounds.x, max_x);
            let y = if cursor.y - 12.0 - sz.height >= bounds.y {
                cursor.y - 12.0 - sz.height
            } else {
                (cursor.y + 12.0).min(max_y)
            };
            (x, y)
        } else {
            let x = if cursor.x + 12.0 + sz.width <= bounds.x + bounds.width {
                cursor.x + 12.0
            } else {
                (cursor.x - 12.0 - sz.width).max(bounds.x)
            };
            let y = (cursor.y - sz.height * 0.5).clamp(bounds.y, max_y);
            (x, y)
        };
        let tb = Rectangle::new(Point::new(x, y), sz);

        let pal = theme.extended_palette();
        renderer.fill_quad(
            Quad {
                bounds: Rectangle::new(Point::new(tb.x + 1.0, tb.y + 1.0), sz),
                border: Default::default(),
                ..Default::default()
            },
            Background::Color(color::with_alpha(pal.background.base.color, 0.3)),
        );
        renderer.fill_quad(
            Quad {
                bounds: tb,
                border: iced::Border {
                    color: crate::ui::theme::BORDER_SUBTLE,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            },
            Background::Color(pal.background.strong.color),
        );

        renderer.fill_text(
            crate::visuals::make_text(&content, TOOLTIP_SIZE, tsz),
            Point::new(tb.x + TOOLTIP_PAD, tb.y + TOOLTIP_PAD),
            pal.background.base.text,
            Rectangle::new(Point::new(tb.x + TOOLTIP_PAD, tb.y + TOOLTIP_PAD), tsz),
        );
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
            (state.sample_rate / state.fft_size as f32).max(20.0),
            state.freq_scale,
            state.rotation_index(),
        );
        let dfreqs = state.buffer.borrow().display_freqs.clone();
        drop(state);
        let horizontal = matches!(rot, 1 | 3);

        let (freq_top, freq_bot) = (
            norm_to_freq(1.0 - uv_range[0], nyq, min_f, scale),
            norm_to_freq(1.0 - uv_range[1], nyq, min_f, scale),
        );
        let midi_lo = MusicalNote::from_frequency(freq_bot.max(16.0))
            .map(|n| (n.midi_number - 1).max(PIANO_MIDI_LO))
            .unwrap_or(PIANO_MIDI_LO);
        let midi_hi = MusicalNote::from_frequency(freq_top)
            .map(|n| (n.midi_number + 1).min(PIANO_MIDI_HI))
            .unwrap_or(PIANO_MIDI_HI);

        let pal = theme.extended_palette();
        let (white, black) = (
            color::mix_colors(pal.background.weak.color, Color::WHITE, 0.5),
            Color::from_rgb(0.1, 0.1, 0.1),
        );
        let (freq_org, freq_ext, time_org, time_ext) = if horizontal {
            (bounds.x, bounds.width, bounds.y, bounds.height)
        } else {
            (bounds.y, bounds.height, bounds.x, bounds.width)
        };

        // Must mirror frequency_at_cursor so keys align with the tooltip.
        let freq_to_px = |f: f32| -> f32 {
            let uv = if !dfreqs.is_empty() {
                search_descending(&dfreqs, f)
            } else {
                1.0 - freq_to_norm(f, nyq, min_f, scale)
            };
            let t = ((uv - uv_range[0]) / (uv_range[1] - uv_range[0])).clamp(0.0, 1.0);
            freq_org + freq_ext * if matches!(rot, 1 | 2) { 1.0 - t } else { t }
        };

        let strip = match overlay {
            PianoRollOverlay::Left => time_org,
            PianoRollOverlay::Right => time_org + time_ext - PIANO_ROLL_WIDTH,
            PianoRollOverlay::Off => return,
        };
        let wborder = iced::Border {
            color: color::with_alpha(black, 0.4),
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
                let Some(note) = MusicalNote::from_midi(midi) else {
                    continue;
                };
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
                        1.0
                    } else {
                        -1.0
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
                st.drag = None
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
        {
            let mut state_mut = self.state.borrow_mut();
            let (bw, bh) = (
                bounds.width.round().max(1.0) as u32,
                bounds.height.round().max(1.0) as u32,
            );
            let swapped = matches!(state_mut.rotation_index(), 1 | 3);
            let (pw, ph) = if swapped { (bh, bw) } else { (bw, bh) };
            if state_mut.view_width != pw || state_mut.view_height != ph {
                state_mut.view_width = pw;
                state_mut.view_height = ph;
            }
            drop(state_mut);
        }
        let interaction = tree.state.downcast_ref::<InteractionState>();
        let state = self.state.borrow();
        let uv_y_range = state.uv_y_range();
        renderer.fill_quad(
            Quad {
                bounds,
                border: Default::default(),
                shadow: Default::default(),
                snap: true,
            },
            Background::Color(state.style.background),
        );
        if let Some(p) = state.visual_params(bounds, uv_y_range) {
            renderer.draw_primitive(bounds, SpectrogramPrimitive::new(p));
        }
        let piano_roll = state.piano_roll_overlay;
        state.clear_pending();
        drop(state);
        if piano_roll != PianoRollOverlay::Off {
            renderer.with_layer(bounds, |r| {
                self.draw_piano_roll(r, theme, bounds, piano_roll, uv_y_range)
            });
        }
        if let Some(c) = interaction.cursor
            && bounds.contains(c)
        {
            renderer.with_layer(bounds, |r| {
                self.draw_tooltip(r, theme, bounds, c, uv_y_range)
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bin_mapping_log_profile() {
        let fft = 2048;
        let rate = DEFAULT_SAMPLE_RATE;
        let height = 256;
        let m = BinMapping::new(height, fft, rate, FrequencyScale::Logarithmic, false);
        assert_eq!(m.lower.len(), height);

        assert!(
            m.lower[0] > m.lower[height - 1],
            "top bin {} must exceed bottom bin {}",
            m.lower[0],
            m.lower[height - 1]
        );

        let mid = height / 2;
        let top_span = m.lower[0] - m.lower[mid];
        let bot_span = m.lower[mid] - m.lower[height - 1];
        assert!(
            top_span > bot_span * 2,
            "log scale: top span ({top_span}) should dwarf bottom span ({bot_span})"
        );

        for i in 1..height {
            let prev = m.lower[i - 1] as f32 + m.weight[i - 1];
            let curr = m.lower[i] as f32 + m.weight[i];
            assert!(
                prev >= curr,
                "row {i}: freq {curr:.2} exceeds row {} freq {prev:.2}",
                i - 1
            );
        }
    }
}
