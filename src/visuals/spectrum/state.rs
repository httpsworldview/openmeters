// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{SpectrumSnapshot, SpectrumTraceSnapshot};
use super::render::{
    MIN_TRACE_POINTS, RAMP_SIZE, SpectrumCutoutParams, SpectrumParams, SpectrumPeakParams,
};
use crate::persistence::settings::SpectrumSettings;
use crate::util::audio::musical::NoteInfo;
use crate::util::audio::{Channel, DISPLAY_MIN_HZ, FrequencyScale, fmt_freq, smoothing_weight};
use crate::util::color::{color_to_rgba, with_alpha};
use crate::util::lerp;
use crate::visuals::options::{SpectrumDisplayMode, SpectrumWeightingMode};
use crate::visuals::palettes::{self, spectrum::SIZE as PALETTE_SIZE};
use crate::visuals::render::common::{
    QuarterTurn, fill_bordered_rect, fill_rect, text as raw_text,
};
use iced::advanced::Renderer as _;
use iced::advanced::graphics::text::Paragraph;
use iced::advanced::text::{Paragraph as _, Renderer as _};
use iced::{Color, Padding, Point, Rectangle, Size};
use std::sync::{Arc, LazyLock};

const EPSILON: f32 = 1e-6;
const MAX_DB: f32 = 0.0;
// Lower ticks crowd together, and a tick at the 1 Hz edge reads as a bound.
const MIN_GRID_HZ: f32 = 10.0;
const GRID_LABEL_SIZE: f32 = 10.0;
const GRID_LABEL_GAP: f32 = 6.0;
const GRID_LABEL_PADDING_X: f32 = 4.0;
const GRID_LABEL_PADDING_Y: f32 = 2.0;
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
#[derive(Clone, Copy)]
struct GridLabelLayout {
    tick_index: usize,
    bounds: Rectangle,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct GridLayoutKey {
    bounds: [u32; 4],
    range: [u32; 2],
    scale: FrequencyScale,
    reverse: bool,
    turn: QuarterTurn,
}
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

crate::macros::default_struct! {
    pub(crate) struct SpectrumState {
        pub(in crate::visuals) style: SpectrumSettings = SpectrumSettings::default(),
        pub(in crate::visuals) palette: [Color; PALETTE_SIZE] = palettes::spectrum::COLORS,
        pub(in crate::visuals) stop_positions: [f32; RAMP_SIZE] = palettes::spectrum::DEFAULT_POSITIONS,
        pub(in crate::visuals) stop_spreads: [f32; RAMP_SIZE] = [1.0; RAMP_SIZE],
        points: [SharedPoints; 2] = std::array::from_fn(|_| Arc::clone(&EMPTY_POINTS)),
        geometry: crate::visuals::GeometryKey = crate::visuals::GeometryKey::new(),
        peak: Option<PeakLabel> = None,
        effective_range: Option<(f32, f32)> = None,
        x_cache_key: (usize, u32, FrequencyScale) = (0, 0, FrequencyScale::default()),
        x_cache: Vec<f32> = Vec::new(),
        grid_ticks: Vec<GridTick> = Vec::new(),
        grid_labels: Vec<GridLabelLayout> = Vec::new(),
        grid_layout_key: Option<GridLayoutKey> = None,
        grid_layout_revision: u64 = 0,
        grid_cutouts: Arc<Vec<Rectangle>> = Arc::new(Vec::new()),
    }
}

impl SpectrumState {
    pub fn update_view_settings(
        &mut self,
        settings: &SpectrumSettings,
        snapshot: &SpectrumSnapshot,
    ) {
        self.style = settings.clone();
        // Do not animate from peak coordinates in the old projection.
        self.peak = None;
        self.apply_snapshot(snapshot);
    }

    crate::visuals::palette_setter!(PALETTE_SIZE => geometry);

    fn turn(&self) -> QuarterTurn {
        QuarterTurn::new(self.style.rotation)
    }

    pub fn reset_audio(&mut self) {
        self.points.fill_with(|| Arc::clone(&EMPTY_POINTS));
        self.effective_range = None;
        self.peak = None;
        self.clear_grid_layout();
        self.geometry.invalidate();
    }

