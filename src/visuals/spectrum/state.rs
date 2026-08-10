// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{SpectrumSnapshot, SpectrumTraceSnapshot};
use super::render::{SpectrumParams, SpectrumPeakParams};
use crate::persistence::settings::SpectrumSettings;
use crate::visuals::options::{SpectrumDisplayMode, SpectrumWeightingMode};
use crate::util::audio::musical::NoteInfo;
use crate::util::audio::{Channel, FrequencyScale, fmt_freq};
use crate::util::color::{color_to_rgba, with_alpha};
use crate::util::lerp;
use crate::visuals::palettes::{self, spectrum::SIZE as PALETTE_SIZE};
use crate::visuals::render::common::{fill_bordered_rect, fill_rect, text as raw_text};
use iced::advanced::Renderer as _;
use iced::advanced::text::{Paragraph as _, Renderer as _};
use iced::advanced::graphics::text::Paragraph;
use iced::{Color, Point, Rectangle, Size};
use std::sync::{Arc, LazyLock};

const EPSILON: f32 = 1e-6;
const MIN_FREQUENCY: f32 = 20.0;
const MAX_DB: f32 = 0.0;
const GRID_LABEL_SIZE: f32 = 10.0;
const GRID_LABEL_GAP: f32 = 6.0;

struct PeakLabel {
    content: [String; 2],
    text: [Paragraph; 2],
    label_pos: [f32; 2],
    marker_pos: [f32; 2],
    opacity: f32,
}
type PeakUpdate = ([String; 2], [f32; 2]);
type GridTick = (f32, bool, Option<Paragraph>);
type SharedPoints = Arc<Vec<[f32; 2]>>;
static EMPTY_POINTS: LazyLock<SharedPoints> = LazyLock::new(|| Arc::new(Vec::new()));
fn rebuild_points(
    points: &mut SharedPoints,
    capacity: usize,
    build: impl FnOnce(&mut Vec<[f32; 2]>),
) {
    if Arc::strong_count(points) != 1 || points.capacity() != capacity {
        *points = Arc::new(Vec::with_capacity(capacity));
    }
    let points = Arc::get_mut(points).expect("unshared spectrum geometry");
    points.clear();
    build(points);
}

pub(crate) struct SpectrumState {
    pub(in crate::visuals) style: SpectrumSettings,
    pub(in crate::visuals) palette: [Color; PALETTE_SIZE],
    points: [SharedPoints; 2],
    geometry: crate::visuals::GeometryKey,
    peak: Option<PeakLabel>,
    effective_range: Option<(f32, f32)>,
    x_cache_key: (usize, u32, FrequencyScale),
    x_cache: Vec<f32>,
    grid_ticks: Vec<GridTick>,
}

impl SpectrumState {
    pub fn new() -> Self {
        Self {
            style: SpectrumSettings::default(),
            palette: palettes::spectrum::COLORS,
            points: std::array::from_fn(|_| Arc::clone(&EMPTY_POINTS)),
            geometry: crate::visuals::GeometryKey::new(),
            peak: None,
            effective_range: None,
            x_cache_key: (0, 0, FrequencyScale::default()),
            x_cache: Vec::new(),
            grid_ticks: Vec::new(),
        }
    }

    pub fn update_view_settings(&mut self, settings: &SpectrumSettings, floor_db: f32) {
        self.style = settings.clone();
        self.style.floor_db = floor_db;
        let _ = self.peak.take_if(|_| !settings.show_peak_label);
        self.invalidate_geometry();
    }

    crate::visuals::palette_setter!(PALETTE_SIZE => geometry);

    pub fn reset_audio(&mut self) {
        self.points.fill_with(|| Arc::clone(&EMPTY_POINTS));
        self.effective_range = None;
        self.peak = None;
        self.invalidate_geometry();
    }

