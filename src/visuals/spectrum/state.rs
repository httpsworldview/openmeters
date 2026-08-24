// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{SpectrumSnapshot, SpectrumTraceSnapshot};
use super::render::{MIN_TRACE_POINTS, SpectrumParams, SpectrumPeakParams};
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
use iced::{Color, Padding, Point, Rectangle, Size};
use std::sync::{Arc, LazyLock};

const EPSILON: f32 = 1e-6;
const MIN_FREQUENCY: f32 = 20.0;
const MAX_DB: f32 = 0.0;
const GRID_LABEL_SIZE: f32 = 10.0;
const GRID_LABEL_GAP: f32 = 6.0;
const PEAK_PALETTE_INDEX: usize = PALETTE_SIZE - 1;
const MIN_PEAK_OPACITY: f32 = 0.01;

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
        if !settings.show_peak_label { self.peak = None; }
        self.geometry.invalidate();
    }

    crate::visuals::palette_setter!(PALETTE_SIZE => geometry);

    pub fn reset_audio(&mut self) {
        self.points.fill_with(|| Arc::clone(&EMPTY_POINTS));
        self.effective_range = None;
        self.peak = None;
        self.geometry.invalidate();
    }

    pub fn apply_snapshot(&mut self, snap: &SpectrumSnapshot) {
        const MIN_DISPLAY_RANGE_FACTOR: f32 = 1.02;
        let bins = snap.frequency_bins.as_slice();
        let primary_trace = (self.style.source != Channel::None).then_some(0);
        let secondary_trace = match (self.style.source, self.style.secondary_source) {
            (_, Channel::None) => None,
            (primary_source, secondary_source) if primary_source == secondary_source => Some(0),
            _ => Some(1),
        };
        let min_f = MIN_FREQUENCY;
        let max_f = bins[bins.len() - 1].max(min_f * MIN_DISPLAY_RANGE_FACTOR);
        self.ensure_x_cache(min_f, max_f, bins);
        let style = &self.style;

        for ((points, trace), weighting) in self
            .points
            .iter_mut()
            .zip([primary_trace, secondary_trace])
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
        let peak = primary_trace
            .filter(|_| style.show_peak_label)
            .and_then(|trace| self.build_peak(bins, trace_db(&snap.traces[trace], style.weighting_mode), min_f, max_f));
        self.effective_range = Some((min_f, max_f));
        self.fade_peak(peak);
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
            self.x_cache
                .push(scale.pos_of(min_f, max_f, f).clamp(0.0, 1.0));
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
        const MIN_NORMALIZED_LEVEL: f32 = 0.08;
        let bin = peak_bin(bins, db, min_f, max_f)?;
        let (f, m) = interpolated_peak(bins, db, bin);
        let t = self.style.frequency_scale.pos_of(min_f, max_f, f);
        let x = if self.style.reverse_frequency { 1.0 - t } else { t }.clamp(0.0, 1.0);
        let y = ((m - self.style.floor_db) / (MAX_DB - self.style.floor_db).max(EPSILON))
            .clamp(0.0, 1.0);
        if y < MIN_NORMALIZED_LEVEL { return None; }
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
        const POSITION_TRACKING_RATE: f32 = 0.20;
        const FADE_IN_RATE: f32 = 0.35;
        const FADE_OUT_RETENTION: f32 = 0.88;
        match (incoming, &mut self.peak) {
            (Some((contents, position)), Some(peak)) => {
                for (index, content) in contents.into_iter().enumerate() {
                    if peak.content[index] != content {
                        peak.text[index] = peak_text(&content, index);
                        peak.content[index] = content;
                    }
                }
                peak.label_pos = std::array::from_fn(|i| lerp(peak.label_pos[i], position[i], POSITION_TRACKING_RATE));
                peak.marker_pos = position;
                peak.opacity = lerp(peak.opacity, 1.0, FADE_IN_RATE).min(1.0);
            }
            (Some((contents, position)), None) => {
                self.peak = Some(PeakLabel {
                    text: std::array::from_fn(|index| peak_text(&contents[index], index)),
                    content: contents,
                    label_pos: position,
                    marker_pos: position,
                    opacity: 1.0,
                });
            }
            (None, Some(peak)) => {
                peak.opacity *= FADE_OUT_RETENTION;
                if peak.opacity < MIN_PEAK_OPACITY {
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
                && self.points[0].len() >= MIN_TRACE_POINTS
        })
    }

    pub(in crate::visuals) fn ignores_audio(&self) -> bool {
        [self.style.source, self.style.secondary_source] == [Channel::None; 2]
    }

    pub(in crate::visuals) fn is_quiescent(&self) -> bool {
        let quiet = [self.style.source, self.style.secondary_source]
            .into_iter()
            .zip(&self.points)
            .filter(|(source, _)| *source != Channel::None)
            .all(|(_, points)| points.iter().all(|point| point[1] == 0.0));
        self.ignores_audio()
            || (self.effective_range.is_some() && self.peak.is_none() && quiet)
    }

    fn visual_params(
        &self,
        bounds: Rectangle,
        theme: &iced::Theme,
        peak_layout: Option<PeakLayout>,
    ) -> Option<SpectrumParams> {
        const AUXILIARY_LINE_ALPHA: f32 = 0.32;
        let style = &self.style;
        let visible_points = |source: Channel, points: &SharedPoints| {
            let visible = source != Channel::None && points.len() >= MIN_TRACE_POINTS;
            Arc::clone(if visible { points } else { &EMPTY_POINTS })
        };
        let (mut primary, mut secondary) = (
            visible_points(style.source, &self.points[0]),
            visible_points(style.secondary_source, &self.points[1]),
        );
        if primary.is_empty() && secondary.is_empty() { return None; }
        if style.display_mode == SpectrumDisplayMode::Bar && primary.is_empty() {
            std::mem::swap(&mut primary, &mut secondary);
        }

        let theme_palette = theme.extended_palette();
        let primary_color = theme_palette.background.base.text;
        let secondary_color = theme_palette.secondary.weak.text;
        let peak_color = self.palette[PEAK_PALETTE_INDEX];

        Some(SpectrumParams {
            bounds,
            normalized_points: primary,
            secondary_points: secondary,
            geometry: self.geometry,
            line_color: color_to_rgba(with_alpha(primary_color, 0.92)),
            secondary_line_color: color_to_rgba(with_alpha(secondary_color, AUXILIARY_LINE_ALPHA)),
            highlight_threshold: style.highlight_threshold,
            spectrum_palette: self.palette.map(color_to_rgba),
            display_mode: style.display_mode,
            bar_count: style.bar_count,
            bar_gap: style.bar_gap,
            peak: self.peak().map(|peak| SpectrumPeakParams {
                marker: peak.marker_pos,
                marker_color: color_to_rgba(with_alpha(peak_color, peak.opacity * 0.95)),
                leader_anchor: peak_layout.map(|layout| point_to_normalized(bounds, layout.leader_anchor)),
                leader_color: color_to_rgba(with_alpha(peak_color, peak.opacity * AUXILIARY_LINE_ALPHA)),
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
    if let Some((peak, layout)) = peak.zip(peak_layout) {
        let peak_color = state.palette[PEAK_PALETTE_INDEX];
        r.with_layer(b, |r| draw_peak(r, th, peak, layout, peak_color));
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
        ..bins.partition_point(|&f| f <= max_f).min(bins.len() - 1))
        .filter(|&i| db[i].is_finite())
        .max_by(|&a, &b| db[a].total_cmp(&db[b]))
}

fn interpolated_peak(bins: &[f32], db: &[f32], bin: usize) -> (f32, f32) {
    let next = bin + 1;
    let bin_hz = bins[1] - bins[0];
    let (center_freq, center) = (bins[bin], db[bin]);
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
    ((center_freq + offset * bin_hz).max(0.0), level)
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
    fn secondary_trace_renders_without_primary_source() {
        let trace = [vec![-20.0; 3], vec![-20.0; 3]];
        let mut state = SpectrumState::new();
        state.style.source = Channel::None;
        state.style.secondary_source = Channel::Left;
        state.apply_snapshot(&SpectrumSnapshot {
            frequency_bins: vec![0.0, 20.0, 40.0],
            traces: [SpectrumTraceSnapshot::default(), trace],
        });
        let bounds = Rectangle::new(Point::ORIGIN, Size::new(100.0, 50.0));

        let line = state.visual_params(bounds, &iced::Theme::Dark, None).unwrap();
        assert!(line.normalized_points.is_empty());
        assert!(line.secondary_points.len() >= 2);
        assert!(line.peak.is_none());

        state.style.display_mode = SpectrumDisplayMode::Bar;
        let bars = state.visual_params(bounds, &iced::Theme::Dark, None).unwrap();
        assert!(bars.normalized_points.len() >= 2);
        assert!(bars.secondary_points.is_empty());
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
    let theme_palette = th.extended_palette();
    let text_color = theme_palette.background.base.text;
    let major_colors = (with_alpha(text_color, 0.25), with_alpha(text_color, 0.75));
    let minor_colors = (with_alpha(text_color, 0.10), with_alpha(text_color, 0.20));

    let tick_x = |f: f32| -> Option<f32> {
        if !(min_f..=max_f).contains(&f) { return None; }
        let pos = ((scale.scale(f) - scaled_min) / scaled_span).clamp(0.0, 1.0);
        pos.is_finite()
            .then_some(b.x + b.width * if reverse { 1.0 - pos } else { pos })
    };
    let draw_vertical_line = |r: &mut iced::Renderer, x: f32, top: f32, h: f32, color: Color| {
        let x = (x - 0.5).clamp(b.x, (b.x + b.width - 1.0).max(b.x));
        fill_rect(r, Rectangle::new(Point::new(x, top), Size::new(1.0, h)), color);
    };

    let label_slot = Size::new(48.0, 12.0);
    let label_y = b.y + GRID_LABEL_GAP;
    let min_label_x = b.x + GRID_LABEL_GAP;
    let max_label_x = (b.x + b.width - GRID_LABEL_GAP - label_slot.width).max(min_label_x);
    let mut last_right = f32::NEG_INFINITY;
    let mut draw_tick = |(frequency, major, text): &GridTick| {
        let Some(x) = tick_x(*frequency) else { return };
        let (line_color, text_color) = if *major { major_colors } else { minor_colors };
        draw_vertical_line(r, x, b.y, b.height, line_color);
        let Some(text) = text else { return };

        let label_x = (x - label_slot.width * 0.5).clamp(min_label_x, max_label_x);
        if label_x < last_right {
            return;
        }
        last_right = label_x + label_slot.width + GRID_LABEL_GAP;
        r.fill_paragraph(
            text,
            Point::new(label_x + (label_slot.width - text.min_bounds().width) * 0.5, label_y),
            text_color,
            Rectangle::new(Point::new(label_x, label_y), label_slot),
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
    text_layouts: [(Point, Size); 2],
    leader_anchor: Point,
}

fn point_to_normalized(b: Rectangle, p: Point) -> [f32; 2] {
    [(p.x - b.x) / b.width, 1.0 - (p.y - b.y) / b.height]
}

fn peak_label_layout(b: Rectangle, peak: &PeakLabel) -> Option<PeakLayout> {
    const MIN_VIEW_SIZE: f32 = 8.0;
    const LABEL_GAP: f32 = 8.0;
    const LINE_GAP: f32 = 2.0;
    if peak.opacity < MIN_PEAK_OPACITY || b.width < MIN_VIEW_SIZE || b.height < MIN_VIEW_SIZE { return None; }
    let [title, detail] = peak.text.each_ref().map(|text| text.min_bounds());
    let [px, py] = peak.label_pos;
    let peak_pos = Point::new(b.x + b.width * px, b.y + b.height * (1.0 - py));
    let padding = Padding::new(7.0).top(6.0).bottom(5.0);
    let size = Size::new(
        title.width.max(detail.width) + padding.x(),
        title.height + detail.height + padding.y() + LINE_GAP,
    );
    let fits_right = peak_pos.x + size.width + LABEL_GAP <= b.x + b.width;
    let x = if fits_right { peak_pos.x + LABEL_GAP } else { peak_pos.x - size.width - LABEL_GAP }
        .clamp(b.x, (b.x + b.width - size.width).max(b.x));
    let y = (peak_pos.y - size.height - LABEL_GAP).clamp(b.y, (b.y + b.height - size.height).max(b.y));
    let title_pos = Point::new(x + padding.left, y + padding.top);
    let detail_pos = Point::new(title_pos.x, title_pos.y + title.height + LINE_GAP);
    Some(PeakLayout {
        rect: Rectangle::new(Point::new(x, y), size),
        text_layouts: [(title_pos, title), (detail_pos, detail)],
        leader_anchor: Point::new(if fits_right { x } else { x + size.width }, y + size.height),
    })
}

fn draw_peak(
    r: &mut iced::Renderer,
    th: &iced::Theme,
    peak: &PeakLabel,
    layout: PeakLayout,
    peak_color: Color,
) {
    let theme_palette = th.extended_palette();
    let [(title_pos, title_bounds), (detail_pos, detail_bounds)] = layout.text_layouts;
    fill_bordered_rect(
        r,
        layout.rect,
        with_alpha(theme_palette.background.strong.color, 0.90 * peak.opacity),
        iced::Border {
            color: with_alpha(peak_color, 0.50 * peak.opacity),
            width: 1.0,
            radius: 2.0.into(),
        },
        true,
    );
    r.fill_paragraph(
        &peak.text[0],
        title_pos,
        with_alpha(theme_palette.background.base.text, peak.opacity),
        Rectangle::new(title_pos, title_bounds),
    );
    r.fill_paragraph(
        &peak.text[1],
        detail_pos,
        with_alpha(theme_palette.secondary.weak.text, 0.84 * peak.opacity),
        Rectangle::new(detail_pos, detail_bounds),
    );
}
