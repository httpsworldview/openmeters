// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{
    SpectrumConfig, SpectrumProcessor as CoreSpectrumProcessor, SpectrumSnapshot,
};
use super::render::{SpectrumParams, SpectrumPrimitive};
use crate::persistence::settings::{SpectrumDisplayMode, SpectrumSettings, SpectrumWeightingMode};
use crate::util::audio::musical::NoteInfo;
use crate::util::audio::{fmt_freq, lerp};
use crate::util::color::{color_to_rgba, lerp_color, with_alpha};
use crate::vis_processor;
use crate::visuals::palettes;
use crate::visuals::spectrogram::processor::FrequencyScale;
use iced::advanced::renderer::{self, Quad};
use iced::advanced::text::Renderer as _;
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Layout, Renderer as _, Widget, layout, mouse};
use iced::{Background, Color, Element, Length, Point, Rectangle, Size};
use iced_wgpu::primitive::Renderer as _;
use std::cell::{Cell, RefCell};
use std::sync::{Arc, LazyLock};

const EPSILON: f32 = 1e-6;
const GRID_LABEL_SIZE: f32 = 10.0;
const GRID_LABEL_GAP: f32 = 6.0;

// Process-wide max label extent, letting `draw_grid` skip per-label shaping.
static GRID_LABEL_SLOT: LazyLock<Size> = LazyLock::new(|| {
    ["20.00Hz", "100.0Hz", "1.00kHz", "10.0kHz"]
        .iter()
        .map(|s| crate::visuals::measure_text(s, GRID_LABEL_SIZE))
        .fold(Size::ZERO, |a, s| {
            Size::new(a.width.max(s.width), a.height.max(s.height))
        })
});

vis_processor!(
    SpectrumProcessor,
    CoreSpectrumProcessor,
    SpectrumConfig,
    SpectrumSnapshot,
    sync_rate
);

#[derive(Debug, Clone, Copy)]
pub(crate) struct SpectrumStyle {
    pub min_db: f32,
    pub max_db: f32,
    pub min_frequency: f32,
    pub max_frequency: f32,
    pub resolution: usize,
    pub line_thickness: f32,
    pub secondary_line_thickness: f32,
    pub smoothing_radius: usize,
    pub smoothing_passes: usize,
    pub highlight_threshold: f32,
    pub spectrum_palette: [Color; 6],
    pub frequency_scale: FrequencyScale,
    pub reverse_frequency: bool,
    pub show_grid: bool,
    pub show_peak_label: bool,
    pub display_mode: SpectrumDisplayMode,
    pub weighting_mode: SpectrumWeightingMode,
    pub show_secondary_line: bool,
    pub bar_count: usize,
    pub bar_gap: f32,
}

