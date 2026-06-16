// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{SpectrumSnapshot, SpectrumTraceSnapshot};
use super::render::{SpectrumParams, SpectrumPeakParams, SpectrumPrimitive};
use crate::persistence::settings::SpectrumSettings;
use crate::visuals::options::{SpectrumDisplayMode, SpectrumWeightingMode};
use crate::util::audio::musical::NoteInfo;
use crate::util::audio::{Channel, FrequencyScale, fmt_freq, lerp};
use crate::util::color::{color_to_rgba, with_alpha};
use crate::visuals::palettes;
use crate::visuals::render::common::{fill_rect, fill_snapped_bordered_rect, make_text, measure_text};
use iced::advanced::Renderer as _;
use iced::advanced::text::Renderer as _;
use iced::{Color, Point, Rectangle, Size};
use std::sync::Arc;

const EPSILON: f32 = 1e-6;
const MIN_FREQUENCY: f32 = 20.0;
const MAX_DB: f32 = 0.0;
const LINE_THICKNESS: f32 = 1.0;
const SECONDARY_LINE_THICKNESS: f32 = 0.75;
const GRID_LABEL_SIZE: f32 = 10.0;
const GRID_LABEL_GAP: f32 = 6.0;

#[derive(Debug, Clone, Copy)]
pub(in crate::visuals) struct SpectrumStyle {
    pub min_db: f32,
    pub highlight_threshold: f32,
    pub spectrum_palette: [Color; 6],
    pub frequency_scale: FrequencyScale,
    pub source: Channel,
    pub reverse_frequency: bool,
    pub show_grid: bool,
    pub show_peak_label: bool,
    pub display_mode: SpectrumDisplayMode,
    pub weighting_mode: SpectrumWeightingMode,
    pub secondary_weighting_mode: SpectrumWeightingMode,
    pub secondary_source: Channel,
    pub bar_count: usize,
    pub bar_gap: f32,
}

impl Default for SpectrumStyle {
    fn default() -> Self {
        let defaults = SpectrumSettings::default();
        Self {
            min_db: defaults.floor_db,
            highlight_threshold: defaults.highlight_threshold,
            spectrum_palette: palettes::spectrum::COLORS,
            frequency_scale: defaults.frequency_scale,
            source: defaults.source,
            reverse_frequency: defaults.reverse_frequency,
            show_grid: defaults.show_grid,
            show_peak_label: defaults.show_peak_label,
            display_mode: defaults.display_mode,
            weighting_mode: defaults.weighting_mode,
            secondary_weighting_mode: defaults.secondary_weighting_mode,
            secondary_source: defaults.secondary_source,
            bar_count: defaults.bar_count,
            bar_gap: defaults.bar_gap,
        }
    }
}

#[derive(Debug, Clone)]
struct PeakLabel {
    text: [String; 2],
    label_pos: [f32; 2],
    marker_pos: [f32; 2],
    opacity: f32,
}

type PeakUpdate = ([String; 2], [f32; 2]);

#[derive(Debug, Clone)]
pub(in crate::visuals) struct SpectrumState {
    style: SpectrumStyle,
    primary: Arc<[[f32; 2]]>,
    secondary: Arc<[[f32; 2]]>,
    key: u64,
    peak: Option<PeakLabel>,
    effective_range: Option<(f32, f32)>,
    x_cache_key: (usize, u32, FrequencyScale),
    x_cache: Vec<f32>,
}

