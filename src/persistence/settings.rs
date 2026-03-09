// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo
mod bar;
mod palette;
mod schema;
mod store;
mod theme;
mod visuals;

pub use bar::{BAR_MAX_HEIGHT, BAR_MIN_HEIGHT, BarAlignment, BarSettings, clamp_bar_height};
pub use palette::{HasPalette, PaletteSettings};
pub use store::{SettingsHandle, SettingsManager};
pub use theme::{BUILTIN_THEME, ThemeChoice, ThemeFile};
pub use visuals::{
    ChannelMode, CorrelationMeterMode, CorrelationMeterSide, LoudnessSettings, MeterMode,
    ModuleSettings, OscilloscopeSettings, PianoRollOverlay, SpectrogramSettings,
    SpectrumDisplayMode, SpectrumSettings, SpectrumWeightingMode, StereometerMode,
    StereometerScale, StereometerSettings, VisualSettings, WaveformColorMode, WaveformSettings,
};
