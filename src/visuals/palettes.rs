// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

// Default palette constants for each visual type.
//
// Extracted from ui/theme.rs so that visuals and persistence can reference
// defaults without crossing into the UI layer.

use crate::util::color::palettes_equal;
use iced::Color;

pub const BG_BASE: Color = Color::from_rgba(0.065, 0.065, 0.065, 1.0);

#[derive(Debug, Clone, Default)]
pub struct Palette {
    colors: Vec<Color>,
    defaults: &'static [Color],
    labels: &'static [&'static str],
}

impl Palette {
    pub const fn new(defaults: &'static [Color], labels: &'static [&'static str]) -> Self {
        Self {
            colors: Vec::new(),
            defaults,
            labels,
        }
    }

    #[inline]
    pub fn colors(&self) -> &[Color] {
        if self.colors.is_empty() {
            self.defaults
        } else {
            &self.colors
        }
    }

    #[inline]
    pub fn labels(&self) -> &'static [&'static str] {
        self.labels
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.defaults.len()
    }

    // Sets colors, only stores if different from defaults.
    pub fn set(&mut self, colors: &[Color]) {
        self.colors.clear();
        if colors.len() == self.defaults.len() && !palettes_equal(colors, self.defaults) {
            self.colors.extend_from_slice(colors);
        }
    }

    pub fn reset(&mut self) {
        self.colors.clear();
    }

    #[inline]
    pub fn is_default(&self) -> bool {
        self.colors.is_empty() || palettes_equal(&self.colors, self.defaults)
    }
}

// Spectrogram heat map: quiet -> loud (5 stops)
pub mod spectrogram {
    use super::*;
    pub const COLORS: [Color; 5] = [
        Color::from_rgba(0.000, 0.000, 0.000, 0.0),
        Color::from_rgba(0.218, 0.106, 0.332, 1.0),
        Color::from_rgba(0.609, 0.000, 0.000, 1.0),
        Color::from_rgba(1.000, 0.737, 0.353, 1.0),
        Color::from_rgba(1.000, 1.000, 1.000, 1.0),
    ];
    pub const LABELS: &[&str] = &["Quietest", "->", "->", "->", "Loud"];
}

// Spectrum analyzer gradient: quiet -> loud (6 stops)
pub mod spectrum {
    use super::*;
    pub const COLORS: [Color; 6] = [
        Color::from_rgba(0.000, 0.000, 0.000, 0.0),
        Color::from_rgba(0.218, 0.106, 0.332, 1.0),
        Color::from_rgba(0.609, 0.000, 0.000, 1.0),
        Color::from_rgba(0.906, 0.485, 0.000, 1.0),
        Color::from_rgba(1.000, 0.737, 0.353, 1.0),
        Color::from_rgba(1.000, 1.000, 0.000, 1.0),
    ];
    pub const LABELS: &[&str] = &["Floor", "Low", "Low-Mid", "Mid", "High", "Peak"];
}

// dark red (low) -> orange -> green -> cyan -> blue (high)
pub mod waveform {
    use super::*;
    pub const COLORS: [Color; 6] = [
        Color::from_rgba(0.545, 0.000, 0.000, 1.0),
        Color::from_rgba(1.000, 0.259, 0.000, 1.0),
        Color::from_rgba(1.000, 0.412, 0.000, 1.0),
        Color::from_rgba(0.298, 1.000, 0.180, 1.0),
        Color::from_rgba(0.196, 0.804, 1.000, 1.0),
        Color::from_rgba(0.000, 0.000, 1.000, 1.0),
    ];
    pub const LABELS: &[&str] = &["Sub-bass", "->", "->", "->", "->", "Brilliance"];
}

// Oscilloscope trace color (1 stop)
pub mod oscilloscope {
    use super::*;
    pub const COLORS: [Color; 1] = [Color::from_rgba(1.000, 1.000, 1.000, 1.0)];
    pub const LABELS: &[&str] = &["Trace"];
}

// Stereometer (9 stops)
pub mod stereometer {
    use super::*;
    pub const COLORS: [Color; 9] = [
        Color::from_rgba(1.000, 1.000, 1.000, 1.0),
        Color::from_rgba(0.10, 0.10, 0.10, 1.0),
        Color::from_rgba(0.50, 0.50, 0.50, 1.0),
        Color::from_rgba(0.45, 0.65, 0.50, 1.0),
        Color::from_rgba(0.70, 0.35, 0.35, 1.0),
        Color::from_rgba(0.55, 0.45, 0.70, 1.0),
        Color::from_rgba(0.50, 0.60, 0.55, 1.0),
        Color::from_rgba(0.65, 0.55, 0.45, 1.0),
        Color::from_rgba(0.50, 0.50, 0.50, 0.25),
    ];
    pub const LABELS: &[&str] = &[
        "Trace",
        "Corr BG",
        "Corr Center",
        "Corr +",
        "Corr -",
        "Low",
        "Mid",
        "High",
        "Grid",
    ];
}

// Loudness meter: background, left_ch_1, left_ch_2, right_fill, guide_line (5 stops)
pub mod loudness {
    use super::*;
    pub const COLORS: [Color; 5] = [
        Color::from_rgba(0.161, 0.161, 0.161, 1.0),
        Color::from_rgba(0.626, 0.665, 0.680, 1.0),
        Color::from_rgba(0.584, 0.618, 0.650, 1.0),
        Color::from_rgba(0.701, 0.767, 0.735, 1.0),
        Color::from_rgba(0.735, 0.748, 0.774, 0.88),
    ];
    pub const LABELS: &[&str] = &["Background", "Left 1", "Left 2", "Right", "Guide"];
}

// App background color (1 stop)
pub mod background {
    use super::*;
    pub const COLORS: [Color; 1] = [BG_BASE];
    pub const LABELS: &[&str] = &["Background"];
}