    pub fn apply_snapshot(&mut self, snap: &SpectrumSnapshot) {
        let bins = snap.frequency_bins.len();
        let primary = (self.style.source != Channel::None).then_some(0);
        let secondary = match (self.style.source, self.style.secondary_source) {
            (_, Channel::None) => None,
            (primary, secondary) if primary == secondary => Some(0),
            _ => Some(1),
        };
        let min_f = MIN_FREQUENCY;
        let max_f = snap.frequency_bins[bins - 1].max(min_f * 1.02);
        let bins = snap.frequency_bins.as_slice();
        self.ensure_x_cache(min_f, max_f, bins);
        let style = &self.style;

        for ((points, trace), weighting) in self
            .points
            .iter_mut()
            .zip([primary, secondary])
            .zip([style.weighting_mode, style.secondary_weighting_mode])
        {
            let Some(trace) = trace else {
                *points = Arc::clone(&EMPTY_POINTS);
                continue;
            };
            rebuild_points(points, self.x_cache.len(), |points| {
                build_single_points_into(
                    points,
                    style,
                    min_f,
                    max_f,
                    bins,
                    trace_db(&snap.traces[trace], weighting),
                    &self.x_cache,
                );
            });
        }
        let pk = primary
            .filter(|_| self.style.show_peak_label)
            .and_then(|idx| self.build_peak(bins, trace_db(&snap.traces[idx], self.style.weighting_mode), min_f, max_f));
        self.effective_range = Some((min_f, max_f));
        self.fade_peak(pk);
        self.invalidate_geometry();
    }

    fn invalidate_geometry(&mut self) {
        self.geometry.invalidate();
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
        self.grid_ticks.clear();
        let exponents = min_f.max(1.0).log10().floor() as i32..=max_f.log10().ceil() as i32;
        self.grid_ticks.extend(
            exponents
                .flat_map(|exponent| {
                    let base = 10f32.powi(exponent);
                    (1..10).map(move |multiplier| (base * multiplier as f32, multiplier))
                })
                .filter(|(frequency, _)| (min_f..=max_f).contains(frequency))
                .map(|(frequency, multiplier)| {
                    let label = matches!(multiplier, 1 | 2 | 5).then(|| {
                        let text = raw_text(fmt_freq(frequency), GRID_LABEL_SIZE, Size::INFINITE);
                        Paragraph::with_text(text.as_ref())
                    });
                    (frequency, multiplier == 1, label)
                }),
        );
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
        let y = ((m - self.style.floor_db) / (MAX_DB - self.style.floor_db).max(EPSILON))
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
                for (index, content) in new.0.into_iter().enumerate() {
                    if p.content[index] != content {
                        p.text[index] = peak_text(&content, index);
                        p.content[index] = content;
                    }
                }
                p.label_pos = std::array::from_fn(|i| lerp(p.label_pos[i], new.1[i], 0.20));
                p.marker_pos = new.1;
                p.opacity = (0.65 * p.opacity + 0.35).min(1.0);
            }
            (Some(new), None) => {
                self.peak = Some(PeakLabel {
                    text: std::array::from_fn(|index| peak_text(&new.0[index], index)),
                    content: new.0,
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
            self.style.show_peak_label
                && self.style.source != Channel::None
                && self.points[0].len() >= 2
        })
    }

