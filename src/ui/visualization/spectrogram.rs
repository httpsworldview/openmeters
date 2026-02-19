use crate::dsp::spectrogram::{
    FrequencyScale, SpectrogramColumn, SpectrogramConfig,
    SpectrogramProcessor as CoreSpectrogramProcessor, SpectrogramUpdate,
};
use crate::ui::render::spectrogram::{
    ColumnBuffer, ColumnBufferPool, SPECTROGRAM_PALETTE_SIZE, SpectrogramColumnUpdate,
    SpectrogramParams, SpectrogramPrimitive,
};
use crate::ui::settings::PianoRollOverlay;
use crate::ui::theme;
use crate::util::audio::musical::MusicalNote;
use crate::util::audio::{DB_FLOOR, DEFAULT_SAMPLE_RATE};
use crate::vis_processor;
use iced::advanced::renderer::{self, Quad};
use iced::advanced::text::Renderer as _;
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Layout, Renderer as _, Widget, layout, mouse};
use iced::{Background, Color, Element, Length, Point, Rectangle, Size, keyboard};
use iced_wgpu::primitive::Renderer as _;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

const DB_CEILING: f32 = 0.0;
const MAX_TEXTURE_BINS: usize = 8_192;
const TOOLTIP_SIZE: f32 = 14.0;
const TOOLTIP_PAD: f32 = 8.0;
const PIANO_ROLL_WIDTH: f32 = 18.0;

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
}

