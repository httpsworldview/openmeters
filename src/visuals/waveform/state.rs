// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{NUM_BANDS, WAVEFORM_CHANNELS, WaveFrame, WaveformPreview, WaveformUpdate};
use super::render::{WaveformParams, column_count};
use crate::persistence::settings::WaveformSettings;
use crate::util::{color::color_to_rgba, unpoison};
use crate::visuals::palettes;
use crate::visuals::render::common::lane_bounds;
use iced::Color;
use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const INITIAL_VIEW_COLUMNS: usize = 512;
const SCROLL_CLOCK_TIMEOUT: Duration = Duration::from_millis(100);

pub(crate) struct WaveformState {
    data: Arc<Mutex<VecDeque<WaveFrame>>>,
    preview: WaveformPreview,
    scroll: Cell<(Instant, f32)>,
    snapshot_at: Cell<Instant>,
    view_columns: Cell<usize>,
    quiescent_columns: usize,
    pub(in crate::visuals) palette: [Color; NUM_BANDS],
    pub(in crate::visuals) settings: WaveformSettings,
    keys: [u64; 2],
}

impl Default for WaveformState {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            data: Arc::new(Mutex::new(VecDeque::with_capacity(INITIAL_VIEW_COLUMNS))),
            preview: WaveformPreview::default(),
            scroll: Cell::new((now, 0.0)),
            snapshot_at: Cell::new(now),
            view_columns: Cell::new(INITIAL_VIEW_COLUMNS),
            quiescent_columns: 0,
            palette: palettes::waveform::COLORS,
            settings: WaveformSettings::default(),
            keys: std::array::from_fn(|_| crate::visuals::next_key()),
        }
    }
}

impl WaveformState {
    pub fn reset_audio(&mut self) {
        unpoison(self.data.lock()).clear();
        self.preview = WaveformPreview::default();
        let now = Instant::now();
        self.scroll.set((now, 0.0));
        self.snapshot_at.set(now);
        self.quiescent_columns = 0;
    }

    pub fn apply_snapshot(&mut self, update: WaveformUpdate<'_>) {
        let now = Instant::now();
        if update.reset
            || now.saturating_duration_since(self.snapshot_at.get()) > SCROLL_CLOCK_TIMEOUT
        {
            self.scroll.set((now, update.preview.progress));
        } else {
            let (last, offset) = self.scroll.get();
            self.scroll
                .set((last, offset - update.columns.len() as f32));
        }
        self.snapshot_at.set(now);
        self.preview = update.preview;
        if !update.reset && update.columns.is_empty() {
            return;
        }
        let max_columns = self.view_columns.get();
        let mut data = unpoison(self.data.lock());
        Self::configure_ring(&mut data, max_columns, update.reset);
        if update.reset {
            self.quiescent_columns = 0;
        }
        for &columns in update.columns {
            self.quiescent_columns = if columns == WaveFrame::default() {
                self.quiescent_columns.saturating_add(1)
            } else {
                0
            };
            if data.len() == max_columns {
                data.pop_front();
            }
            data.push_back(columns);
        }
    }

    pub(in crate::visuals) fn is_quiescent(&self) -> bool {
        let needed = self.view_columns.get();
        self.quiescent_columns >= needed
            && unpoison(self.data.lock()).len() >= needed
            && self.preview.columns.unwrap_or_default() == WaveFrame::default()
    }

    pub(in crate::visuals) fn view_columns(&self) -> usize {
        self.view_columns.get()
    }

    pub fn update_view_settings(&mut self, settings: &WaveformSettings) {
        self.settings = settings.clone();
    }

    crate::visuals::palette_setter!(NUM_BANDS);

    pub fn visual_params(
        &self,
        bounds: iced::Rectangle,
    ) -> impl Iterator<Item = WaveformParams> + '_ {
        let now = Instant::now();
        let (last, offset) = self.scroll.get();
        let elapsed = now.saturating_duration_since(last);
        let scroll_offset = if elapsed <= SCROLL_CLOCK_TIMEOUT
            && now.saturating_duration_since(self.snapshot_at.get()) <= SCROLL_CLOCK_TIMEOUT
        {
            offset
                + elapsed.as_secs_f32()
                    * crate::util::finite_positive(self.settings.scroll_speed).unwrap_or(0.0)
        } else {
            self.preview.progress
        }
        .clamp(0.0, 1.0);
        self.scroll.set((now, scroll_offset));

        if bounds.width > 0.0 {
            self.view_columns.set(column_count(bounds.width));
        }

        let lanes = [self.settings.channel_1, self.settings.channel_2]
            .into_iter()
            .filter_map(|channel| {
                WAVEFORM_CHANNELS
                    .iter()
                    .position(|&source| source == channel)
            });
        let count = lanes.clone().count();
        lanes.enumerate().filter_map(move |(ch, lane)| {
            Some(WaveformParams {
                bounds: lane_bounds(bounds, count, ch)?,
                lane,
                data: Arc::clone(&self.data),
                preview: WaveformPreview {
                    progress: scroll_offset,
                    ..self.preview
                },
                color_mode: self.settings.color_mode,
                history_mode: self.settings.history_mode,
                band_db_floor: self.settings.band_db_floor,
                palette: self.palette.map(color_to_rgba),
                key: self.keys[ch],
            })
        })
    }

    fn configure_ring(data: &mut VecDeque<WaveFrame>, max_columns: usize, reset: bool) {
        if reset {
            data.clear();
        }
        data.drain(..data.len().saturating_sub(max_columns));
        if data.capacity() < max_columns {
            data.reserve(max_columns.saturating_sub(data.len()));
        } else if data.capacity() > max_columns.saturating_mul(2) {
            data.shrink_to(max_columns);
        }
    }
}

crate::visuals::visualization_widget!(Waveform, WaveformState);
