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

const fn evenly_spaced<const N: usize>() -> [f32; N] {
    let mut positions = [0.0; N];
    let mut i = 1;
    while i < N {
        positions[i] = i as f32 / (N - 1) as f32;
        i += 1;
    }
    positions
}

macro_rules! palette {
    (@define $name:ident { $($color:expr => $label:expr),+ } $positions:expr) => {
        pub mod $name {
            use super::Color;
            pub const LABELS: &[&str] = &[$($label),+];
            pub const COLORS: [Color; LABELS.len()] = [$($color),+];
            pub const SIZE: usize = COLORS.len();
            pub const DEFAULT_POSITIONS: [f32; SIZE] = $positions;
        }
    };
    ($name:ident { $($color:expr => $label:expr),+ $(,)? }) => {
        palette!(@define $name { $($color => $label),+ } super::evenly_spaced());
    };
    ($name:ident { $($color:expr => $label:expr),+ $(,)? } => $positions:expr) => {
        palette!(@define $name { $($color => $label),+ } $positions);
    };
}

palette!(spectrogram {
    super::HEAT_RAMP[0] => "Quietest",
    super::HEAT_RAMP[1] => "->",
    super::HEAT_RAMP[2] => "->",
    super::HEAT_RAMP[3] => "->",
    super::HEAT_RAMP[4] => "Loud",
} => [0.0, 0.402_523_83, 0.679_189_3, 0.869_322_26, 1.0]);

palette!(spectrum {
    super::HEAT_RAMP[0] => "Floor",
    super::HEAT_RAMP[1] => "Low",
    super::HEAT_RAMP[2] => "Low-Mid",
    super::HEAT_RAMP[3] => "Mid",
    super::HEAT_RAMP[4] => "High",
    super::HEAT_RAMP[4] => "Peak",
});

palette!(waveform {
    Color::from_rgb8(0xFF, 0x00, 0x00) => "Low",
    Color::from_rgb8(0x00, 0xFF, 0x00) => "Mid",
    Color::from_rgb8(0x00, 0x00, 0xFF) => "High",
});

palette!(oscilloscope {
    Color::from_rgb8(0xFF, 0xFF, 0xFF) => "Channel 1",
    Color::from_rgb8(0xFF, 0xFF, 0xFF) => "Channel 2",
});

palette!(stereometer {
    Color::from_rgb8(0xFF, 0xFF, 0xFF) => "Trace",
    Color::from_rgb8(0x1A, 0x1A, 0x1A) => "Corr BG",
    Color::from_rgb8(0x80, 0x80, 0x80) => "Corr Center",
    Color::from_rgb8(0x73, 0xA6, 0x80) => "Corr +",
    Color::from_rgb8(0xB3, 0x59, 0x59) => "Corr -",
    Color::from_rgb8(0xFF, 0x00, 0x00) => "Low",
    Color::from_rgb8(0x00, 0xFF, 0x00) => "Mid",
    Color::from_rgb8(0x00, 0x00, 0xFF) => "High",
    Color::from_rgba8(0x80, 0x80, 0x80, 64.0 / 255.0) => "Grid",
});

palette!(loudness {
    Color::from_rgb8(0x29, 0x29, 0x29) => "Background",
    Color::from_rgb8(0xA0, 0xAA, 0xAD) => "Low",
    Color::from_rgb8(0xAB, 0xCF, 0xAD) => "Mid",
    Color::from_rgb8(0xFF, 0xB7, 0x54) => "High",
    Color::from_rgb8(0xFF, 0x5C, 0x4F) => "Danger",
    Color::from_rgb8(0xF5, 0xED, 0xC4) => "Peak",
    Color::from_rgba8(0xB7, 0xC2, 0xC9, 224.0 / 255.0) => "Guide",
} => [0.0, 0.16, 0.32, 0.48, 0.64, 0.80, 1.0]);

palette!(background { super::BG_BASE => "Background" });