    pub fn apply_snapshot(&mut self, snap: &SpectrumSnapshot) {
        const MIN_DISPLAY_RANGE_FACTOR: f32 = 1.02;
        let bins = snap.frequency_bins.as_slice();
        let Some(&max_f) = bins.last() else {
            self.reset_audio();
            return;
        };
        let primary_trace = (self.style.source != Channel::None).then_some(0);
        let secondary_trace = match (self.style.source, self.style.secondary_source) {
            (_, Channel::None) => None,
            (primary_source, secondary_source) if primary_source == secondary_source => Some(0),
            _ => Some(1),
        };
        let min_f = DISPLAY_MIN_HZ;
        let max_f = max_f.max(min_f * MIN_DISPLAY_RANGE_FACTOR);
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
                    snap,
                    trace_db(&snap.traces[trace], weighting),
                    &self.x_cache,
                );
            });
        }
        let peak = primary_trace
            .filter(|_| style.show_peak_label)
            .and_then(|trace| {
                self.build_peak(
                    snap,
                    trace_db(&snap.traces[trace], style.weighting_mode),
                    min_f,
                    max_f,
                )
            });
        self.effective_range = Some((min_f, max_f));
        self.fade_peak(peak, snap.elapsed);
        self.geometry.invalidate();
    }

    fn ensure_x_cache(&mut self, min_f: f32, max_f: f32, bins: &[f32]) {
        let scale = self.style.frequency_scale;
        let key = (bins.len(), max_f.to_bits(), scale);
        if self.x_cache_key == key {
            return;
        }

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
        let exponents = MIN_GRID_HZ.log10().floor() as i32..=max_f.log10().ceil() as i32;
        self.grid_ticks.extend(
            exponents
                .flat_map(|exponent| {
                    let base = 10f32.powi(exponent);
                    (1..10).map(move |multiplier| (base * multiplier as f32, multiplier))
                })
                .filter(|(frequency, _)| (MIN_GRID_HZ..=max_f).contains(frequency))
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

    fn clear_grid_layout(&mut self) {
        if self.grid_layout_key.take().is_none() {
            return;
        }
        self.grid_labels.clear();
        self.grid_cutouts = Arc::new(Vec::new());
        self.grid_layout_revision = self.grid_layout_revision.wrapping_add(1);
    }

    fn layout_grid_labels(&mut self, bounds: Rectangle, range: (f32, f32)) {
        let key = GridLayoutKey {
            bounds: [bounds.x, bounds.y, bounds.width, bounds.height].map(f32::to_bits),
            range: [range.0.to_bits(), range.1.to_bits()],
            scale: self.style.frequency_scale,
            reverse: self.style.reverse_frequency,
            turn: self.turn(),
        };
        if self.grid_layout_key == Some(key) {
            return;
        }
        self.grid_layout_key = Some(key);
        self.grid_layout_revision = self.grid_layout_revision.wrapping_add(1);
        self.grid_labels.clear();
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            self.grid_cutouts = Arc::new(Vec::new());
            return;
        }

        // Lay out in the unturned frame, then turn the finished labels.
        let turn = self.turn();
        let style = &self.style;
        let frame = turn.frame(bounds);
        let label_y = frame.y + GRID_LABEL_GAP;
        let min_label_x = frame.x + GRID_LABEL_GAP;
        let ticks = &self.grid_ticks;
        let labels = &mut self.grid_labels;
        let tick_x = grid_tick_x(frame, range, style);
        let mut layout_tick = |(tick_index, tick): (usize, &GridTick)| {
            let (frequency, major, Some(text)) = tick else {
                return;
            };
            let Some(x) = tick_x(*frequency) else {
                return;
            };
            let text_bounds = text.min_bounds();
            let label_size = turn.frame_size(Size::new(
                text_bounds.width + GRID_LABEL_PADDING_X * 2.0,
                text_bounds.height + GRID_LABEL_PADDING_Y * 2.0,
            ));
            let max_label_x =
                (frame.x + frame.width - GRID_LABEL_GAP - label_size.width).max(min_label_x);
            let label_x = (x - label_size.width * 0.5).clamp(min_label_x, max_label_x);
            // Placed labels are in frame order, so only a suffix can overlap.
            let keep = labels.partition_point(|placed| {
                placed.bounds.x + placed.bounds.width + GRID_LABEL_GAP <= label_x
            });
            // Decade labels evict overlapping 2x/5x labels; anything else yields.
            let overlapped = &labels[keep..];
            if !overlapped.is_empty()
                && (!major || overlapped.iter().any(|placed| ticks[placed.tick_index].1))
            {
                return;
            }
            labels.truncate(keep);
            labels.push(GridLabelLayout {
                tick_index,
                bounds: Rectangle::new(Point::new(label_x, label_y), label_size),
            });
        };
        if style.reverse_frequency {
            ticks.iter().enumerate().rev().for_each(&mut layout_tick);
        } else {
            ticks.iter().enumerate().for_each(layout_tick);
        }
        for label in &mut self.grid_labels {
            label.bounds = turn.to_screen(bounds, label.bounds);
        }
        self.grid_cutouts = Arc::new(self.grid_labels.iter().map(|label| label.bounds).collect());
    }

    fn build_peak(
        &self,
        snap: &SpectrumSnapshot,
        db: &[f32],
        min_f: f32,
        max_f: f32,
    ) -> Option<PeakUpdate> {
        const MIN_NORMALIZED_LEVEL: f32 = 0.08;
        let bins = &snap.frequency_bins;
        let bin = peak_bin(bins, db, min_f, max_f)?;
        if db[bin] <= snap.floor_db {
            return None;
        }
        let (f, m) = interpolated_peak(bins, db, bin);
        let x = view_x(&self.style, (min_f, max_f), f);
        let y = ((m - self.style.floor_db) / (MAX_DB - self.style.floor_db).max(EPSILON))
            .clamp(0.0, 1.0);
        if y < MIN_NORMALIZED_LEVEL {
            return None;
        }
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

    fn fade_peak(&mut self, incoming: Option<PeakUpdate>, elapsed: f32) {
        const POSITION_TRACKING: f32 = 0.1;
        const FADE_IN: f32 = 0.05;
        const FADE_OUT: f32 = 0.17;
        match (incoming, &mut self.peak) {
            (Some((contents, position)), Some(peak)) => {
                for (index, content) in contents.into_iter().enumerate() {
                    if peak.content[index] != content {
                        peak.text[index] = peak_text(&content, index);
                        peak.content[index] = content;
                    }
                }
                let tracking = smoothing_weight(elapsed, POSITION_TRACKING);
                peak.label_pos =
                    std::array::from_fn(|i| lerp(peak.label_pos[i], position[i], tracking));
                peak.marker_pos = position;
                peak.opacity = lerp(peak.opacity, 1.0, smoothing_weight(elapsed, FADE_IN)).min(1.0);
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
                peak.opacity *= 1.0 - smoothing_weight(elapsed, FADE_OUT);
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
        self.ignores_audio() || (self.effective_range.is_some() && self.peak.is_none() && quiet)
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
        let mut primary = visible_points(style.source, &self.points[0]);
        let mut secondary = visible_points(style.secondary_source, &self.points[1]);
        if primary.is_empty() && secondary.is_empty() {
            return None;
        }
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
            ramp: std::array::from_fn(|i| color_to_rgba(self.palette[i])),
            stop_positions: self.stop_positions,
            stop_spreads: self.stop_spreads,
            display_mode: style.display_mode,
            turn: self.turn(),
            bar_count: style.bar_count,
            bar_gap: style.bar_gap,
            peak: self.peak().map(|peak| SpectrumPeakParams {
                marker: peak.marker_pos,
                marker_color: color_to_rgba(with_alpha(peak_color, peak.opacity * 0.95)),
                leader_anchor: peak_layout.map(|layout| layout.leader_anchor),
                leader_color: color_to_rgba(with_alpha(
                    peak_color,
                    peak.opacity * AUXILIARY_LINE_ALPHA,
                )),
            }),
        })
    }

    fn cutout_params(
        &self,
        bounds: Rectangle,
        theme: &iced::Theme,
    ) -> Option<SpectrumCutoutParams> {
        (!self.grid_cutouts.is_empty()).then(|| SpectrumCutoutParams {
            bounds,
            rectangles: Arc::clone(&self.grid_cutouts),
            geometry: self.geometry,
            revision: self.grid_layout_revision,
            // Replace instead of blending so transparent backgrounds keep their alpha.
            background: color_to_rgba(theme.extended_palette().background.base.color),
        })
    }
}

crate::visuals::visualization_widget!(Spectrum, SpectrumState, |this, r, th, b| {
    let mut state = this.state.borrow_mut();
    let grid_range = state.effective_range.filter(|_| state.style.show_grid);
    if let Some(range) = grid_range {
        state.layout_grid_labels(b, range);
    } else {
        state.clear_grid_layout();
    }

    let peak = state.peak();
    let peak_layout = peak.and_then(|p| peak_label_layout(b, state.turn(), p));
    let Some(params) = state.visual_params(b, th, peak_layout) else {
        return;
    };
    if let Some(range) = grid_range {
        r.with_layer(b, |r| {
            draw_grid_lines(r, th, b, range, &state);
            if let Some(cutouts) = state.cutout_params(b, th) {
                r.draw_primitive(b, cutouts);
            }
            draw_grid_labels(r, th, &state);
        });
    }
    r.with_layer(b, |r| r.draw_primitive(b, params));
    if let Some((peak, layout)) = peak.zip(peak_layout) {
        let peak_color = state.palette[PEAK_PALETTE_INDEX];
        r.with_layer(b, |r| draw_peak(r, th, peak, layout, peak_color));
    }
});

fn value_at(bins: &[f32], mags: &[f32], f: f32) -> f32 {
    let i = bins.partition_point(|&bin| bin < f);
    if i == 0 {
        return mags[0];
    }
    if i >= bins.len() {
        return mags[bins.len() - 1];
    }
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
    fn secondary_trace_renders_without_primary_source() {
        let trace = [vec![-20.0; 3], vec![-20.0; 3]];
        let mut state = SpectrumState::default();
        state.style.source = Channel::None;
        state.style.secondary_source = Channel::Left;
        state.apply_snapshot(&SpectrumSnapshot {
            floor_db: state.style.floor_db,
            frequency_bins: vec![0.0, 20.0, 40.0],
            traces: [SpectrumTraceSnapshot::default(), trace],
            ..Default::default()
        });
        let bounds = Rectangle::new(Point::ORIGIN, Size::new(100.0, 50.0));

        let line = state
            .visual_params(bounds, &iced::Theme::Dark, None)
            .unwrap();
        assert!(line.normalized_points.is_empty());
        assert!(line.secondary_points.len() >= 2);
        assert!(line.peak.is_none());

        state.style.display_mode = SpectrumDisplayMode::Bar;
        let bars = state
            .visual_params(bounds, &iced::Theme::Dark, None)
            .unwrap();
        assert!(bars.normalized_points.len() >= 2);
        assert!(bars.secondary_points.is_empty());
    }

    #[test]
    fn view_changes_rebuild_frozen_traces_and_labels() {
        let snapshot = SpectrumSnapshot {
            floor_db: -120.0,
            frequency_bins: vec![0.0, 6_000.0, 12_000.0, 18_000.0, 24_000.0],
            traces: std::array::from_fn(|_| {
                [
                    vec![-120.0, -60.0, -30.0, -90.0, -120.0],
                    vec![-120.0, -30.0, -12.0, -60.0, -120.0],
                ]
            }),
            ..Default::default()
        };
        let baseline = SpectrumSettings {
            show_peak_label: true,
            ..Default::default()
        };
        let mut state = SpectrumState {
            style: baseline.clone(),
            ..Default::default()
        };
        state.apply_snapshot(&snapshot);
        let original = state.points.clone();
        let peak = |s: &SpectrumState| {
            s.peak
                .as_ref()
                .map(|p| (p.content.clone(), p.marker_pos, p.label_pos))
        };
        let changes: [fn(&mut SpectrumSettings); 8] = [
            |s| s.reverse_frequency = true,
            |s| s.rotation = 1,
            |s| s.rotation = -1,
            |s| s.frequency_scale = FrequencyScale::Linear,
            |s| s.frequency_scale = FrequencyScale::Erb,
            |s| s.weighting_mode = SpectrumWeightingMode::Raw,
            |s| s.secondary_weighting_mode = SpectrumWeightingMode::Raw,
            |s| s.show_peak_label = false,
        ];
        for change in changes {
            let mut settings = baseline.clone();
            change(&mut settings);
            let revision = state.geometry.revision;
            state.update_view_settings(&settings, &snapshot);
            let mut expected = SpectrumState {
                style: settings,
                ..Default::default()
            };
            expected.apply_snapshot(&snapshot);
            assert_ne!(state.geometry.revision, revision);
            assert_eq!(state.points, expected.points);
            assert_eq!(peak(&state), peak(&expected));
            state.update_view_settings(&baseline, &snapshot);
            assert_eq!(state.points, original);
            assert!(state.peak.is_some());
        }
    }

    #[test]
    fn view_changes_reach_cached_spectrum_without_new_audio() {
        use crate::dsp::{AudioFormat, ChannelPosition};
        use crate::persistence::settings::VisualConfig;
        use crate::visuals::registry::{VisualContent, VisualKind, VisualManager};

        let mut settings = SpectrumSettings {
            show_peak_label: true,
            ..Default::default()
        };
        let original_floor = settings.floor_db;
        let mut manager = VisualManager::default();
        manager.apply_config(VisualConfig::Spectrum(settings.clone()));
        manager.set_enabled(VisualKind::Spectrum, true);
        let format = AudioFormat::new(1, 48_000, 1, ChannelPosition::fallback(1));
        manager.ingest_samples(
            &crate::util::audio::sine_wave(1_000.0, 48_000.0, settings.fft_size, 0.5),
            format,
        );
        let state = manager
            .snapshot()
            .into_iter()
            .find_map(|slot| match slot.content {
                VisualContent::Spectrum(state) => Some(state),
                _ => None,
            })
            .unwrap();
        let original = state.borrow().points.clone();
        let peak_content = state.borrow().peak.as_ref().unwrap().content.clone();
        assert!(original[0].len() > 2 && original[0].iter().any(|point| point[1] > 0.5));
        assert!(original[0].windows(2).all(|pair| pair[0][0] < pair[1][0]));
        for (floor, peak_visible) in [(-60.0, true), (-3.0, false), (-140.0, true), (-120.0, true)]
        {
            for (reverse, rotation) in [(false, 0), (true, 1), (false, 2), (true, -1)] {
                settings.floor_db = floor;
                settings.reverse_frequency = reverse;
                settings.rotation = rotation;
                manager.apply_config(VisualConfig::Spectrum(settings.clone()));
                let state = state.borrow();
                for (before, after) in original.iter().zip(&state.points) {
                    let mut expected: Vec<_> = before
                        .iter()
                        .map(|&[x, y]| {
                            let db = original_floor + y * -original_floor;
                            let y = if y == 0.0 {
                                0.0
                            } else {
                                ((db - floor) / -floor).clamp(0.0, 1.0)
                            };
                            [if reverse { 1.0 - x } else { x }, y]
                        })
                        .collect();
                    if reverse {
                        expected.reverse();
                    }
                    assert_eq!(after.len(), expected.len());
                    for (actual, expected) in after.iter().zip(expected) {
                        assert_eq!(actual[0], expected[0]);
                        assert!((actual[1] - expected[1]).abs() < 1.0e-6, "floor={floor}");
                    }
                }
                assert_eq!(
                    state.peak.as_ref().map(|peak| &peak.content),
                    peak_visible.then_some(&peak_content),
                );
            }
        }

        manager.ingest_samples(&vec![0.0; settings.fft_size], format);
        settings.floor_db = -140.0;
        manager.apply_config(VisualConfig::Spectrum(settings));
        let state = state.borrow();
        assert!(
            state
                .points
                .iter()
                .all(|points| points.iter().all(|point| point[1] == 0.0))
        );
        assert!(state.peak.is_none());
    }

    #[test]
    fn theme_colors_invalidate_cached_geometry_without_audio_update() {
        let mut state = SpectrumState::default();
        state.points[0] = Arc::new(vec![[0.0, 0.0], [1.0, 1.0]]);
        let bounds = Rectangle::new(Point::ORIGIN, Size::new(100.0, 50.0));
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
    fn grid_labels_stay_in_screen_order_across_projections() {
        let range = (DISPLAY_MIN_HZ, 24_000.0);
        let majors = |state: &SpectrumState| -> Vec<usize> {
            let ticks = &state.grid_ticks;
            let labels = state.grid_labels.iter().map(|label| label.tick_index);
            labels.filter(|&index| ticks[index].1).collect()
        };
        let mut state = SpectrumState::default();
        let mut majors_only = SpectrumState::default();
        for scale in [
            FrequencyScale::Logarithmic,
            FrequencyScale::Linear,
            FrequencyScale::Erb,
        ] {
            for reverse in [false, true] {
                for state in [&mut state, &mut majors_only] {
                    state.style.frequency_scale = scale;
                    state.style.reverse_frequency = reverse;
                    state.ensure_x_cache(range.0, range.1, &[0.0, range.1]);
                }
                for tick in majors_only.grid_ticks.iter_mut().filter(|tick| !tick.1) {
                    tick.2 = None;
                }
                assert_eq!(state.grid_ticks.first().map(|tick| tick.0), Some(10.0));
                for width in [150.0, 600.0] {
                    let bounds = Rectangle::new(Point::ORIGIN, Size::new(width, 100.0));
                    state.layout_grid_labels(bounds, range);
                    majors_only.layout_grid_labels(bounds, range);
                    let context = format!("scale={scale:?}, reverse={reverse}, width={width}");
                    assert!(state.grid_labels.len() >= if width > 300.0 { 3 } else { 1 });
                    assert!(
                        state.grid_labels.windows(2).all(|pair| {
                            pair[0].bounds.x + pair[0].bounds.width <= pair[1].bounds.x
                        }),
                        "{context}"
                    );
                    assert_eq!(majors(&state), majors(&majors_only), "{context}");

                    // Turned labels stay upright, inside the pane, and in order.
                    let pane = Rectangle::new(Point::ORIGIN, Size::new(100.0, width));
                    state.style.rotation = 1;
                    state.layout_grid_labels(pane, range);
                    state.style.rotation = 0;
                    assert!(!state.grid_labels.is_empty(), "{context}");
                    for label in &state.grid_labels {
                        let text = state.grid_ticks[label.tick_index].2.as_ref().unwrap();
                        let text_width = text.min_bounds().width + GRID_LABEL_PADDING_X * 2.0;
                        assert_eq!(label.bounds.width, text_width, "{context}");
                        assert!(label.bounds.is_within(&pane), "{context}");
                    }
                    assert!(
                        state.grid_labels.windows(2).all(|pair| {
                            pair[0].bounds.y + pair[0].bounds.height <= pair[1].bounds.y
                        }),
                        "{context}"
                    );
                }
            }
        }
    }

    #[test]
    fn grid_layout_and_cutout_geometry_are_cached() {
        let mut state = SpectrumState::default();
        state.ensure_x_cache(DISPLAY_MIN_HZ, 24_000.0, &[0.0, 24_000.0]);
        let bounds = Rectangle::new(Point::ORIGIN, Size::new(600.0, 100.0));
        let range = (DISPLAY_MIN_HZ, 24_000.0);
        state.layout_grid_labels(bounds, range);
        let revision = state.grid_layout_revision;
        let cutouts = Arc::clone(&state.grid_cutouts);

        state.layout_grid_labels(bounds, range);

        assert_eq!(state.grid_layout_revision, revision);
        assert!(Arc::ptr_eq(&state.grid_cutouts, &cutouts));
        assert!(
            cutouts
                .iter()
                .copied()
                .eq(state.grid_labels.iter().map(|label| label.bounds))
        );

        state.layout_grid_labels(bounds.expand(1.0), range);
        assert_ne!(state.grid_layout_revision, revision);
        assert!(!Arc::ptr_eq(&state.grid_cutouts, &cutouts));

        let revision = state.grid_layout_revision;
        state.style.rotation = 1;
        state.layout_grid_labels(bounds.expand(1.0), range);
        assert_ne!(state.grid_layout_revision, revision);
    }

    #[test]
    fn turned_peak_label_stays_upright_inside_pane() {
        let peak = PeakLabel {
            text: ["A4", "440 Hz"].map(|text| peak_text(text, 0)),
            content: ["A4".into(), "440 Hz".into()],
            label_pos: [0.3, 0.5],
            marker_pos: [0.3, 0.5],
            opacity: 1.0,
        };
        let bounds = Rectangle::new(Point::new(5.0, 7.0), Size::new(400.0, 300.0));
        let upright = peak_label_layout(bounds, QuarterTurn::default(), &peak).unwrap();
        for rotation in [1, 2, -1] {
            let layout = peak_label_layout(bounds, QuarterTurn::new(rotation), &peak).unwrap();
            assert_eq!(
                layout.rect.size(),
                upright.rect.size(),
                "rotation={rotation}"
            );
            assert!(layout.rect.is_within(&bounds), "rotation={rotation}");
        }
    }

    #[test]
    fn point_build_emits_only_finite_coordinates() {
        let mut points = Vec::new();
        build_single_points_into(
            &mut points,
            &SpectrumSettings::default(),
            20.0,
            40.0,
            &SpectrumSnapshot {
                floor_db: -120.0,
                frequency_bins: vec![0.0, 20.0, 30.0, 40.0],
                ..Default::default()
            },
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
    snap: &SpectrumSnapshot,
    db: &[f32],
    x_cache: &[f32],
) {
    let bins = &snap.frequency_bins;
    let inv_dr = (MAX_DB - style.floor_db).max(EPSILON).recip();
    let y = |m: f32| {
        if m <= snap.floor_db {
            0.0
        } else {
            ((m - style.floor_db) * inv_dr).clamp(0.0, 1.0)
        }
    };
    let mut push = |x, m| {
        let y = y(m);
        if y.is_finite() {
            out.push([if style.reverse_frequency { 1.0 - x } else { x }, y]);
        }
    };

    let interior = bins.partition_point(|&f| f <= min_f)..bins.partition_point(|&f| f < max_f);
    push(x_cache[0], value_at(bins, db, min_f));
    for (&x, &m) in x_cache[1..x_cache.len() - 1].iter().zip(&db[interior]) {
        push(x, m);
    }
    push(x_cache[x_cache.len() - 1], value_at(bins, db, max_f));
    if style.reverse_frequency {
        out.reverse();
    }
}

// Normalized display x of a frequency, honoring scale and direction.
fn view_x(style: &SpectrumSettings, (min_f, max_f): (f32, f32), frequency: f32) -> f32 {
    let t = style
        .frequency_scale
        .pos_of(min_f, max_f, frequency)
        .clamp(0.0, 1.0);
    if style.reverse_frequency { 1.0 - t } else { t }
}

// Screen x of ticks within `range`, matching `view_x`; scales the range once per frame.
fn grid_tick_x(
    bounds: Rectangle,
    range: (f32, f32),
    style: &SpectrumSettings,
) -> impl Fn(f32) -> Option<f32> {
    let scale = style.frequency_scale;
    let lo = scale.scale(range.0);
    let span = (scale.scale(range.1) - lo).max(EPSILON);
    let reverse = style.reverse_frequency;
    move |frequency| {
        if !(range.0..=range.1).contains(&frequency) {
            return None;
        }
        let t = ((scale.scale(frequency) - lo) / span).clamp(0.0, 1.0);
        let x = if reverse { 1.0 - t } else { t };
        x.is_finite().then_some(bounds.x + bounds.width * x)
    }
}

fn draw_grid_lines(
    r: &mut iced::Renderer,
    th: &iced::Theme,
    bounds: Rectangle,
    range: (f32, f32),
    state: &SpectrumState,
) {
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return;
    }
    let text_color = th.extended_palette().background.base.text;
    let major_color = with_alpha(text_color, 0.25);
    let minor_color = with_alpha(text_color, 0.10);
    let turn = state.turn();
    let frame = turn.frame(bounds);
    let tick_x = grid_tick_x(frame, range, &state.style);

    for (frequency, major, _) in &state.grid_ticks {
        let Some(x) = tick_x(*frequency) else {
            continue;
        };
        let x = (x - 0.5).clamp(frame.x, (frame.x + frame.width - 1.0).max(frame.x));
        fill_rect(
            r,
            turn.to_screen(
                bounds,
                Rectangle::new(Point::new(x, frame.y), Size::new(1.0, frame.height)),
            ),
            if *major { major_color } else { minor_color },
        );
    }
}

fn draw_grid_labels(r: &mut iced::Renderer, th: &iced::Theme, state: &SpectrumState) {
    let text_color = th.extended_palette().background.base.text;
    let major_color = with_alpha(text_color, 0.75);
    let minor_color = with_alpha(text_color, 0.20);
    for layout in &state.grid_labels {
        let (_, major, Some(text)) = &state.grid_ticks[layout.tick_index] else {
            continue;
        };
        r.fill_paragraph(
            text,
            Point::new(
                layout.bounds.x + GRID_LABEL_PADDING_X,
                layout.bounds.y + GRID_LABEL_PADDING_Y,
            ),
            if *major { major_color } else { minor_color },
            layout.bounds,
        );
    }
}

#[derive(Clone, Copy)]
struct PeakLayout {
    rect: Rectangle,
    text_layouts: [(Point, Size); 2],
    leader_anchor: [f32; 2],
}

fn point_to_normalized(b: Rectangle, p: Point) -> [f32; 2] {
    [(p.x - b.x) / b.width, 1.0 - (p.y - b.y) / b.height]
}

// Places the label in the unturned frame, then turns it upright into `bounds`.
fn peak_label_layout(bounds: Rectangle, turn: QuarterTurn, peak: &PeakLabel) -> Option<PeakLayout> {
    const MIN_VIEW_SIZE: f32 = 8.0;
    const LABEL_GAP: f32 = 8.0;
    const LINE_GAP: f32 = 2.0;
    let b = turn.frame(bounds);
    if peak.opacity < MIN_PEAK_OPACITY || b.width < MIN_VIEW_SIZE || b.height < MIN_VIEW_SIZE {
        return None;
    }
    let [title, detail] = peak.text.each_ref().map(|text| text.min_bounds());
    let [px, py] = peak.label_pos;
    let peak_pos = Point::new(b.x + b.width * px, b.y + b.height * (1.0 - py));
    let padding = Padding::new(7.0).top(6.0).bottom(5.0);
    let size = turn.frame_size(Size::new(
        title.width.max(detail.width) + padding.x(),
        title.height + detail.height + padding.y() + LINE_GAP,
    ));
    let fits_right = peak_pos.x + size.width + LABEL_GAP <= b.x + b.width;
    let x = if fits_right {
        peak_pos.x + LABEL_GAP
    } else {
        peak_pos.x - size.width - LABEL_GAP
    }
    .clamp(b.x, (b.x + b.width - size.width).max(b.x));
    let y =
        (peak_pos.y - size.height - LABEL_GAP).clamp(b.y, (b.y + b.height - size.height).max(b.y));
    let rect = turn.to_screen(bounds, Rectangle::new(Point::new(x, y), size));
    let title_pos = Point::new(rect.x + padding.left, rect.y + padding.top);
    let detail_pos = Point::new(title_pos.x, title_pos.y + title.height + LINE_GAP);
    let anchor = Point::new(if fits_right { x } else { x + size.width }, y + size.height);
    Some(PeakLayout {
        rect,
        text_layouts: [(title_pos, title), (detail_pos, detail)],
        leader_anchor: point_to_normalized(b, anchor),
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