impl Default for SpectrumStyle {
    fn default() -> Self {
        let defaults = SpectrumSettings::default();
        Self {
            min_db: -120.0,
            max_db: 0.0,
            min_frequency: 20.0,
            max_frequency: 20_000.0,
            resolution: 1024,
            line_thickness: 0.5,
            secondary_line_thickness: 0.5,
            smoothing_radius: defaults.smoothing_radius,
            smoothing_passes: defaults.smoothing_passes,
            highlight_threshold: defaults.highlight_threshold,
            spectrum_palette: palettes::spectrum::COLORS,
            frequency_scale: defaults.frequency_scale,
            reverse_frequency: defaults.reverse_frequency,
            show_grid: defaults.show_grid,
            show_peak_label: defaults.show_peak_label,
            display_mode: defaults.display_mode,
            weighting_mode: defaults.weighting_mode,
            show_secondary_line: defaults.show_secondary_line,
            bar_count: defaults.bar_count,
            bar_gap: defaults.bar_gap,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PeakLabel {
    text: String,
    x: f32,
    y: f32,
    opacity: f32,
    flipped: Cell<bool>,
}

#[derive(Debug, Clone)]
pub(crate) struct SpectrumState {
    style: SpectrumStyle,
    weighted: Arc<[[f32; 2]]>,
    unweighted: Arc<[[f32; 2]]>,
    key: u64,
    peak: Option<PeakLabel>,
    effective_range: Option<(f32, f32)>,
    scratch: Vec<f32>,
}

impl SpectrumState {
    pub fn new() -> Self {
        Self {
            style: SpectrumStyle::default(),
            weighted: Arc::from([]),
            unweighted: Arc::from([]),
            key: crate::visuals::next_key(),
            peak: None,
            effective_range: None,
            scratch: Vec::new(),
        }
    }

    pub fn style(&self) -> &SpectrumStyle {
        &self.style
    }
    pub fn style_mut(&mut self) -> &mut SpectrumStyle {
        &mut self.style
    }

    pub fn update_show_grid(&mut self, show: bool) {
        self.style.show_grid = show;
    }

    pub fn update_show_peak_label(&mut self, show: bool) {
        self.style.show_peak_label = show;
        if !show {
            self.peak = None;
        }
    }

    pub fn set_palette(&mut self, palette: &[Color; 6]) {
        self.style.spectrum_palette = *palette;
    }

    pub fn apply_snapshot(&mut self, snap: SpectrumSnapshot) {
        if snap.frequency_bins.is_empty()
            || snap.magnitudes_db.is_empty()
            || snap.frequency_bins.len() != snap.magnitudes_db.len()
        {
            self.effective_range = None;
            self.fade_peak(None);
            return;
        }
        let nyq = snap
            .frequency_bins
            .last()
            .copied()
            .unwrap_or(self.style.max_frequency);
        let (min_f, mut max_f) = (
            self.style.min_frequency.max(EPSILON),
            self.style.max_frequency.min(nyq),
        );
        if max_f <= min_f {
            max_f = nyq.max(min_f * 1.02);
        }
        if max_f <= min_f {
            self.effective_range = None;
            self.fade_peak(None);
            return;
        }

        let scale = Scale::new(min_f, max_f);
        let res = self.style.resolution.max(32);
        let (mut w, mut u) = (Vec::new(), Vec::new());
        build_points(&self.style, &mut w, &mut u, res, &scale, &snap);

        if self.style.smoothing_radius > 0 && self.style.smoothing_passes > 0 {
            let (r, p) = (self.style.smoothing_radius, self.style.smoothing_passes);
            smooth(&mut w, r, p, &mut self.scratch);
            smooth(&mut u, r, p, &mut self.scratch);
        }
        if self.style.reverse_frequency {
            for buf in [&mut w, &mut u] {
                buf.reverse();
                reindex(&mut buf[..]);
            }
        }

        self.weighted = Arc::from(w);
        self.unweighted = Arc::from(u);
        self.effective_range = Some((min_f, max_f));

        let pk = self
            .style
            .show_peak_label
            .then(|| self.build_peak(&snap, &scale))
            .flatten();
        self.fade_peak(pk);
    }

    fn build_peak(&self, snap: &SpectrumSnapshot, sc: &Scale) -> Option<PeakLabel> {
        let f = snap
            .peak_frequency_hz
            .filter(|&f| f.is_finite() && f > 0.0)?;
        let mut x = sc.pos_of(self.style.frequency_scale, f);
        if self.style.reverse_frequency {
            x = 1.0 - x;
        }
        let m = interp(&snap.frequency_bins, &snap.magnitudes_db, f);
        let y = ((m - self.style.min_db) / (self.style.max_db - self.style.min_db).max(EPSILON))
            .clamp(0.0, 1.0);
        if y < 0.08 {
            return None;
        }
        let text = NoteInfo::from_frequency(f).map_or_else(
            || fmt_freq(f),
            |ni| format!("{} | {}", fmt_freq(f), ni.fmt_note_cents()),
        );
        Some(PeakLabel {
            text,
            x: x.clamp(0.0, 1.0),
            y,
            opacity: 0.0,
            flipped: Cell::new(false),
        })
    }

    fn fade_peak(&mut self, incoming: Option<PeakLabel>) {
        match (incoming, &mut self.peak) {
            (Some(new), Some(p)) => {
                p.text = new.text;
                p.x = 0.80 * p.x + 0.20 * new.x;
                p.y = 0.80 * p.y + 0.20 * new.y;
                p.opacity = (0.65 * p.opacity + 0.35).min(1.0);
            }
            (Some(new), None) => self.peak = Some(new),
            (None, Some(p)) => {
                p.opacity *= 0.88;
                if p.opacity < 0.01 {
                    self.peak = None;
                }
            }
            (None, None) => {}
        }
    }

    pub fn peak(&self) -> Option<&PeakLabel> {
        self.peak.as_ref().filter(|_| self.style.show_peak_label)
    }

    fn visual_params(&self, bounds: Rectangle, theme: &iced::Theme) -> Option<SpectrumParams> {
        if self.weighted.len() < 2 {
            return None;
        }
        let pal = theme.extended_palette();

        let (primary, secondary) = match self.style.weighting_mode {
            SpectrumWeightingMode::AWeighted => (&self.weighted, &self.unweighted),
            SpectrumWeightingMode::Raw => (&self.unweighted, &self.weighted),
        };

        Some(SpectrumParams {
            bounds,
            normalized_points: Arc::clone(primary),
            secondary_points: Arc::clone(secondary),
            key: self.key,
            line_color: color_to_rgba(lerp_color(
                pal.primary.base.color,
                pal.background.base.text,
                0.35,
            )),
            line_width: self.style.line_thickness,
            secondary_line_color: color_to_rgba(with_alpha(pal.secondary.weak.text, 0.3)),
            secondary_line_width: self.style.secondary_line_thickness,
            highlight_threshold: self.style.highlight_threshold,
            spectrum_palette: self.style.spectrum_palette.map(color_to_rgba),
            display_mode: self.style.display_mode,
            show_secondary_line: self.style.show_secondary_line,
            bar_count: self.style.bar_count,
            bar_gap: self.style.bar_gap,
        })
    }
}

#[derive(Debug)]
pub(crate) struct Spectrum<'a>(&'a RefCell<SpectrumState>);
impl<'a> Spectrum<'a> {
    pub fn new(state: &'a RefCell<SpectrumState>) -> Self {
        Self(state)
    }
}

impl<'a, M> Widget<M, iced::Theme, iced::Renderer> for Spectrum<'a> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<()>()
    }
    fn state(&self) -> tree::State {
        tree::State::None
    }
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }
    fn children(&self) -> Vec<Tree> {
        Vec::new()
    }
    fn diff(&self, _: &mut Tree) {}
    fn layout(&mut self, _: &mut Tree, _: &iced::Renderer, lim: &layout::Limits) -> layout::Node {
        layout::Node::new(lim.resolve(Length::Fill, Length::Fill, Size::ZERO))
    }
    fn draw(
        &self,
        _: &Tree,
        r: &mut iced::Renderer,
        th: &iced::Theme,
        _: &renderer::Style,
        lay: Layout<'_>,
        _: mouse::Cursor,
        _: &Rectangle,
    ) {
        let b = lay.bounds();
        let state = self.0.borrow();
        let Some(params) = state.visual_params(b, th) else {
            r.fill_quad(
                Quad {
                    bounds: b,
                    border: Default::default(),
                    shadow: Default::default(),
                    snap: true,
                },
                Background::Color(th.extended_palette().background.base.color),
            );
            return;
        };
        if state.style.show_grid
            && let Some((min_f, max_f)) = state.effective_range
        {
            r.with_layer(b, |r| draw_grid(r, th, b, min_f, max_f, &state.style));
        }
        r.draw_primitive(b, SpectrumPrimitive::new(params));
        if let Some(pk) = state.peak() {
            r.with_layer(b, |r| draw_peak(r, th, b, pk));
        }
    }
}

