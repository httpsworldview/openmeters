// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::processor::{ChromaSnapshot, NOTE_NAMES, NUM_PITCH_CLASSES};
use super::render::{ChromaParams, ChromaPrimitive, LABEL_AREA_HEIGHT, VERTICAL_PADDING};
use crate::util::color::color_to_rgba;
use crate::visuals::palettes;
use crate::visuals::render::common::{fill_rect, make_text};
use iced::advanced::text;
use iced::alignment::{Horizontal, Vertical};
use iced::{Color, Point, Rectangle, Size};

pub(crate) const CHROMA_PALETTE_SIZE: usize = palettes::chroma::COLORS.len();

const LABEL_FONT_SIZE: f32 = 10.0;

#[derive(Debug, Clone)]
pub(crate) struct ChromaState {
    snapshot: ChromaSnapshot,
    pub(crate) palette: [Color; CHROMA_PALETTE_SIZE],
    pub(crate) show_peak_hold: bool,
    key: u64,
}

impl ChromaState {
    pub fn new() -> Self {
        Self {
            snapshot: ChromaSnapshot::default(),
            palette: palettes::chroma::COLORS,
            show_peak_hold: true,
            key: crate::visuals::next_key(),
        }
    }

    pub fn apply_snapshot(&mut self, snapshot: ChromaSnapshot) {
        self.snapshot = snapshot;
    }

    pub fn set_palette(&mut self, palette: &[Color; CHROMA_PALETTE_SIZE]) {
        self.palette = *palette;
    }

    pub(crate) fn visual_params(&self, bounds: Rectangle) -> Option<ChromaParams> {
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return None;
        }

        let note_colors: [[f32; 4]; NUM_PITCH_CLASSES] =
            std::array::from_fn(|i| color_to_rgba(self.palette[i]));
        let peak_color = color_to_rgba(self.palette[CHROMA_PALETTE_SIZE - 1]);

        Some(ChromaParams {
            bounds,
            bins: self.snapshot.bins,
            peak_bins: if self.show_peak_hold {
                Some(self.snapshot.peak_bins)
            } else {
                None
            },
            note_colors,
            peak_color,
            note_names: NOTE_NAMES,
            key: self.key,
        })
    }
}

crate::visuals::visualization_widget!(
    Chroma, ChromaState,
    |this, renderer, theme, bounds| {
        let state = this.state.borrow();
        let bg = theme.extended_palette().background.base.color;

        let Some(params) = state.visual_params(bounds) else {
            fill_rect(renderer, bounds, bg);
            return;
        };

        renderer.draw_primitive(bounds, ChromaPrimitive::new(params.clone()));

        // note names along the bottom
        let usable_h = (bounds.height - VERTICAL_PADDING * 2.0 - LABEL_AREA_HEIGHT).max(0.0);
        let bar_slot = bounds.width / NUM_PITCH_CLASSES as f32;
        let label_y = bounds.y + VERTICAL_PADDING + usable_h;

        for (i, &name) in params.note_names.iter().enumerate() {
            let cx = bounds.x + (i as f32 + 0.5) * bar_slot;
            let color = state.palette[i];
            let mut t = make_text(name, LABEL_FONT_SIZE, Size::new(bar_slot, LABEL_AREA_HEIGHT));
            t.align_x = Horizontal::Center.into();
            t.align_y = Vertical::Center;
            text::Renderer::fill_text(
                renderer,
                t,
                Point::new(cx, label_y + LABEL_AREA_HEIGHT * 0.5),
                color,
                bounds,
            );
        }
    }
);
