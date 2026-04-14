// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::domain::visuals::VisualKind;
use crate::util::color::{hex, palettes_equal};
use iced::Color;

pub const BG_BASE: Color = hex(0x00, 0x00, 0x00, 0xFF);

#[derive(Debug, Clone)]
pub struct Palette {
    colors: Vec<Color>,
    pub defaults: &'static [Color],
    pub default_positions: &'static [f32],
    labels: &'static [&'static str],
}

impl Palette {
    pub const fn new(
        defaults: &'static [Color],
        default_positions: &'static [f32],
        labels: &'static [&'static str],
    ) -> Self {
        Self {
            colors: Vec::new(),
            defaults,
            default_positions,
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

    pub const fn for_kind(kind: VisualKind) -> Self {
        macro_rules! p {
            ($m:ident) => {
                Self::new(&$m::COLORS, &$m::DEFAULT_POSITIONS, $m::LABELS)
            };
        }
        match kind {
            VisualKind::Spectrogram => p!(spectrogram),
            VisualKind::Spectrum => p!(spectrum),
            VisualKind::Waveform => p!(waveform),
            VisualKind::Oscilloscope => p!(oscilloscope),
            VisualKind::Stereometer => p!(stereometer),
            VisualKind::Loudness => p!(loudness),
        }
    }
}

// Spectrogram heat map: quiet -> loud (5 stops)
pub mod spectrogram {
    use super::*;
    pub const COLORS: [Color; 5] = [
        hex(0x00, 0x00, 0x00, 0x00),
        hex(0x38, 0x00, 0xAD, 0xFF),
        hex(0xFF, 0x00, 0x00, 0xFF),
        hex(0xFF, 0xFF, 0x21, 0xFF),
        hex(0xFF, 0xFF, 0xFF, 0xFF),
    ];
    pub const LABELS: &[&str] = &["Quietest", "->", "->", "->", "Loud"];

    // Exponential pos(t) = (1-e^-1.5t)/(1-e^-1.5): peak colors only for the loudest signals.
    pub const DEFAULT_POSITIONS: [f32; COLORS.len()] =
        [0.0, 0.402_523_83, 0.679_189_3, 0.869_322_26, 1.0];
}

// Spectrum analyzer gradient: quiet -> loud (6 stops)
pub mod spectrum {
    use super::*;
    pub const COLORS: [Color; 6] = [
        hex(0x00, 0x00, 0x00, 0x00),
        hex(0x38, 0x1B, 0x55, 0xFF),
        hex(0x9B, 0x00, 0x00, 0xFF),
        hex(0xE7, 0x7C, 0x00, 0xFF),
        hex(0xFF, 0xBC, 0x5A, 0xFF),
        hex(0xFF, 0xFF, 0x00, 0xFF),
    ];
    pub const LABELS: &[&str] = &["Floor", "Low", "Low-Mid", "Mid", "High", "Peak"];
    pub const DEFAULT_POSITIONS: [f32; COLORS.len()] = [0.0, 0.2, 0.4, 0.6, 0.8, 1.0];
}

// dark red (low) -> orange -> green -> cyan -> blue (high)
pub mod waveform {
    use super::*;
    pub const GRADIENT_STOPS: usize = 6;
    pub const COLORS: [Color; 9] = [
        hex(0x8B, 0x00, 0x00, 0xFF),
        hex(0xFF, 0x42, 0x00, 0xFF),
        hex(0xFF, 0x69, 0x00, 0xFF),
        hex(0x4C, 0xFF, 0x2E, 0xFF),
        hex(0x32, 0xCD, 0xFF, 0xFF),
        hex(0x00, 0x00, 0xFF, 0xFF),
        hex(0xE0, 0x40, 0xA0, 0xD9),
        hex(0x33, 0xE6, 0x33, 0xD9),
        hex(0x33, 0x66, 0xFF, 0xD9),
    ];
    pub const LABELS: &[&str] = &[
        "Sub-bass",
        "->",
        "->",
        "->",
        "->",
        "Brilliance",
        "Low Band",
        "Mid Band",
        "High Band",
    ];
    pub const DEFAULT_POSITIONS: [f32; COLORS.len()] =
        [0.0, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0];
}

// Oscilloscope trace color (1 stop)
pub mod oscilloscope {
    use super::*;
    pub const COLORS: [Color; 1] = [hex(0xFF, 0xFF, 0xFF, 0xFF)];
    pub const LABELS: &[&str] = &["Trace"];
    pub const DEFAULT_POSITIONS: [f32; COLORS.len()] = [0.0];
}

// Stereometer (9 stops)
pub mod stereometer {
    use super::*;
    pub const COLORS: [Color; 9] = [
        hex(0xFF, 0xFF, 0xFF, 0xFF),
        hex(0x1A, 0x1A, 0x1A, 0xFF),
        hex(0x80, 0x80, 0x80, 0xFF),
        hex(0x73, 0xA6, 0x80, 0xFF),
        hex(0xB3, 0x59, 0x59, 0xFF),
        hex(0xFF, 0x00, 0x00, 0xFF),
        hex(0x00, 0xFF, 0x00, 0xFF),
        hex(0x00, 0x00, 0xFF, 0xFF),
        hex(0x80, 0x80, 0x80, 0x40),
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
    pub const DEFAULT_POSITIONS: [f32; COLORS.len()] =
        [0.0, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0];
}

// Loudness meter: background, left_ch_1, left_ch_2, right_fill, guide_line (5 stops)
pub mod loudness {
    use super::*;
    pub const COLORS: [Color; 5] = [
        hex(0x29, 0x29, 0x29, 0xFF),
        hex(0xA0, 0xAA, 0xAD, 0xFF),
        hex(0x95, 0x9E, 0xA6, 0xFF),
        hex(0xB3, 0xC4, 0xBC, 0xFF),
        hex(0xBB, 0xBF, 0xC5, 0xE0),
    ];
    pub const LABELS: &[&str] = &["Background", "Left 1", "Left 2", "Right", "Guide"];
    pub const DEFAULT_POSITIONS: [f32; COLORS.len()] = [0.0, 0.25, 0.5, 0.75, 1.0];
}

// App background color (1 stop)
pub mod background {
    use super::*;
    pub const COLORS: [Color; 1] = [BG_BASE];
    pub const LABELS: &[&str] = &["Background"];
    pub const DEFAULT_POSITIONS: [f32; COLORS.len()] = [0.0];
}
