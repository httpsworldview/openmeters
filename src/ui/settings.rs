// Settings persistence and management.

mod bar;
mod data;
mod palette;
mod persistence;
mod visuals;

pub use bar::{BAR_MAX_HEIGHT, BAR_MIN_HEIGHT, BarAlignment, BarSettings, clamp_bar_height};
pub use palette::{HasPalette, PaletteSettings};
pub use persistence::{SettingsHandle, SettingsManager};
pub use visuals::{
    ChannelMode, CorrelationMeterMode, CorrelationMeterSide, LoudnessSettings, MeterMode,
    ModuleSettings, OscilloscopeSettings, PianoRollOverlay, SpectrogramSettings,
    SpectrumDisplayMode, SpectrumSettings, SpectrumWeightingMode, StereometerMode,
    StereometerScale, StereometerSettings, VisualSettings, WaveformColorMode, WaveformSettings,
};
