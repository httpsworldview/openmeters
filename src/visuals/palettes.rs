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
            VisualKind::Chroma => p!(chroma),
        }
    }
}

// Spectrogram heat map: quiet -> loud (5 stops)
pub mod spectrogram {
    use super::{Color, hex};
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

pub mod spectrum {
    use super::{Color, hex};
    pub const COLORS: [Color; 6] = [
        hex(0x00, 0x00, 0x00, 0x00),
        hex(0x38, 0x00, 0xAD, 0xFF),
        hex(0xFF, 0x00, 0x00, 0xFF),
        hex(0xFF, 0xFF, 0x21, 0xFF),
        hex(0xFF, 0xFF, 0xFF, 0xFF),
        hex(0xFF, 0xFF, 0xFF, 0xFF),
    ];
    pub const LABELS: &[&str] = &["Floor", "Low", "Low-Mid", "Mid", "High", "Peak"];
    pub const DEFAULT_POSITIONS: [f32; COLORS.len()] = [0.0, 0.2, 0.4, 0.6, 0.8, 1.0];
}

// dark red (low) -> orange -> green -> cyan -> blue (high)
pub mod waveform {
    use super::{Color, hex};
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
    use super::{Color, hex};
    pub const COLORS: [Color; 1] = [hex(0xFF, 0xFF, 0xFF, 0xFF)];
    pub const LABELS: &[&str] = &["Trace"];
    pub const DEFAULT_POSITIONS: [f32; COLORS.len()] = [0.0];
}

// Stereometer (9 stops)
pub mod stereometer {
    use super::{Color, hex};
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

// Loudness meter: background, level zones, peak hold, guide line (7 stops)
pub mod loudness {
    use super::{Color, hex};
    pub const COLORS: [Color; 7] = [
        hex(0x29, 0x29, 0x29, 0xFF),
        hex(0xA0, 0xAA, 0xAD, 0xFF),
        hex(0xAB, 0xCF, 0xAD, 0xFF),
        hex(0xFF, 0xB7, 0x54, 0xFF),
        hex(0xFF, 0x5C, 0x4F, 0xFF),
        hex(0xF5, 0xED, 0xC4, 0xFF),
        hex(0xB7, 0xC2, 0xC9, 0xE0),
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

// App background color (1 stop)
pub mod background {
    use super::{BG_BASE, Color};
    pub const COLORS: [Color; 1] = [BG_BASE];
    pub const LABELS: &[&str] = &["Background"];
    pub const DEFAULT_POSITIONS: [f32; COLORS.len()] = [0.0];
}

// Chromagram: 12 pitch-class colors (C–B) + 1 peak-hold color
pub mod chroma {
    use super::{Color, hex};
    pub const COLORS: [Color; 13] = [
        hex(0xFF, 0x30, 0x30, 0xFF), // C  – red
        hex(0xFF, 0x70, 0x10, 0xFF), // C# – red-orange
        hex(0xFF, 0xA8, 0x00, 0xFF), // D  – orange
        hex(0xD4, 0xD4, 0x00, 0xFF), // D# – yellow
        hex(0x80, 0xFF, 0x00, 0xFF), // E  – yellow-green
        hex(0x00, 0xFF, 0x50, 0xFF), // F  – green
        hex(0x00, 0xFF, 0xC0, 0xFF), // F# – cyan-green
        hex(0x00, 0xC0, 0xFF, 0xFF), // G  – sky blue
        hex(0x00, 0x60, 0xFF, 0xFF), // G# – blue
        hex(0x80, 0x00, 0xFF, 0xFF), // A  – violet
        hex(0xC0, 0x00, 0xFF, 0xFF), // A# – purple
        hex(0xFF, 0x00, 0x90, 0xFF), // B  – magenta
        hex(0xFF, 0xFF, 0xFF, 0xFF), // Peak hold marker
    ];
    pub const LABELS: &[&str] = &[
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B", "Peak",
    ];
    pub const DEFAULT_POSITIONS: [f32; COLORS.len()] = [
        0.0,
        1.0 / 12.0,
        2.0 / 12.0,
        3.0 / 12.0,
        4.0 / 12.0,
        5.0 / 12.0,
        6.0 / 12.0,
        7.0 / 12.0,
        8.0 / 12.0,
        9.0 / 12.0,
        10.0 / 12.0,
        11.0 / 12.0,
        1.0,
    ];
}