pub(crate) fn widget<'a, M: 'a>(state: &'a RefCell<SpectrumState>) -> Element<'a, M> {
    Element::new(Spectrum::new(state))
}

// --- Helpers ---

#[derive(Clone, Copy)]
struct Scale {
    min: f32,
    max: f32,
}

impl Scale {
    fn new(min: f32, max: f32) -> Self {
        Self { min, max }
    }
    fn freq_at(&self, s: FrequencyScale, t: f32) -> f32 {
        s.freq_at(self.min, self.max, t)
    }
    fn pos_of(&self, s: FrequencyScale, f: f32) -> f32 {
        s.pos_of(self.min, self.max, f).clamp(0.0, 1.0)
    }
}

fn interp(bins: &[f32], mags: &[f32], t: f32) -> f32 {
    if bins.is_empty() || t <= bins[0] {
        return mags.first().copied().unwrap_or(0.0);
    }
    if bins.last().is_some_and(|&last| t >= last) {
        return mags.last().copied().unwrap_or(0.0);
    }
    match bins.binary_search_by(|p| p.partial_cmp(&t).unwrap_or(std::cmp::Ordering::Less)) {
        Ok(i) => mags.get(i).copied().unwrap_or(0.0),
        Err(i) => {
            let (lo, hi) = (i.saturating_sub(1), i.min(bins.len() - 1));
            lerp(
                mags.get(lo).copied().unwrap_or(0.0),
                mags.get(hi).copied().unwrap_or(0.0),
                (t - bins[lo]) / (bins[hi] - bins[lo]).max(EPSILON),
            )
        }
    }
}

fn build_points(
    style: &SpectrumStyle,
    w: &mut Vec<[f32; 2]>,
    u: &mut Vec<[f32; 2]>,
    res: usize,
    sc: &Scale,
    snap: &SpectrumSnapshot,
) {
    w.clear();
    u.clear();
    w.reserve(res);
    u.reserve(res);
    let dr = (style.max_db - style.min_db).max(EPSILON);
    for i in 0..res {
        let t = if res > 1 {
            i as f32 / (res - 1) as f32
        } else {
            0.0
        };
        let f = sc.freq_at(style.frequency_scale, t);
        let mw = interp(&snap.frequency_bins, &snap.magnitudes_db, f);
        let mu = interp(&snap.frequency_bins, &snap.magnitudes_unweighted_db, f);
        w.push([t, ((mw - style.min_db) / dr).clamp(0.0, 1.0)]);
        u.push([t, ((mu - style.min_db) / dr).clamp(0.0, 1.0)]);
    }
}

fn smooth(pts: &mut [[f32; 2]], r: usize, passes: usize, scratch: &mut Vec<f32>) {
    if r == 0 || passes == 0 || pts.len() < 3 {
        return;
    }
    scratch.resize(pts.len(), 0.0);
    for _ in 0..passes {
        for (d, p) in scratch.iter_mut().zip(pts.iter()) {
            *d = p[1];
        }
        for (i, p) in pts.iter_mut().enumerate() {
            let (s, e) = (i.saturating_sub(r), (i + r + 1).min(scratch.len()));
            let (mut sum, mut wsum) = (0.0f32, 0.0f32);
            for (j, &v) in scratch[s..e].iter().enumerate() {
                let w = (r - (s + j).abs_diff(i) + 1) as f32;
                sum += v * w;
                wsum += w;
            }
            p[1] = sum / wsum;
        }
    }
}

fn reindex(pts: &mut [[f32; 2]]) {
    let n = pts.len();
    for (i, p) in pts.iter_mut().enumerate() {
        p[0] = if n > 1 {
            i as f32 / (n - 1) as f32
        } else {
            0.0
        };
    }
}

fn draw_grid(
    r: &mut iced::Renderer,
    th: &iced::Theme,
    b: Rectangle,
    min_f: f32,
    max_f: f32,
    style: &SpectrumStyle,
) {
    if b.width <= 0.0 || b.height <= 0.0 {
        return;
    }
    let start_exp = min_f.max(1.0).log10().floor() as i32;
    let end_exp = max_f.log10().ceil() as i32;
    if end_exp < start_exp {
        return;
    }

    let scale = Scale::new(min_f, max_f);
    let reverse = style.reverse_frequency;
    let pal = th.extended_palette();
    let txt = pal.background.base.text;
    let (major_lc, major_tc) = (with_alpha(txt, 0.25), with_alpha(txt, 0.75));
    let (minor_lc, minor_tc) = (with_alpha(txt, 0.10), with_alpha(txt, 0.20));

    // Walk decades in whichever direction produces ascending screen-x, so the
    // collision check in pass 2 is a single forward sweep.
    let exp_of = |di: i32| {
        if reverse {
            end_exp - di
        } else {
            start_exp + di
        }
    };
    let tick_x = |f: f32| -> Option<f32> {
        if !(min_f..=max_f).contains(&f) {
            return None;
        }
        let pos = scale.pos_of(style.frequency_scale, f);
        pos.is_finite()
            .then_some(b.x + b.width * if reverse { 1.0 - pos } else { pos })
    };
    let vline = |r: &mut iced::Renderer, x: f32, top: f32, h: f32, c: Color| {
        let sx = (x - 0.5).clamp(b.x, b.x + b.width - 1.0);
        r.fill_quad(
            Quad {
                bounds: Rectangle::new(Point::new(sx, top), Size::new(1.0, h)),
                border: Default::default(),
                shadow: Default::default(),
                snap: true,
            },
            Background::Color(c),
        );
    };

    // Pass 1: minor lines.
    for di in 0..=(end_exp - start_exp) {
        let base = 10f32.powi(exp_of(di));
        for &mult in &[3u32, 4, 6, 7, 8, 9] {
            if let Some(x) = tick_x(base * mult as f32) {
                vline(r, x, b.y, b.height, minor_lc);
            }
        }
    }

    // Pass 2: labeled ticks. Lines always span the full height so culled
    // labels still leave their tick visible.
    let slot = *GRID_LABEL_SLOT;
    let ty = b.y + GRID_LABEL_GAP;
    let clamp_lo = b.x + GRID_LABEL_GAP;
    let clamp_hi = (b.x + b.width - GRID_LABEL_GAP - slot.width).max(clamp_lo);
    let mults: [u32; 3] = if reverse { [5, 2, 1] } else { [1, 2, 5] };
    let mut last_right = f32::NEG_INFINITY;

    for di in 0..=(end_exp - start_exp) {
        let base = 10f32.powi(exp_of(di));
        for &mult in &mults {
            let f = base * mult as f32;
            let Some(x) = tick_x(f) else { continue };
            let (lc, tc) = if mult == 1 {
                (major_lc, major_tc)
            } else {
                (minor_lc, minor_tc)
            };

            // Line draws unconditionally; collision only gates the text.
            vline(r, x, b.y, b.height, lc);

            let tx = (x - slot.width * 0.5).clamp(clamp_lo, clamp_hi);
            if tx < last_right {
                continue;
            }
            last_right = tx + slot.width + GRID_LABEL_GAP;

            // `make_text` hardcodes Left alignment; inline the struct to get
            // Center so short labels still sit exactly on the tick.
            r.fill_text(
                iced::advanced::text::Text {
                    content: fmt_freq(f),
                    bounds: slot,
                    size: iced::Pixels(GRID_LABEL_SIZE),
                    font: iced::Font::default(),
                    align_x: iced::alignment::Horizontal::Center.into(),
                    align_y: iced::alignment::Vertical::Top,
                    line_height: iced::advanced::text::LineHeight::default(),
                    shaping: iced::advanced::text::Shaping::Basic,
                    wrapping: iced::advanced::text::Wrapping::None,
                },
                Point::new(tx + slot.width * 0.5, ty),
                tc,
                Rectangle::new(Point::new(tx, ty), slot),
            );
        }
    }
}

fn draw_peak(r: &mut iced::Renderer, th: &iced::Theme, b: Rectangle, pk: &PeakLabel) {
    if pk.opacity < 0.01 || b.width < 8.0 || b.height < 8.0 {
        return;
    }
    let sz = crate::visuals::measure_text(&pk.text, 12.0);
    if sz.width <= 0.0 || sz.height <= 0.0 {
        return;
    }
    let (ax, ay) = (
        b.x + b.width * pk.x.clamp(0.0, 1.0),
        b.y + b.height * (1.0 - pk.y.clamp(0.0, 1.0)),
    );
    let (box_w, box_h) = (sz.width + 8.0, sz.height + 8.0);
    let right_edge = b.x + b.width;
    let hyst = box_w * 0.5;
    let flip_right = if pk.flipped.get() {
        ax + box_w > right_edge - hyst
    } else {
        ax + box_w > right_edge
    };
    pk.flipped.set(flip_right);
    let box_x = if flip_right { ax - box_w } else { ax };
    let box_y = ay - box_h;
    let box_x = box_x.clamp(b.x, (b.x + b.width - box_w).max(b.x));
    let box_y = box_y.clamp(b.y, (b.y + b.height - box_h).max(b.y));
    let (tx, ty) = (box_x + 4.0, box_y + 4.0);
    let bg = Rectangle::new(Point::new(box_x, box_y), Size::new(box_w, box_h));
    let bdr = iced::Border {
        color: with_alpha(crate::ui::theme::BORDER_SUBTLE, pk.opacity),
        width: 1.0,
        radius: 0.0.into(),
    };
    let pal = th.extended_palette();
    r.fill_quad(
        Quad {
            bounds: bg,
            border: bdr,
            shadow: Default::default(),
            snap: true,
        },
        Background::Color(with_alpha(pal.background.strong.color, pk.opacity)),
    );
    r.fill_text(
        crate::visuals::make_text(&pk.text, 12.0, sz),
        Point::new(tx, ty),
        with_alpha(pal.background.base.text, pk.opacity),
        Rectangle::new(Point::new(tx, ty), sz),
    );
}
