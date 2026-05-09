// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{
    SpectrumConfig, SpectrumProcessor as CoreSpectrumProcessor, SpectrumSnapshot,
};
use super::render::{MIN_BAR_COUNT, SpectrumParams, SpectrumPeakParams, SpectrumPrimitive};
use crate::persistence::settings::{SpectrumDisplayMode, SpectrumSettings, SpectrumWeightingMode};
use crate::util::audio::musical::NoteInfo;
use crate::util::audio::{FrequencyScale, fmt_freq, lerp};
use crate::util::color::{color_to_rgba, with_alpha};
use crate::vis_processor;
use crate::visuals::palettes;
use crate::visuals::render::common::{
    fill_rect, fill_snapped_bordered_rect, make_text, measure_text,
};
use iced::advanced::renderer;
use iced::advanced::text::Renderer as _;
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Layout, Renderer as _, Widget, layout, mouse};
use iced::{Color, Element, Length, Point, Rectangle, Size};
use iced_wgpu::primitive::Renderer as _;
use std::cell::RefCell;
use std::sync::{Arc, LazyLock};

const EPSILON: f32 = 1e-6;
const GRID_LABEL_SIZE: f32 = 10.0;
const GRID_LABEL_GAP: f32 = 6.0;

static GRID_LABEL_SLOT: LazyLock<Size> = LazyLock::new(|| {
    ["20.00Hz", "100.0Hz", "1.00kHz", "10.0kHz"]
        .iter()
        .map(|s| measure_text(s, GRID_LABEL_SIZE))
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
            min_db: defaults.floor_db,
            max_db: 0.0,
            min_frequency: 20.0,
            max_frequency: 20_000.0,
            resolution: 1024,
            line_thickness: 1.0,
            secondary_line_thickness: 0.75,
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
    text: [String; 2],
    label_pos: [f32; 2],
    marker_pos: [f32; 2],
    opacity: f32,
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
            weighted: Arc::default(),
            unweighted: Arc::default(),
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
        let bins = snap.frequency_bins.len();
        if bins == 0
            || snap.magnitudes_db.len() != bins
            || snap.magnitudes_unweighted_db.len() != bins
        {
            self.clear_visuals();
            return;
        }
        let nyq = snap.frequency_bins[bins - 1];
        let (min_f, mut max_f) = (
            self.style.min_frequency.max(EPSILON),
            self.style.max_frequency.min(nyq),
        );
        if max_f <= min_f {
            max_f = nyq.max(min_f * 1.02);
        }
        if max_f <= min_f {
            self.clear_visuals();
            return;
        }

        let res = self.style.resolution.max(32);
        let (mut w, mut u) = build_points(&self.style, res, min_f, max_f, &snap);

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

        let primary = match self.style.weighting_mode {
            SpectrumWeightingMode::AWeighted => w.as_slice(),
            SpectrumWeightingMode::Raw => u.as_slice(),
        };
        let pk = self
            .style
            .show_peak_label
            .then(|| self.build_peak(primary, min_f, max_f))
            .flatten();

        self.weighted = Arc::from(w);
        self.unweighted = Arc::from(u);
        self.effective_range = Some((min_f, max_f));
        self.fade_peak(pk);
    }

    fn clear_visuals(&mut self) {
        (self.weighted, self.unweighted) = (Arc::default(), Arc::default());
        self.effective_range = None;
        self.peak = None;
    }

    fn build_peak(&self, pts: &[[f32; 2]], min_f: f32, max_f: f32) -> Option<PeakLabel> {
        let [x, y] = visible_peak(pts, &self.style).filter(|p| p[1] >= 0.08)?;
        let t = if self.style.reverse_frequency {
            1.0 - x
        } else {
            x
        }
        .clamp(0.0, 1.0);
        let f = self.style.frequency_scale.freq_at(min_f, max_f, t);
        if !f.is_finite() {
            return None;
        }
        let pos = [x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)];
        let m = lerp(self.style.min_db, self.style.max_db, pos[1]);
        let unit = match self.style.weighting_mode {
            SpectrumWeightingMode::AWeighted => "dBFS(A)",
            SpectrumWeightingMode::Raw => "dBFS",
        };
        let freq = fmt_freq(f);
        let text = NoteInfo::from_frequency(f).map_or_else(
            || [freq.clone(), format!("{:.1} {unit}", m)],
            |ni| [ni.fmt_note_cents(), format!("{freq}   {:.1} {unit}", m)],
        );
        Some(PeakLabel {
            text,
            label_pos: pos,
            marker_pos: pos,
            opacity: 1.0,
        })
    }

    fn fade_peak(&mut self, incoming: Option<PeakLabel>) {
        match (incoming, &mut self.peak) {
            (Some(new), Some(p)) => {
                p.text = new.text;
                p.label_pos = std::array::from_fn(|i| lerp(p.label_pos[i], new.label_pos[i], 0.20));
                p.marker_pos = new.marker_pos;
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

    fn visual_params(
        &self,
        bounds: Rectangle,
        theme: &iced::Theme,
        peak_layout: Option<PeakLayout>,
    ) -> Option<SpectrumParams> {
        if self.weighted.len() < 2 {
            return None;
        }
        let pal = theme.extended_palette();

        let (primary, secondary) = match self.style.weighting_mode {
            SpectrumWeightingMode::AWeighted => (&self.weighted, &self.unweighted),
            SpectrumWeightingMode::Raw => (&self.unweighted, &self.weighted),
        };
        let peak = self.peak();
        let accent = self.style.spectrum_palette[5];

        Some(SpectrumParams {
            bounds,
            normalized_points: Arc::clone(primary),
            secondary_points: Arc::clone(secondary),
            key: self.key,
            line_color: color_to_rgba(with_alpha(pal.background.base.text, 0.92)),
            line_width: self.style.line_thickness,
            secondary_line_color: color_to_rgba(with_alpha(pal.secondary.weak.text, 0.32)),
            secondary_line_width: self.style.secondary_line_thickness,
            highlight_threshold: self.style.highlight_threshold,
            spectrum_palette: self.style.spectrum_palette.map(color_to_rgba),
            display_mode: self.style.display_mode,
            show_secondary_line: self.style.show_secondary_line,
            bar_count: self.style.bar_count,
            bar_gap: self.style.bar_gap,
            peak: peak.map(|p| SpectrumPeakParams {
                marker: p.marker_pos,
                marker_color: color_to_rgba(with_alpha(accent, p.opacity * 0.95)),
                leader_anchor: peak_layout.map(|l| point_to_normalized(bounds, l.leader_anchor)),
                leader_color: color_to_rgba(with_alpha(accent, p.opacity * 0.32)),
            }),
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
        let peak = state.peak();
        let peak_layout = peak.and_then(|p| peak_label_layout(b, p));
        let Some(params) = state.visual_params(b, th, peak_layout) else {
            fill_rect(r, b, th.extended_palette().background.base.color);
            return;
        };
        if let Some((min_f, max_f)) = state.effective_range.filter(|_| state.style.show_grid) {
            r.with_layer(b, |r| draw_grid(r, th, b, min_f, max_f, &state.style));
        }
        r.draw_primitive(b, SpectrumPrimitive::new(params));
        if let Some((pk, layout)) = peak.zip(peak_layout) {
            let accent = state.style.spectrum_palette[5];
            r.with_layer(b, |r| draw_peak(r, th, pk, layout, accent));
        }
    }
}

pub(crate) fn widget<'a, M: 'a>(state: &'a RefCell<SpectrumState>) -> Element<'a, M> {
    Element::new(Spectrum::new(state))
}

fn interp(bins: &[f32], mags: &[f32], t: f32) -> f32 {
    let i = bins.partition_point(|&f| f < t);
    if i == 0 {
        return mags.first().copied().unwrap_or(0.0);
    }
    if i >= bins.len() {
        return mags.last().copied().unwrap_or(0.0);
    }
    lerp(
        mags.get(i - 1).copied().unwrap_or(0.0),
        mags.get(i).copied().unwrap_or(0.0),
        (t - bins[i - 1]) / (bins[i] - bins[i - 1]).max(EPSILON),
    )
}

fn visible_peak(pts: &[[f32; 2]], style: &SpectrumStyle) -> Option<[f32; 2]> {
    match style.display_mode {
        SpectrumDisplayMode::Line => max_finite_point(pts.iter().copied()),
        SpectrumDisplayMode::Bar => {
            let bars = style.bar_count.max(MIN_BAR_COUNT);
            max_finite_point((0..bars).map(|i| {
                let t0 = i as f32 / bars as f32;
                let t1 = (i + 1) as f32 / bars as f32;
                [(t0 + t1) * 0.5, super::render::sample_max(pts, t0, t1)]
            }))
        }
    }
}

fn max_finite_point(points: impl Iterator<Item = [f32; 2]>) -> Option<[f32; 2]> {
    points
        .filter(|p| p[0].is_finite() && p[1].is_finite())
        .max_by(|a, b| a[1].total_cmp(&b[1]))
}

fn build_points(
    style: &SpectrumStyle,
    res: usize,
    min_f: f32,
    max_f: f32,
    snap: &SpectrumSnapshot,
) -> (Vec<[f32; 2]>, Vec<[f32; 2]>) {
    let (bins, db, raw) = (
        snap.frequency_bins.as_slice(),
        snap.magnitudes_db.as_slice(),
        snap.magnitudes_unweighted_db.as_slice(),
    );
    let dr = (style.max_db - style.min_db).max(EPSILON);
    let denom = res.saturating_sub(1).max(1) as f32;
    let y = |m: f32| ((m - style.min_db) / dr).clamp(0.0, 1.0);
    (0..res)
        .map(|i| {
            let t = i as f32 / denom;
            let f = style.frequency_scale.freq_at(min_f, max_f, t);
            let mw = y(interp(bins, db, f));
            let mu = y(interp(bins, raw, f));
            ([t, mw], [t, mu])
        })
        .unzip()
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
    let denom = pts.len().saturating_sub(1).max(1) as f32;
    for (i, p) in pts.iter_mut().enumerate() {
        p[0] = i as f32 / denom;
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

    let reverse = style.reverse_frequency;
    let pal = th.extended_palette();
    let txt = pal.background.base.text;
    let (major_lc, major_tc) = (with_alpha(txt, 0.25), with_alpha(txt, 0.75));
    let (minor_lc, minor_tc) = (with_alpha(txt, 0.10), with_alpha(txt, 0.20));

    let exp_of = |di| {
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
        let pos = style
            .frequency_scale
            .pos_of(min_f, max_f, f)
            .clamp(0.0, 1.0);
        pos.is_finite()
            .then_some(b.x + b.width * if reverse { 1.0 - pos } else { pos })
    };
    let vline = |r: &mut iced::Renderer, x: f32, top: f32, h: f32, c: Color| {
        let sx = (x - 0.5).clamp(b.x, (b.x + b.width - 1.0).max(b.x));
        fill_rect(r, Rectangle::new(Point::new(sx, top), Size::new(1.0, h)), c);
    };

    for di in 0..=(end_exp - start_exp) {
        let base = 10f32.powi(exp_of(di));
        for &mult in &[3u32, 4, 6, 7, 8, 9] {
            if let Some(x) = tick_x(base * mult as f32) {
                vline(r, x, b.y, b.height, minor_lc);
            }
        }
    }

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

            vline(r, x, b.y, b.height, lc);

            let tx = (x - slot.width * 0.5).clamp(clamp_lo, clamp_hi);
            if tx < last_right {
                continue;
            }
            last_right = tx + slot.width + GRID_LABEL_GAP;

            let mut text = make_text(&fmt_freq(f), GRID_LABEL_SIZE, slot);
            text.align_x = iced::alignment::Horizontal::Center.into();
            r.fill_text(
                text,
                Point::new(tx + slot.width * 0.5, ty),
                tc,
                Rectangle::new(Point::new(tx, ty), slot),
            );
        }
    }
}

#[derive(Clone, Copy)]
struct PeakLayout {
    rect: Rectangle,
    title: Size,
    detail: Size,
    text: Point,
    leader_anchor: Point,
}

fn point_to_normalized(b: Rectangle, p: Point) -> [f32; 2] {
    [(p.x - b.x) / b.width, 1.0 - (p.y - b.y) / b.height]
}

fn peak_label_layout(b: Rectangle, pk: &PeakLabel) -> Option<PeakLayout> {
    if pk.opacity < 0.01 || b.width < 8.0 || b.height < 8.0 {
        return None;
    }
    let title = measure_text(&pk.text[0], 12.0);
    let detail = measure_text(&pk.text[1], 10.0);
    let [px, py] = pk.label_pos;
    let p = Point::new(b.x + b.width * px, b.y + b.height * (1.0 - py));
    let (w, h) = (
        title.width.max(detail.width) + 14.0,
        title.height + detail.height + 13.0,
    );
    let right = p.x + w + 8.0 <= b.x + b.width;
    let x = if right { p.x + 8.0 } else { p.x - w - 8.0 }.clamp(b.x, (b.x + b.width - w).max(b.x));
    let y = (p.y - h - 8.0).clamp(b.y, (b.y + b.height - h).max(b.y));
    Some(PeakLayout {
        rect: Rectangle::new(Point::new(x, y), Size::new(w, h)),
        title,
        detail,
        text: Point::new(x + 7.0, y + 6.0),
        leader_anchor: Point::new(if right { x } else { x + w }, y + h),
    })
}

fn draw_peak(
    r: &mut iced::Renderer,
    th: &iced::Theme,
    pk: &PeakLabel,
    layout: PeakLayout,
    accent: Color,
) {
    let pal = th.extended_palette();
    fill_snapped_bordered_rect(
        r,
        layout.rect,
        with_alpha(pal.background.strong.color, 0.90 * pk.opacity),
        iced::Border {
            color: with_alpha(accent, 0.50 * pk.opacity),
            width: 1.0,
            radius: 2.0.into(),
        },
    );
    r.fill_text(
        make_text(&pk.text[0], 12.0, layout.title),
        layout.text,
        with_alpha(pal.background.base.text, pk.opacity),
        Rectangle::new(layout.text, layout.title),
    );
    let pos = Point::new(layout.text.x, layout.text.y + layout.title.height + 2.0);
    r.fill_text(
        make_text(&pk.text[1], 10.0, layout.detail),
        pos,
        with_alpha(pal.secondary.weak.text, 0.84 * pk.opacity),
        Rectangle::new(pos, layout.detail),
    );
}