impl SpectrumState {
    pub fn new() -> Self {
        Self {
            style: SpectrumStyle::default(),
            primary: Arc::default(),
            secondary: Arc::default(),
            key: crate::visuals::next_key(),
            peak: None,
            effective_range: None,
            x_cache_key: (0, 0, FrequencyScale::default()),
            x_cache: Vec::new(),
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
        let (primary, secondary) = (primary_trace(&self.style), secondary_trace(&self.style));
        if bins == 0
            || (primary.is_none() && secondary.is_none())
            || [primary, secondary]
                .into_iter()
                .flatten()
                .any(|idx| snap.traces[idx].iter().any(|buf| buf.len() != bins))
        {
            self.clear_visuals();
            return;
        }
        let min_f = MIN_FREQUENCY;
        let max_f = snap.frequency_bins[bins - 1].max(min_f * 1.02);
        let bins = snap.frequency_bins.as_slice();
        let style = self.style;
        self.ensure_x_cache(min_f, max_f, bins);

        let points = |idx, mode| {
            build_single_points(
                &style,
                min_f,
                max_f,
                bins,
                trace_db(&snap.traces[idx], mode),
                &self.x_cache,
            )
        };
        let primary_points = primary
            .map(|idx| points(idx, self.style.weighting_mode))
            .unwrap_or_default();
        let secondary_points = secondary
            .map(|idx| points(idx, self.style.secondary_weighting_mode))
            .unwrap_or_default();
        let pk = primary
            .filter(|_| self.style.show_peak_label)
            .and_then(|idx| self.build_peak(bins, trace_db(&snap.traces[idx], self.style.weighting_mode), min_f, max_f));

        self.primary = Arc::from(primary_points);
        self.secondary = Arc::from(secondary_points);
        self.effective_range = Some((min_f, max_f));
        self.fade_peak(pk);
    }

    fn clear_visuals(&mut self) {
        (self.primary, self.secondary) = (Arc::default(), Arc::default());
        self.effective_range = None;
        self.peak = None;
    }

    fn ensure_x_cache(&mut self, min_f: f32, max_f: f32, bins: &[f32]) {
        let scale = self.style.frequency_scale;
        let key = (bins.len(), max_f.to_bits(), scale);
        if self.x_cache_key == key { return; }

        self.x_cache.clear();
        self.x_cache.reserve(bins.len() + 2);
        for f in std::iter::once(min_f)
            .chain(bins.iter().copied().filter(|&f| f > min_f && f < max_f))
            .chain([max_f])
        {
            let x = scale.pos_of(min_f, max_f, f).clamp(0.0, 1.0);
            self.x_cache.push(if x.is_finite() { x } else { 0.0 });
        }
        self.x_cache_key = key;
    }

    fn build_peak(
        &self,
        bins: &[f32],
        db: &[f32],
        min_f: f32,
        max_f: f32,
    ) -> Option<PeakUpdate> {
        let bin = peak_bin(bins, db, min_f, max_f)?;
        let (f, m) = interpolated_peak(bins, db, bin)?;
        let t = self.style.frequency_scale.pos_of(min_f, max_f, f);
        if !t.is_finite() || !m.is_finite() { return None; }
        let x = if self.style.reverse_frequency { 1.0 - t } else { t }.clamp(0.0, 1.0);
        let y = ((m - self.style.min_db) / (MAX_DB - self.style.min_db).max(EPSILON))
            .clamp(0.0, 1.0);
        if y < 0.08 { return None; }
        let unit = match self.style.weighting_mode {
            SpectrumWeightingMode::AWeighted => "dBFS(A)",
            SpectrumWeightingMode::Raw => "dBFS",
        };
        let freq = fmt_freq(f);
        let text = match NoteInfo::from_frequency(f) {
            Some(ni) => [ni.fmt_note_cents(), format!("{freq}   {m:.1} {unit}")],
            None => [freq, format!("{m:.1} {unit}")],
        };
        Some((text, [x, y]))
    }

    fn fade_peak(&mut self, incoming: Option<PeakUpdate>) {
        match (incoming, &mut self.peak) {
            (Some(new), Some(p)) => {
                p.text = new.0;
                p.label_pos = std::array::from_fn(|i| lerp(p.label_pos[i], new.1[i], 0.20));
                p.marker_pos = new.1;
                p.opacity = (0.65 * p.opacity + 0.35).min(1.0);
            }
            (Some(new), None) => {
                self.peak = Some(PeakLabel {
                    text: new.0,
                    label_pos: new.1,
                    marker_pos: new.1,
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

    fn peak(&self) -> Option<&PeakLabel> {
        self.peak.as_ref().filter(|_| {
            self.style.show_peak_label && self.style.source != Channel::None && self.primary.len() >= 2
        })
    }

    fn visual_params(
        &self,
        bounds: Rectangle,
        theme: &iced::Theme,
        peak_layout: Option<PeakLayout>,
    ) -> Option<SpectrumParams> {
        let has_primary = self.style.source != Channel::None && self.primary.len() >= 2;
        let has_secondary = self.style.secondary_source != Channel::None && self.secondary.len() >= 2;
        if !has_primary && !has_secondary { return None; }
        let pal = theme.extended_palette();

        let visible = |show: bool, points: &Arc<[[f32; 2]]>| {
            if show { Arc::clone(points) } else { Arc::default() }
        };
        let peak = self.peak();
        let accent = self.style.spectrum_palette[5];
        let (mut primary, mut secondary) = (
            visible(has_primary, &self.primary),
            visible(has_secondary, &self.secondary),
        );
        if self.style.display_mode == SpectrumDisplayMode::Bar && primary.is_empty() {
            std::mem::swap(&mut primary, &mut secondary);
        }

        Some(SpectrumParams {
            bounds,
            normalized_points: primary,
            secondary_points: secondary,
            key: self.key,
            line_color: color_to_rgba(with_alpha(pal.background.base.text, 0.92)),
            line_width: LINE_THICKNESS,
            secondary_line_color: color_to_rgba(with_alpha(pal.secondary.weak.text, 0.32)),
            secondary_line_width: SECONDARY_LINE_THICKNESS,
            highlight_threshold: self.style.highlight_threshold,
            spectrum_palette: self.style.spectrum_palette.map(color_to_rgba),
            display_mode: self.style.display_mode,
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

fn value_at(bins: &[f32], mags: &[f32], f: f32) -> f32 {
    let i = bins.partition_point(|&bin| bin < f);
    if i == 0 { return mags[0]; }
    if i >= bins.len() { return mags[bins.len() - 1]; }
    lerp(
        mags[i - 1],
        mags[i],
        (f - bins[i - 1]) / (bins[i] - bins[i - 1]).max(EPSILON),
    )
}

fn peak_bin(bins: &[f32], db: &[f32], min_f: f32, max_f: f32) -> Option<usize> {
    (1..bins.len().saturating_sub(1))
        .filter(|&i| (min_f..=max_f).contains(&bins[i]) && db[i].is_finite())
        .max_by(|&a, &b| db[a].total_cmp(&db[b]))
}

fn interpolated_peak(bins: &[f32], db: &[f32], bin: usize) -> Option<(f32, f32)> {
    let next = bin.checked_add(1)?;
    if bins.len() != db.len() || bin == 0 || next >= bins.len() { return None; }
    let bin_hz = bins[1] - bins[0];
    let (center_freq, center) = (bins[bin], db[bin]);
    if !bin_hz.is_finite()
        || bin_hz <= 0.0
        || !center_freq.is_finite()
        || !center.is_finite()
    {
        return None;
    }

    let (left, right) = (db[bin - 1], db[next]);
    let offset = if left.is_finite() && right.is_finite() {
        let denom = left - 2.0 * center + right;
        if denom < -EPSILON {
            (0.5 * (left - right) / denom).clamp(-0.5, 0.5)
        } else {
            0.0
        }
    } else {
        0.0
    };
    let level = if offset == 0.0 {
        center
    } else {
        (center - 0.25 * (left - right) * offset).max(center)
    };
    Some(((center_freq + offset * bin_hz).max(0.0), level))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secondary_trace_can_render_without_primary_source() {
        let trace = [vec![-20.0; 3], vec![-20.0; 3]];
        let mut state = SpectrumState::new();
        state.style.source = Channel::None;
        state.style.secondary_source = Channel::Left;

        state.apply_snapshot(SpectrumSnapshot {
            frequency_bins: vec![0.0, 20.0, 40.0],
            traces: [SpectrumTraceSnapshot::default(), trace],
        });

        assert!(state.primary.is_empty());
        assert!(state.secondary.len() >= 2);
        assert!(state.peak().is_none());
    }
}

fn primary_trace(style: &SpectrumStyle) -> Option<usize> {
    (style.source != Channel::None).then_some(0)
}

fn secondary_trace(style: &SpectrumStyle) -> Option<usize> {
    match (style.source, style.secondary_source) {
        (_, Channel::None) => None,
        (primary, secondary) if primary == secondary => Some(0),
        _ => Some(1),
    }
}

fn weighting_slot(mode: SpectrumWeightingMode) -> usize {
    match mode {
        SpectrumWeightingMode::AWeighted => 0,
        SpectrumWeightingMode::Raw => 1,
    }
}

fn trace_db(trace: &SpectrumTraceSnapshot, mode: SpectrumWeightingMode) -> &[f32] {
    &trace[weighting_slot(mode)]
}

fn build_single_points(
    style: &SpectrumStyle,
    min_f: f32,
    max_f: f32,
    bins: &[f32],
    db: &[f32],
    x_cache: &[f32],
) -> Vec<[f32; 2]> {
    let dr = (MAX_DB - style.min_db).max(EPSILON);
    let y = |m: f32| ((m - style.min_db) / dr).clamp(0.0, 1.0);
    let mut out = Vec::with_capacity(x_cache.len());
    let mut xi = 0;
    let mut push = |m: f32| {
        let Some(&x) = x_cache.get(xi) else { return; };
        xi += 1;
        out.push([if style.reverse_frequency { 1.0 - x } else { x }, y(m)]);
    };

    push(value_at(bins, db, min_f));
    for (&f, &m) in bins.iter().zip(db) {
        if f > min_f && f < max_f {
            push(m);
        }
    }
    push(value_at(bins, db, max_f));
    if style.reverse_frequency {
        out.reverse();
    }
    out
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
        if !(min_f..=max_f).contains(&f) { return None; }
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

    let slot = Size::new(48.0_f32, 12.0);
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

            let mut text = make_text(fmt_freq(f), GRID_LABEL_SIZE, slot);
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
    if pk.opacity < 0.01 || b.width < 8.0 || b.height < 8.0 { return None; }
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
