// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::domain::visuals::VisualKind;
use crate::util::color::palettes_equal;
use iced::Color;

pub const BG_BASE: Color = Color::BLACK;

const HEAT_RAMP: [Color; 5] = [
    Color::TRANSPARENT,
    Color::from_rgb8(0x38, 0x00, 0xAD),
    Color::from_rgb8(0xFF, 0x00, 0x00),
    Color::from_rgb8(0xFF, 0xFF, 0x21),
    Color::from_rgb8(0xFF, 0xFF, 0xFF),
];

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

    pub fn colors(&self) -> &[Color] {
        if self.colors.is_empty() {
            self.defaults
        } else {
            &self.colors
        }
    }

    pub fn labels(&self) -> &'static [&'static str] {
        self.labels
    }

    pub fn len(&self) -> usize {
        self.defaults.len()
    }

    pub fn set_colors(&mut self, colors: &[Color]) {
        self.colors.clear();
        if colors.len() == self.defaults.len() && !palettes_equal(colors, self.defaults) {
            self.colors.extend_from_slice(colors);
        }
    }

    pub fn reset(&mut self) {
        self.colors.clear();
    }

    pub fn is_default(&self) -> bool {
        palettes_equal(self.colors(), self.defaults)
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

pub mod spectrogram {
    use super::{Color, HEAT_RAMP};
    pub const COLORS: [Color; 5] = HEAT_RAMP;
    pub const LABELS: &[&str] = &["Quietest", "->", "->", "->", "Loud"];

    pub const DEFAULT_POSITIONS: [f32; COLORS.len()] =
        [0.0, 0.402_523_83, 0.679_189_3, 0.869_322_26, 1.0];
}

pub mod spectrum {
    use super::{Color, HEAT_RAMP};
    pub const COLORS: [Color; 6] = {
        let [floor, low, low_mid, mid, high] = HEAT_RAMP;
        [floor, low, low_mid, mid, high, high]
    };
    pub const LABELS: &[&str] = &["Floor", "Low", "Low-Mid", "Mid", "High", "Peak"];
    pub const DEFAULT_POSITIONS: [f32; COLORS.len()] = [0.0, 0.2, 0.4, 0.6, 0.8, 1.0];
}

pub mod waveform {
    use super::Color;
    pub const COLORS: [Color; 3] = [
        Color::from_rgb8(0xFF, 0x00, 0x00),
        Color::from_rgb8(0x00, 0xFF, 0x00),
        Color::from_rgb8(0x00, 0x00, 0xFF),
    ];
    pub const LABELS: &[&str] = &["Low", "Mid", "High"];
    pub const DEFAULT_POSITIONS: [f32; COLORS.len()] = [0.0, 0.5, 1.0];
}

pub mod oscilloscope {
    use super::Color;
    pub const COLORS: [Color; 2] = [
        Color::from_rgb8(0xFF, 0xFF, 0xFF),
        Color::from_rgb8(0xFF, 0xFF, 0xFF),
    ];
    pub const LABELS: &[&str] = &["Channel 1", "Channel 2"];
    pub const DEFAULT_POSITIONS: [f32; COLORS.len()] = [0.0, 1.0];
}

pub mod stereometer {
    use super::Color;
    pub const COLORS: [Color; 9] = [
        Color::from_rgb8(0xFF, 0xFF, 0xFF),
        Color::from_rgb8(0x1A, 0x1A, 0x1A),
        Color::from_rgb8(0x80, 0x80, 0x80),
        Color::from_rgb8(0x73, 0xA6, 0x80),
        Color::from_rgb8(0xB3, 0x59, 0x59),
        Color::from_rgb8(0xFF, 0x00, 0x00),
        Color::from_rgb8(0x00, 0xFF, 0x00),
        Color::from_rgb8(0x00, 0x00, 0xFF),
        Color::from_rgba8(0x80, 0x80, 0x80, 64.0 / 255.0),
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

pub mod loudness {
    use super::Color;
    pub const COLORS: [Color; 7] = [
        Color::from_rgb8(0x29, 0x29, 0x29),
        Color::from_rgb8(0xA0, 0xAA, 0xAD),
        Color::from_rgb8(0xAB, 0xCF, 0xAD),
        Color::from_rgb8(0xFF, 0xB7, 0x54),
        Color::from_rgb8(0xFF, 0x5C, 0x4F),
        Color::from_rgb8(0xF5, 0xED, 0xC4),
        Color::from_rgba8(0xB7, 0xC2, 0xC9, 224.0 / 255.0),
    ];
    pub const LABELS: &[&str] = &[
        "Background",
        "Low",
        "Mid",
        "High",
        "Danger",
        "Peak",
        "Guide",
    ];
    pub const DEFAULT_POSITIONS: [f32; COLORS.len()] = [0.0, 0.16, 0.32, 0.48, 0.64, 0.80, 1.0];
}
pub mod background {
    use super::{BG_BASE, Color};
    pub const COLORS: [Color; 1] = [BG_BASE];
    pub const LABELS: &[&str] = &["Background"];
    pub const DEFAULT_POSITIONS: [f32; COLORS.len()] = [0.0];
}
