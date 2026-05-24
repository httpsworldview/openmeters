// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::SpectrumSnapshot;
use super::render::{MIN_BAR_COUNT, SpectrumParams, SpectrumPeakParams, SpectrumPrimitive};
use crate::persistence::settings::SpectrumSettings;
use crate::visuals::options::{SpectrumDisplayMode, SpectrumWeightingMode};
use crate::util::audio::musical::NoteInfo;
use crate::util::audio::{FrequencyScale, fmt_freq, lerp};
use crate::util::color::{color_to_rgba, with_alpha};
use crate::visuals::palettes;
use crate::visuals::render::common::{
    fill_rect, fill_snapped_bordered_rect, make_text, measure_text,
};
use iced::advanced::Renderer as _;
use iced::advanced::text::Renderer as _;
use iced::{Color, Point, Rectangle, Size};
use std::sync::{Arc, LazyLock};

const EPSILON: f32 = 1e-6;
const MIN_FREQUENCY: f32 = 20.0;
const MAX_FREQUENCY: f32 = 20_000.0;
const MAX_DB: f32 = 0.0;
const SPECTRUM_RESOLUTION: usize = 1024;
const LINE_THICKNESS: f32 = 1.0;
const SECONDARY_LINE_THICKNESS: f32 = 0.75;
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

#[derive(Debug, Clone, Copy)]
pub(crate) struct SpectrumStyle {
    pub min_db: f32,
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
    title_size: Size,
    detail_size: Size,
    label_pos: [f32; 2],
    marker_pos: [f32; 2],
    opacity: f32,
}

#[derive(Debug, Clone)]
struct PeakUpdate {
    text: [String; 2],
    label_pos: [f32; 2],
    marker_pos: [f32; 2],
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
        let (min_f, mut max_f) = (MIN_FREQUENCY, MAX_FREQUENCY.min(nyq));
        if max_f <= min_f {
            max_f = nyq.max(min_f * 1.02);
        }
        if max_f <= min_f {
            self.clear_visuals();
            return;
        }

        let res = SPECTRUM_RESOLUTION;
        let (mut w, mut u) = build_points(&self.style, res, min_f, max_f, &snap);

        if self.style.smoothing_radius > 0 && self.style.smoothing_passes > 0 {
            let (r, p) = (self.style.smoothing_radius, self.style.smoothing_passes);
            smooth(&mut w, r, p, &mut self.scratch);
            smooth(&mut u, r, p, &mut self.scratch);
            self.scratch.shrink_to(res);
        } else {
            self.scratch = Vec::new();
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

    fn build_peak(&self, pts: &[[f32; 2]], min_f: f32, max_f: f32) -> Option<PeakUpdate> {
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
        let m = lerp(self.style.min_db, MAX_DB, pos[1]);
        let unit = match self.style.weighting_mode {
            SpectrumWeightingMode::AWeighted => "dBFS(A)",
            SpectrumWeightingMode::Raw => "dBFS",
        };
        let freq = fmt_freq(f);
        let text = match NoteInfo::from_frequency(f) {
            Some(ni) => [ni.fmt_note_cents(), format!("{freq}   {:.1} {unit}", m)],
            None => [freq, format!("{:.1} {unit}", m)],
        };
        Some(PeakUpdate {
            text,
            label_pos: pos,
            marker_pos: pos,
        })
    }

    fn fade_peak(&mut self, incoming: Option<PeakUpdate>) {
        match (incoming, &mut self.peak) {
            (Some(new), Some(p)) => {
                if p.text != new.text {
                    let (title_size, detail_size) = measure_peak_text(&new.text);
                    p.text = new.text;
                    p.title_size = title_size;
                    p.detail_size = detail_size;
                }
                p.label_pos = std::array::from_fn(|i| lerp(p.label_pos[i], new.label_pos[i], 0.20));
                p.marker_pos = new.marker_pos;
                p.opacity = (0.65 * p.opacity + 0.35).min(1.0);
            }
            (Some(new), None) => {
                let (title_size, detail_size) = measure_peak_text(&new.text);
                self.peak = Some(PeakLabel {
                    text: new.text,
                    title_size,
                    detail_size,
                    label_pos: new.label_pos,
                    marker_pos: new.marker_pos,
                    opacity: 1.0,
                });
            }
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
            line_width: LINE_THICKNESS,
            secondary_line_color: color_to_rgba(with_alpha(pal.secondary.weak.text, 0.32)),
            secondary_line_width: SECONDARY_LINE_THICKNESS,
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

crate::visuals::visualization_widget!(Spectrum, SpectrumState, |this, r, th, b| {
    let state = this.state.borrow();
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
});

fn interp_at(bins: &[f32], mags: &[f32], t: f32, i: usize) -> f32 {
    if i == 0 {
        return mags[0];
    }
    if i >= bins.len() {
        return mags[bins.len() - 1];
    }
    lerp(
        mags[i - 1],
        mags[i],
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
    let dr = (MAX_DB - style.min_db).max(EPSILON);
    let denom = res.saturating_sub(1).max(1) as f32;
    let y = |m: f32| ((m - style.min_db) / dr).clamp(0.0, 1.0);
    let (mut weighted, mut unweighted) = (Vec::with_capacity(res), Vec::with_capacity(res));
    let mut bin = 0;

    for i in 0..res {
        let t = i as f32 / denom;
        let f = style.frequency_scale.freq_at(min_f, max_f, t);
        while bin < bins.len() && bins[bin] < f {
            bin += 1;
        }
        weighted.push([t, y(interp_at(bins, db, f, bin))]);
        unweighted.push([t, y(interp_at(bins, raw, f, bin))]);
    }

    (weighted, unweighted)
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

fn measure_peak_text(text: &[String; 2]) -> (Size, Size) {
    (measure_text(&text[0], 12.0), measure_text(&text[1], 10.0))
}

fn point_to_normalized(b: Rectangle, p: Point) -> [f32; 2] {
    [(p.x - b.x) / b.width, 1.0 - (p.y - b.y) / b.height]
}

fn peak_label_layout(b: Rectangle, pk: &PeakLabel) -> Option<PeakLayout> {
    if pk.opacity < 0.01 || b.width < 8.0 || b.height < 8.0 {
        return None;
    }
    let (title, detail) = (pk.title_size, pk.detail_size);
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
