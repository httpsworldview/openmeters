// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{
    MAX_COLUMN_CAPACITY, NUM_BANDS, WAVEFORM_CHANNELS, WaveFrame, WaveformPreview,
    WaveformUpdate,
};
use super::render::{WaveformParams, WaveformPrimitive};
use crate::persistence::settings::WaveformSettings;
use crate::util::color::color_to_rgba;
use crate::visuals::palettes;
use iced::Color;
use std::{cell::Cell, collections::VecDeque, sync::Arc};

const COLUMN_WIDTH_PIXELS: f32 = 1.0;
const INITIAL_VIEW_COLUMNS: usize = 512;

#[derive(Debug)]
pub(in crate::visuals) struct WaveformState {
    data: Arc<VecDeque<WaveFrame>>,
    preview: WaveformPreview,
    view_columns: Cell<usize>,
    pub(in crate::visuals) style: WaveformStyle,
    settings: WaveformSettings,
    key: u64,
}

impl WaveformState {
    pub fn new() -> Self {
        Self {
            data: Arc::new(VecDeque::with_capacity(INITIAL_VIEW_COLUMNS)),
            preview: WaveformPreview::default(),
            view_columns: Cell::new(INITIAL_VIEW_COLUMNS),
            style: WaveformStyle::default(),
            settings: WaveformSettings::default(),
            key: crate::visuals::next_key(),
        }
    }

    pub fn apply_snapshot(&mut self, update: WaveformUpdate<'_>) {
        self.preview = update.preview;
        if !update.reset && update.columns.is_empty() {
            return;
        }
        let max_columns = self.view_columns.get().clamp(1, MAX_COLUMN_CAPACITY);
        let data = Arc::make_mut(&mut self.data);
        Self::configure_ring(data, max_columns, update.reset);
        for &columns in update.columns {
            Self::push_column(data, columns, max_columns);
        }
    }

    pub(in crate::visuals) fn view_columns(&self) -> usize {
        self.view_columns.get()
    }

    pub fn update_view_settings(&mut self, settings: &WaveformSettings) {
        self.settings = settings.clone();
    }

    pub fn export_settings(&self) -> WaveformSettings {
        self.settings.clone()
    }

    pub fn set_palette(&mut self, palette: &[Color; NUM_BANDS]) {
        self.style.palette = *palette;
    }

    pub fn visual_params(&self, bounds: iced::Rectangle) -> Option<WaveformParams> {
        let needed = ((bounds.width / COLUMN_WIDTH_PIXELS).ceil() as usize)
            .clamp(1, MAX_COLUMN_CAPACITY);
        if bounds.width > 0.0 {
            self.view_columns.set(needed);
        }

        let total_columns = self.data.len();
        let (lanes, selected_channels) = self.selected_lanes();
        if bounds.width <= 0.0
            || selected_channels == 0
            || (total_columns == 0 && self.preview.columns.is_none())
        {
            return None;
        }

        let lanes = &lanes[..selected_channels];

        Some(WaveformParams {
            bounds,
            lanes: [lanes[0], lanes.get(1).copied().unwrap_or(0)],
            channels: selected_channels,
            column_width: COLUMN_WIDTH_PIXELS,
            columns: needed,
            data: Arc::clone(&self.data),
            preview: self.preview,
            color_mode: self.settings.color_mode,
            history_mode: self.settings.history_mode,
            band_db_floor: self.settings.band_db_floor,
            palette: self.style.palette.map(color_to_rgba),
            fill_alpha: self.style.fill_alpha,
            vertical_padding: self.style.vertical_padding,
            channel_gap: self.style.channel_gap,
            amplitude_scale: self.style.amplitude_scale,
            key: self.key,
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

    fn push_column(data: &mut VecDeque<WaveFrame>, columns: WaveFrame, max_columns: usize) {
        if data.len() == max_columns {
            data.pop_front();
        }
        data.push_back(columns);
    }

    fn selected_lanes(&self) -> ([usize; 2], usize) {
        let mut lanes = [0; 2];
        let mut len = 0;
        for lane in [self.settings.channel_1, self.settings.channel_2]
            .into_iter()
            .filter_map(|channel| WAVEFORM_CHANNELS.iter().position(|&source| source == channel))
        {
            lanes[len] = lane;
            len += 1;
        }
        (lanes, len)
    }
}

crate::macros::default_struct! {
    #[derive(Debug)]
    pub(in crate::visuals) struct WaveformStyle {
        pub fill_alpha: f32 = 1.0,
        pub vertical_padding: f32 = 8.0,
        pub channel_gap: f32 = 12.0,
        pub amplitude_scale: f32 = 1.0,
        pub(in crate::visuals) palette: [Color; NUM_BANDS] = palettes::waveform::COLORS,
    }
}

crate::visuals::visualization_widget!(Waveform, WaveformState, WaveformPrimitive);
