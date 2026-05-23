// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

crate::macros::choice_enum!(all pub enum StereometerMode {
    Lissajous => "Lissajous",
    #[default] DotCloud => "Dot Cloud",
    DotCloudBands => "Dot Cloud (Bands)",
});
crate::macros::choice_enum!(all pub enum StereometerScale { Linear => "Linear", #[default] Exponential => "Exponential" });
crate::macros::choice_enum!(all pub enum CorrelationMeterMode { Off => "Off", SingleBand => "Single Band", #[default] MultiBand => "Multi Band" });
crate::macros::choice_enum!(all pub enum CorrelationMeterSide { Left => "Left", #[default] Right => "Right" });
crate::macros::choice_enum!(all pub enum PianoRollOverlay { #[default] Off => "Off", Right => "Right", Left => "Left" });

crate::macros::choice_enum!(all pub enum MeterMode {
    #[default]
    LufsShortTerm => "LUFS Short-term",
    LufsMomentary => "LUFS Momentary",
    RmsFast => "RMS Fast",
    RmsSlow => "RMS Slow",
    TruePeak => "True Peak",
});

impl MeterMode {
    pub fn unit_label(self) -> &'static str {
        match self {
            Self::LufsShortTerm | Self::LufsMomentary => "LUFS",
            Self::RmsFast | Self::RmsSlow => "dB",
            Self::TruePeak => "dBTP",
        }
    }
}

crate::macros::choice_enum!(all pub enum SpectrumDisplayMode { #[default] Line => "Line", Bar => "Bar" });
crate::macros::choice_enum!(all pub enum SpectrumWeightingMode { #[default] AWeighted => "A-Weighted", Raw => "Raw" });
crate::macros::choice_enum!(all pub enum WaveformColorMode { #[default] Frequency => "Frequency", Loudness => "Loudness", Static => "Static" });