    fn visual_params(
        &self,
        bounds: Rectangle,
        theme: &iced::Theme,
        peak_layout: Option<PeakLayout>,
    ) -> Option<SpectrumParams> {
        let has_primary = self.style.source != Channel::None && self.points[0].len() >= 2;
        let has_secondary =
            self.style.secondary_source != Channel::None && self.points[1].len() >= 2;
        if !has_primary && !has_secondary { return None; }
        let pal = theme.extended_palette();

        let visible = |show: bool, points: &SharedPoints| {
            if show { Arc::clone(points) } else { Arc::clone(&EMPTY_POINTS) }
        };
        let peak = self.peak();
        let accent = self.palette[5];
        let (mut primary, mut secondary) = (
            visible(has_primary, &self.points[0]),
            visible(has_secondary, &self.points[1]),
        );
        if self.style.display_mode == SpectrumDisplayMode::Bar && primary.is_empty() {
            std::mem::swap(&mut primary, &mut secondary);
        }

        Some(SpectrumParams {
            bounds,
            normalized_points: primary,
            secondary_points: secondary,
            geometry: self.geometry,
            line_color: color_to_rgba(with_alpha(pal.background.base.text, 0.92)),
            secondary_line_color: color_to_rgba(with_alpha(pal.secondary.weak.text, 0.32)),
            highlight_threshold: self.style.highlight_threshold,
            spectrum_palette: self.palette.map(color_to_rgba),
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
    if let Some(range) = state.effective_range.filter(|_| state.style.show_grid) {
        r.with_layer(b, |r| draw_grid(r, th, b, range, &state));
    }
    r.draw_primitive(b, params);
    if let Some((pk, layout)) = peak.zip(peak_layout) {
        let accent = state.palette[5];
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
    (bins.partition_point(|&f| f < min_f).max(1)
        ..bins.partition_point(|&f| f <= max_f).min(bins.len().saturating_sub(1)))
        .filter(|&i| db[i].is_finite())
        .max_by(|&a, &b| db[a].total_cmp(&db[b]))
}

fn interpolated_peak(bins: &[f32], db: &[f32], bin: usize) -> Option<(f32, f32)> {
    let next = bin.checked_add(1)?;
    if bins.len() != db.len() || bin == 0 || next >= bins.len() { return None; }
    let bin_hz = bins[1] - bins[0];
    let (center_freq, center) = (bins[bin], db[bin]);
    if crate::util::finite_positive(bin_hz).is_none()
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
    fn theme_colors_invalidate_cached_geometry_without_audio_update() {
        let mut state = SpectrumState::new();
        state.style.source = Channel::Left;
        state.points[0] = Arc::new(vec![[0.0, 0.0], [1.0, 1.0]]);
        let bounds = Rectangle {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 50.0,
        };

        let dark = state
            .visual_params(bounds, &iced::Theme::Dark, None)
            .unwrap();
        let light = state
            .visual_params(bounds, &iced::Theme::Light, None)
            .unwrap();

        assert_eq!(dark.geometry.revision, light.geometry.revision);
        assert_ne!(dark.line_color, light.line_color);
        assert_ne!(dark.geometry_fingerprint(), light.geometry_fingerprint());
    }

    #[test]
    fn secondary_trace_can_render_without_primary_source() {
        let trace = [vec![-20.0; 3], vec![-20.0; 3]];
        let mut state = SpectrumState::new();
        state.style.source = Channel::None;
        state.style.secondary_source = Channel::Left;

        state.apply_snapshot(&SpectrumSnapshot {
            frequency_bins: vec![0.0, 20.0, 40.0],
            traces: [SpectrumTraceSnapshot::default(), trace],
        });

        assert!(state.points[0].is_empty());
        assert!(state.points[1].len() >= 2);
        assert!(state.peak().is_none());
    }

    #[test]
    fn point_build_emits_only_finite_coordinates() {
        let mut points = Vec::new();
        build_single_points_into(
            &mut points,
            &SpectrumSettings::default(),
            20.0,
            40.0,
            &[0.0, 20.0, 30.0, 40.0],
            &[0.0, f32::NAN, -10.0, f32::INFINITY],
            &[0.0, 0.5, 1.0],
        );

        assert_eq!(points.len(), 2);
        assert!(points.iter().flatten().all(|value| value.is_finite()));
    }
}

fn peak_text(content: &str, index: usize) -> Paragraph {
    Paragraph::with_text(raw_text(content, [12.0, 10.0][index], Size::INFINITE).as_ref())
}

fn trace_db(trace: &SpectrumTraceSnapshot, mode: SpectrumWeightingMode) -> &[f32] {
    &trace[match mode {
        SpectrumWeightingMode::AWeighted => 0,
        SpectrumWeightingMode::Raw => 1,
    }]
}

fn build_single_points_into(
    out: &mut Vec<[f32; 2]>,
    style: &SpectrumSettings,
    min_f: f32,
    max_f: f32,
    bins: &[f32],
    db: &[f32],
    x_cache: &[f32],
) {
    let dr = (MAX_DB - style.floor_db).max(EPSILON);
    let y = |m: f32| ((m - style.floor_db) / dr).clamp(0.0, 1.0);
    let mut push = |x, m| {
        let y = y(m);
        if y.is_finite() {
            out.push([if style.reverse_frequency { 1.0 - x } else { x }, y]);
        }
    };

    let interior = bins.partition_point(|&f| f <= min_f)..bins.partition_point(|&f| f < max_f);
    push(x_cache[0], value_at(bins, db, min_f));
    for (&x, &m) in x_cache[1..x_cache.len() - 1].iter().zip(&db[interior]) { push(x, m); }
    push(x_cache[x_cache.len() - 1], value_at(bins, db, max_f));
    if style.reverse_frequency {
        out.reverse();
    }
}

fn draw_grid(
    r: &mut iced::Renderer,
    th: &iced::Theme,
    b: Rectangle,
    (min_f, max_f): (f32, f32),
    state: &SpectrumState,
) {
    if b.width <= 0.0 || b.height <= 0.0 {
        return;
    }
    let style = &state.style;
    let reverse = style.reverse_frequency;
    let scale = style.frequency_scale;
    let (scaled_min, scaled_max) = (scale.scale(min_f), scale.scale(max_f));
    let scaled_span = (scaled_max - scaled_min).max(EPSILON);
    let pal = th.extended_palette();
    let txt = pal.background.base.text;
    let (major_lc, major_tc) = (with_alpha(txt, 0.25), with_alpha(txt, 0.75));
    let (minor_lc, minor_tc) = (with_alpha(txt, 0.10), with_alpha(txt, 0.20));

    let tick_x = |f: f32| -> Option<f32> {
        if !(min_f..=max_f).contains(&f) { return None; }
        let pos = ((scale.scale(f) - scaled_min) / scaled_span).clamp(0.0, 1.0);
        pos.is_finite()
            .then_some(b.x + b.width * if reverse { 1.0 - pos } else { pos })
    };
    let vline = |r: &mut iced::Renderer, x: f32, top: f32, h: f32, c: Color| {
        let sx = (x - 0.5).clamp(b.x, (b.x + b.width - 1.0).max(b.x));
        fill_rect(r, Rectangle::new(Point::new(sx, top), Size::new(1.0, h)), c);
    };

    let slot = Size::new(48.0_f32, 12.0);
    let ty = b.y + GRID_LABEL_GAP;
    let clamp_lo = b.x + GRID_LABEL_GAP;
    let clamp_hi = (b.x + b.width - GRID_LABEL_GAP - slot.width).max(clamp_lo);
    let mut last_right = f32::NEG_INFINITY;
    let mut draw_tick = |(frequency, major, text): &GridTick| {
        let Some(x) = tick_x(*frequency) else { return };
        let (lc, tc) = if *major {
            (major_lc, major_tc)
        } else {
            (minor_lc, minor_tc)
        };
        vline(r, x, b.y, b.height, lc);
        let Some(text) = text else { return };

        let tx = (x - slot.width * 0.5).clamp(clamp_lo, clamp_hi);
        if tx < last_right {
            return;
        }
        last_right = tx + slot.width + GRID_LABEL_GAP;
        r.fill_paragraph(
            text,
            Point::new(tx + (slot.width - text.min_bounds().width) * 0.5, ty),
            tc,
            Rectangle::new(Point::new(tx, ty), slot),
        );
    };
    if reverse {
        state.grid_ticks.iter().rev().for_each(&mut draw_tick);
    } else {
        state.grid_ticks.iter().for_each(draw_tick);
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
    let title = pk.text[0].min_bounds();
    let detail = pk.text[1].min_bounds();
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
    fill_bordered_rect(
        r,
        layout.rect,
        with_alpha(pal.background.strong.color, 0.90 * pk.opacity),
        iced::Border {
            color: with_alpha(accent, 0.50 * pk.opacity),
            width: 1.0,
            radius: 2.0.into(),
        },
        true,
    );
    r.fill_paragraph(
        &pk.text[0],
        layout.text,
        with_alpha(pal.background.base.text, pk.opacity),
        Rectangle::new(layout.text, layout.title),
    );
    let pos = Point::new(layout.text.x, layout.text.y + layout.title.height + 2.0);
    r.fill_paragraph(
        &pk.text[1],
        pos,
        with_alpha(pal.secondary.weak.text, 0.84 * pk.opacity),
        Rectangle::new(pos, layout.detail),
    );
}