impl Default for SpectrogramStyle {
    fn default() -> Self {
        Self {
            background: theme::with_alpha(theme::BG_BASE, 0.0),
            floor_db: DB_FLOOR,
            ceiling_db: DB_CEILING,
            opacity: 0.95,
            contrast: 1.4,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct BinMapping {
    lower: Vec<usize>,
    upper: Vec<usize>,
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
            let idx: Vec<usize> = (0..height).collect();
            return Self {
                lower: idx.clone(),
                upper: idx,
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
            upper: Vec::with_capacity(height),
            weight: Vec::with_capacity(height),
        };
        for row in 0..height {
            let pos = ((norm_to_freq(1.0 - row as f32 / denom, nyq, min_f, scale)
                * fft_size as f32)
                / sample_rate)
                .clamp(0.0, max_bin);
            let lo = pos.floor() as usize;
            res.lower.push(lo);
            res.upper.push((lo + 1).min(fft_size / 2));
            res.weight.push(pos - lo as f32);
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
    pool: ColumnBufferPool,
    last_col_time: Option<Instant>,
    col_interval_secs: f32,
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
        self.capacity = upd.history_length as u32;
        self.height = history
            .iter()
            .map(|c| c.magnitudes_db.len().min(MAX_TEXTURE_BINS) as u32)
            .find(|&h| h > 0)
            .unwrap_or(0);
        if self.capacity == 0 || self.height == 0 {
            (self.values, self.write_idx, self.col_count, self.mapping) =
                (vec![], 0, 0, BinMapping::default());
            self.last_col_time = None;
            return;
        }
        let passthrough = upd.display_bins_hz.as_ref().is_some_and(|b| {
            !b.is_empty() && history.iter().all(|c| c.magnitudes_db.len() == b.len())
        });
        self.update_mapping(upd, passthrough);
        self.values = vec![0.0; self.capacity as usize * self.height as usize];
        (self.write_idx, self.col_count) = (0, 0);
        self.last_col_time = None;
        let h = self.height as usize;
        for col in history {
            if col.magnitudes_db.len() >= h {
                self.push_column(&col.magnitudes_db, style);
            }
        }
        if self.col_count > 0 {
            self.pending_base = Some(Arc::from(self.values.clone()));
            self.last_col_time = Some(Instant::now());
        }
    }

    fn append(&mut self, columns: &[SpectrogramColumn], style: &SpectrogramStyle) {
        if self.capacity == 0 || self.height == 0 {
            return;
        }
        let h = self.height as usize;
        let now = Instant::now();
        let new_cols = columns
            .iter()
            .filter(|c| c.magnitudes_db.len() >= h)
            .count();
        for col in columns.iter().filter(|c| c.magnitudes_db.len() >= h) {
            let idx = self.push_column(&col.magnitudes_db, style);
            let start = idx as usize * h;
            let mut buf = self.pool.acquire(h);
            buf.copy_from_slice(&self.values[start..start + h]);
            self.pending_cols.push(SpectrogramColumnUpdate {
                column_index: idx,
                values: Arc::new(ColumnBuffer::new(buf, self.pool.clone())),
            });
        }
        if new_cols > 0 {
            if let Some(last) = self.last_col_time {
                let elapsed = now.duration_since(last).as_secs_f32();
                let interval = elapsed / new_cols as f32;
                self.col_interval_secs = if self.col_interval_secs > 0.0 {
                    self.col_interval_secs * 0.8 + interval * 0.2
                } else {
                    interval
                };
            }
            self.last_col_time = Some(now);
        }
        if self.pending_cols.len() as u32 >= (self.capacity / 2).max(16) {
            self.pending_cols.clear();
            self.pending_base = Some(Arc::from(self.values.clone()));
        }
    }

    fn push_column(&mut self, mags: &[f32], style: &SpectrogramStyle) -> u32 {
        let (h, col, n) = (self.height as usize, self.write_idx, mags.len());
        let inv = 1.0 / (style.ceiling_db - style.floor_db).max(f32::EPSILON);
        let out = &mut self.values[col as usize * h..(col as usize + 1) * h];
        for (i, v) in out.iter_mut().enumerate() {
            let lo = self.mapping.lower[i].min(n - 1);
            let hi = self.mapping.upper[i].min(n - 1);
            let val = mags[lo] + self.mapping.weight[i] * (mags[hi] - mags[lo]);
            *v = (val.clamp(style.floor_db, style.ceiling_db) - style.floor_db) * inv;
        }
        if self.col_count < self.capacity {
            self.col_count += 1;
        }
        self.write_idx = (self.write_idx + 1) % self.capacity;
        col
    }

    fn update_mapping(&mut self, upd: &SpectrogramUpdate, passthrough: bool) {
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
    }

    fn needs_rebuild(&self, upd: &SpectrogramUpdate, new_height: Option<u32>) -> bool {
        self.capacity == 0
            || self.height == 0
            || self.capacity != upd.history_length as u32
            || new_height.is_some_and(|h| h > 0 && h != self.height)
    }

    fn latest_column(&self) -> u32 {
        if self.col_count == 0 {
            0
        } else {
            (self.write_idx + self.capacity - 1) % self.capacity
        }
    }

    fn scroll_phase(&self) -> f32 {
        let Some(last) = self.last_col_time else {
            return 0.0;
        };
        if self.col_interval_secs <= 0.0 {
            return 0.0;
        }
        let phase = Instant::now().duration_since(last).as_secs_f32() / self.col_interval_secs;
        if phase >= 1.0 { 0.0 } else { phase }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SpectrogramState {
    buffer: RefCell<SpectrogramBuffer>,
    style: SpectrogramStyle,
    palette: [Color; SPECTROGRAM_PALETTE_SIZE],
    history: VecDeque<SpectrogramColumn>,
    key: u64,
    pub(crate) piano_roll_overlay: PianoRollOverlay,
    sample_rate: f32,
    fft_size: usize,
    freq_scale: FrequencyScale,
    zoom: f32,
    pan: f32,
}

impl SpectrogramState {
    pub fn new() -> Self {
        Self {
            buffer: RefCell::new(SpectrogramBuffer::new()),
            style: SpectrogramStyle::default(),
            palette: theme::spectrogram::COLORS,
            history: VecDeque::new(),
            key: super::next_key(),
            piano_roll_overlay: PianoRollOverlay::Off,
            sample_rate: DEFAULT_SAMPLE_RATE,
            fft_size: 4096,
            freq_scale: FrequencyScale::default(),
            zoom: 1.0,
            pan: 0.5,
        }
    }

    pub fn set_palette(&mut self, palette: &[Color; SPECTROGRAM_PALETTE_SIZE]) {
        if self.palette != *palette {
            self.palette = *palette;
            let values = self.buffer.borrow().values.clone();
            self.buffer.borrow_mut().pending_base = Some(Arc::from(values));
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
        if (self.style.floor_db - floor).abs() > f32::EPSILON {
            self.style.floor_db = floor;
            self.rebuild_buffer();
        }
    }

    pub fn palette(&self) -> [Color; SPECTROGRAM_PALETTE_SIZE] {
        self.palette
    }

    pub fn floor_db(&self) -> f32 {
        self.style.floor_db
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
        let new_h = snap
            .new_columns
            .iter()
            .map(|c| c.magnitudes_db.len().min(MAX_TEXTURE_BINS) as u32)
            .find(|&h| h > 0);

        if let Some(h) = new_h {
            let buf_h = self.buffer.borrow().height;
            if buf_h > 0 && h != buf_h {
                self.history.retain(|c| c.magnitudes_db.len() == h as usize);
            }
        }

        let mut buf = self.buffer.borrow_mut();
        if snap.reset || buf.needs_rebuild(snap, new_h) {
            buf.rebuild(&self.history, snap, &self.style);
        } else if !snap.new_columns.is_empty() {
            buf.append(&snap.new_columns, &self.style);
        }
    }

    pub fn visual_params(&self, bounds: Rectangle) -> Option<SpectrogramParams> {
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
            background: to_rgba(self.style.background),
            contrast: self.style.contrast,
            uv_y_range: [0.0, 1.0],
            screen_height: bounds.height,
            scroll_phase: buf.scroll_phase(),
        })
    }

    fn frequency_at_y_zoomed(&self, y: f32, bounds: Rectangle, uv_range: [f32; 2]) -> Option<f32> {
        if bounds.height <= 0.0 || y < bounds.y || y > bounds.y + bounds.height {
            return None;
        }
        let tex_uv = uv_range[0] + (y - bounds.y) / bounds.height * (uv_range[1] - uv_range[0]);
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

    fn clear_pending(&self) {
        let mut buf = self.buffer.borrow_mut();
        (buf.pending_base, buf.pending_cols) = (None, vec![]);
    }

    fn rebuild_buffer(&mut self) {
        let (history_length, display_bins_hz) = {
            let buf = self.buffer.borrow();
            (
                buf.capacity as usize,
                (!buf.display_freqs.is_empty()).then(|| buf.display_freqs.clone()),
            )
        };
        if self.history.is_empty() || history_length == 0 {
            return;
        }
        let update = SpectrogramUpdate {
            fft_size: self.fft_size,
            sample_rate: self.sample_rate,
            frequency_scale: self.freq_scale,
            history_length,
            reset: false,
            display_bins_hz,
            new_columns: Vec::new(),
        };
        self.buffer
            .borrow_mut()
            .rebuild(&self.history, &update, &self.style);
    }
}

const MIN_ZOOM: f32 = 1.0;
const MAX_ZOOM: f32 = 32.0;
const ZOOM_STEP: f32 = 1.15;

#[derive(Default)]
struct InteractionState {
    cursor: Option<Point>,
    modifiers: keyboard::Modifiers,
    drag: Option<(f32, f32)>, // (origin_y, start_pan)
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
        let Some(freq) = state.frequency_at_y_zoomed(cursor.y, bounds, uv_range) else {
            return;
        };
        let content = MusicalNote::format_with_hz(freq);

        let tsz = super::measure_text(&content, TOOLTIP_SIZE);

        let sz = Size::new(
            tsz.width + TOOLTIP_PAD * 2.0,
            tsz.height + TOOLTIP_PAD * 2.0,
        );
        let x = if cursor.x + 12.0 + sz.width <= bounds.x + bounds.width {
            cursor.x + 12.0
        } else {
            (cursor.x - 12.0 - sz.width).max(bounds.x)
        };
        let y = (cursor.y - sz.height * 0.5).clamp(bounds.y, bounds.y + bounds.height - sz.height);
        let tb = Rectangle::new(Point::new(x, y), sz);

        let pal = theme.extended_palette();
        renderer.fill_quad(
            Quad {
                bounds: Rectangle::new(Point::new(tb.x + 1.0, tb.y + 1.0), sz),
                border: Default::default(),
                ..Default::default()
            },
            Background::Color(theme::with_alpha(pal.background.base.color, 0.3)),
        );
        renderer.fill_quad(
            Quad {
                bounds: tb,
                border: theme::sharp_border(),
                ..Default::default()
            },
            Background::Color(pal.background.strong.color),
        );

        renderer.fill_text(
            super::make_text(&content, TOOLTIP_SIZE, tsz),
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
        let (nyq, min_f, scale) = (
            (state.sample_rate / 2.0).max(1.0),
            (state.sample_rate / state.fft_size as f32).max(20.0),
            state.freq_scale,
        );
        drop(state);

        let (freq_top, freq_bot) = (
            norm_to_freq(1.0 - uv_range[0], nyq, min_f, scale),
            norm_to_freq(1.0 - uv_range[1], nyq, min_f, scale),
        );
        let midi_lo = MusicalNote::from_frequency(freq_bot.max(16.0))
            .map(|n| (n.midi_number - 1).max(0))
            .unwrap_or(11);
        let midi_hi = MusicalNote::from_frequency(freq_top)
            .map(|n| n.midi_number + 1)
            .unwrap_or(128);

        let pal = theme.extended_palette();
        let (white, black) = (pal.background.weak.color, Color::from_rgb(0.1, 0.1, 0.1));
        let x = match overlay {
            PianoRollOverlay::Left => bounds.x,
            PianoRollOverlay::Right => bounds.x + bounds.width - PIANO_ROLL_WIDTH,
            PianoRollOverlay::Off => return,
        };

        let freq_to_y = |f: f32| {
            let tex_norm = freq_to_norm(f, nyq, min_f, scale);
            let view_norm = ((1.0 - tex_norm) - uv_range[0]) / (uv_range[1] - uv_range[0]);
            bounds.y + bounds.height * view_norm.clamp(0.0, 1.0)
        };

        let semi = 2.0f32.powf(0.5 / 12.0);
        for midi in midi_lo..=midi_hi {
            let Some(note) = MusicalNote::from_midi(midi) else {
                continue;
            };
            let freq = note.to_frequency();
            let (yt, yb) = (freq_to_y(freq * semi), freq_to_y(freq / semi));
            if yb < bounds.y || yt > bounds.y + bounds.height {
                continue;
            }
            let (fill, brd) = if note.is_black() {
                (black, Default::default())
            } else {
                (
                    white,
                    iced::Border {
                        color: theme::with_alpha(black, 0.4),
                        width: 0.5,
                        radius: 0.0.into(),
                    },
                )
            };
            renderer.fill_quad(
                Quad {
                    bounds: Rectangle::new(
                        Point::new(x, yt),
                        Size::new(PIANO_ROLL_WIDTH, (yb - yt).max(1.0)),
                    ),
                    border: brd,
                    ..Default::default()
                },
                Background::Color(fill),
            );
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
                if let Some((origin_y, start_pan)) = st.drag {
                    let mut state = self.state.borrow_mut();
                    let h = 0.5 / state.zoom;
                    state.pan = (start_pan - (position.y - origin_y) / b.height / state.zoom)
                        .clamp(h, 1.0 - h);
                    shell.request_redraw();
                }
            }
            iced::Event::Mouse(mouse::Event::CursorLeft) => st.cursor = None,
            iced::Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => st.modifiers = *m,
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) if st.modifiers.control() => {
                if let Some(pos) = st.cursor.filter(|p| b.contains(*p) && b.height > 0.0) {
                    let scroll_y = match *delta {
                        mouse::ScrollDelta::Lines { y, .. } => y,
                        mouse::ScrollDelta::Pixels { y, .. } => y / 50.0,
                    };
                    self.state
                        .borrow_mut()
                        .zoom_at((pos.y - b.y) / b.height, ZOOM_STEP.powf(scroll_y));
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
                    st.drag = Some((pos.y, state.pan));
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
        if let Some(mut p) = state.visual_params(bounds) {
            p.uv_y_range = uv_y_range;
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
        let m = BinMapping::new(
            8,
            2048,
            DEFAULT_SAMPLE_RATE,
            FrequencyScale::Logarithmic,
            false,
        );
        assert_eq!(m.lower.len(), 8);
        assert!(m.lower[0] as f32 + m.weight[0] > m.lower[7] as f32 + m.weight[7]);
    }
}
