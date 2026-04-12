// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::palette::{HasPalette, PaletteSettings};
use crate::domain::visuals::VisualKind;
use crate::visuals::{
    oscilloscope::processor::{OscilloscopeConfig, TriggerMode},
    spectrogram::processor::{FrequencyScale, SpectrogramConfig, WindowKind},
    spectrum::processor::{AveragingMode, SpectrumConfig},
    stereometer::processor::StereometerConfig,
    waveform::processor::WaveformConfig,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::HashMap;
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VisualSettings {
    pub modules: HashMap<VisualKind, ModuleSettings>,
    pub order: Vec<VisualKind>,
}

impl VisualSettings {
    pub fn sanitize(&mut self) {
        macro_rules! check {
            ($($k:ident => $t:ty),+) => {|k: &VisualKind, m: &mut ModuleSettings| match k {
                $(VisualKind::$k => m.config.as_ref().is_none_or(|v| <$t>::deserialize(v).is_ok())),+
            }};
        }
        let valid = check!(Spectrogram => SpectrogramSettings, Spectrum => SpectrumSettings,
            Oscilloscope => OscilloscopeSettings, Waveform => WaveformSettings,
            Loudness => LoudnessSettings, Stereometer => StereometerSettings);
        self.modules.retain(valid);
    }

    /// Strips palette data from all module configs (theme owns palettes, not settings.json).
    pub fn strip_all_palettes(&mut self) {
        for ms in self.modules.values_mut() {
            ms.strip_palette();
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModuleSettings {
    pub enabled: Option<bool>,
    // Module configuration payload.
    //
    // Persisted format is `{ "enabled": bool?, "config": <json>? }`.
    config: Option<Value>,
}

impl ModuleSettings {
    pub fn with_config<T: Serialize>(config: &T) -> Self {
        Self {
            enabled: None,
            config: serde_json::to_value(config).ok(),
        }
    }
    pub fn set_config<T: Serialize>(&mut self, config: &T) {
        self.config = serde_json::to_value(config).ok();
    }
    pub fn parse_config<T: DeserializeOwned>(&self) -> Option<T> {
        self.config.as_ref().and_then(|v| T::deserialize(v).ok())
    }
    pub fn config_or_default<T: DeserializeOwned + Default>(&self) -> T {
        self.config
            .as_ref()
            .and_then(|v| {
                T::deserialize(v)
                    .inspect_err(|e| warn!("[settings] config parse error: {e}"))
                    .ok()
            })
            .unwrap_or_default()
    }
    /// Replaces the palette field inside the config JSON without touching other fields.
    pub fn override_palette(&mut self, palette: Option<&PaletteSettings>) {
        let obj = self
            .config
            .get_or_insert_with(|| Value::Object(Default::default()));
        if let Value::Object(map) = obj {
            if let Some(v) = palette.and_then(|p| serde_json::to_value(p).ok()) {
                map.insert("palette".into(), v);
            } else {
                map.remove("palette");
            }
        }
    }

    /// Extracts palette data from the config JSON without consuming it.
    pub fn extract_palette(&self) -> Option<PaletteSettings> {
        self.config
            .as_ref()
            .and_then(|v| v.get("palette"))
            .and_then(|pal| serde_json::from_value(pal.clone()).ok())
    }

    /// Removes palette data from the config JSON (for clean settings.json persistence).
    pub fn strip_palette(&mut self) {
        if let Some(Value::Object(map)) = &mut self.config {
            map.remove("palette");
        }
    }
}

macro_rules! settings_enum {
    ($(#[$attr:meta])* $vis:vis enum $name:ident { $($(#[$var_attr:meta])* $variant:ident => $label:expr),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
        #[serde(rename_all = "snake_case")] $(#[$attr])*
        $vis enum $name { $($(#[$var_attr])* $variant,)+ }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(match self { $(Self::$variant => $label),+ })
            }
        }
    };
}

macro_rules! visual_settings {
    (@has_palette $name:ident) => {
        impl HasPalette for $name {
            fn palette(&self) -> Option<&PaletteSettings> { self.palette.as_ref() }
            fn set_palette(&mut self, palette: Option<PaletteSettings>) { self.palette = palette; }
        }
    };
    ($name:ident from $config_ty:ty { $($field:ident : $ty:ty),* $(,)? } $(extra { $($extra:ident : $extra_ty:ty = $default:expr),* $(,)? })?) => {
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(default)]
        pub struct $name { $(pub $field: $ty,)* $($(pub $extra: $extra_ty,)*)? pub palette: Option<PaletteSettings> }
        impl Default for $name { fn default() -> Self { Self::from_config(&<$config_ty>::default()) } }
        impl $name {
            pub fn from_config(cfg: &$config_ty) -> Self {
                Self { $($field: cfg.$field,)* $($($extra: $default,)*)? palette: None }
            }
            pub fn apply_to(&self, cfg: &mut $config_ty) { $(cfg.$field = self.$field;)* }
        }
        visual_settings!(@has_palette $name);
    };
    ($name:ident { $($field:ident : $ty:ty = $default:expr),* $(,)? }) => {
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(default)]
        pub struct $name { $(pub $field: $ty,)* pub palette: Option<PaletteSettings> }
        impl Default for $name { fn default() -> Self { Self { $($field: $default,)* palette: None } } }
        visual_settings!(@has_palette $name);
    };
}

settings_enum!(pub enum Channel {
    #[default]
    Left => "Left",
    Right => "Right",
    Mid => "Mid",
    Side => "Side",
    None => "None",
});

impl Channel {
    pub const ALL: &'static [Channel] = &[
        Channel::Left,
        Channel::Right,
        Channel::Mid,
        Channel::Side,
        Channel::None,
    ];
}
settings_enum!(pub enum StereometerMode  { Lissajous => "Lissajous", #[default] DotCloud => "Dot Cloud" });
settings_enum!(pub enum StereometerScale { Linear => "Linear", #[default] Exponential => "Exponential" });
settings_enum!(pub enum CorrelationMeterMode { Off => "Off", SingleBand => "Single Band", #[default] MultiBand => "Multi Band" });
settings_enum!(pub enum CorrelationMeterSide { Left => "Left", #[default] Right => "Right" });
settings_enum!(pub enum PianoRollOverlay { #[default] Off => "Off", Right => "Right", Left => "Left" });

settings_enum!(pub enum MeterMode {
    #[default]
    LufsShortTerm => "LUFS Short-term",
    LufsMomentary => "LUFS Momentary",
    RmsFast => "RMS Fast",
    RmsSlow => "RMS Slow",
    TruePeak => "True Peak",
});

impl MeterMode {
    pub const ALL: &'static [MeterMode] = &[
        MeterMode::LufsShortTerm,
        MeterMode::LufsMomentary,
        MeterMode::RmsFast,
        MeterMode::RmsSlow,
        MeterMode::TruePeak,
    ];

    pub fn unit_label(self) -> &'static str {
        match self {
            MeterMode::LufsShortTerm | MeterMode::LufsMomentary => "LUFS",
            MeterMode::RmsFast | MeterMode::RmsSlow | MeterMode::TruePeak => "dB",
        }
    }
}
settings_enum!(pub enum SpectrumDisplayMode { #[default] Line => "Line", Bar => "Bar" });
settings_enum!(pub enum SpectrumWeightingMode { #[default] AWeighted => "A-Weighted", Raw => "Raw" });
settings_enum!(pub enum WaveformColorMode { #[default] Frequency => "Frequency", Loudness => "Loudness", Static => "Static" });

visual_settings!(OscilloscopeSettings from OscilloscopeConfig {
    segment_duration: f32, trigger_mode: TriggerMode,
} extra {
    persistence: f32 = 0.0, channel_1: Channel = Channel::Mid, channel_2: Channel = Channel::None,
});

visual_settings!(WaveformSettings from WaveformConfig {
    scroll_speed: f32, band_db_floor: f32,
} extra {
    channel_1: Channel = Channel::Mid, channel_2: Channel = Channel::None,
    color_mode: WaveformColorMode = WaveformColorMode::default(),
    show_peak_history: bool = false,
});

visual_settings!(SpectrumSettings from SpectrumConfig {
    fft_size: usize, hop_size: usize, window: WindowKind, averaging: AveragingMode,
    frequency_scale: FrequencyScale, reverse_frequency: bool, show_grid: bool, show_peak_label: bool,
} extra {
    smoothing_radius: usize = 0, smoothing_passes: usize = 0,
    display_mode: SpectrumDisplayMode = SpectrumDisplayMode::default(),
    weighting_mode: SpectrumWeightingMode = SpectrumWeightingMode::default(),
    show_secondary_line: bool = true,
    bar_count: usize = 64,
    bar_gap: f32 = 0.2,
    highlight_threshold: f32 = 0.45,
});

visual_settings!(SpectrogramSettings from SpectrogramConfig {
    fft_size: usize, hop_size: usize, window: WindowKind, frequency_scale: FrequencyScale,
    use_reassignment: bool,
    zero_padding_factor: usize,
} extra {
    floor_db: f32 = -96.0,
    tilt_db: f32 = 0.0,
    piano_roll_overlay: PianoRollOverlay = PianoRollOverlay::default(),
    rotation: i8 = 0,
});

visual_settings!(StereometerSettings from StereometerConfig {
    segment_duration: f32, target_sample_count: usize, correlation_window: f32,
} extra {
    persistence: f32 = 0.0, mode: StereometerMode = StereometerMode::default(),
    scale: StereometerScale = StereometerScale::default(), scale_range: f32 = 15.0, rotation: i8 = -1, flip: bool = true,
    correlation_meter: CorrelationMeterMode = CorrelationMeterMode::default(),
    correlation_meter_side: CorrelationMeterSide = CorrelationMeterSide::default(),
});

visual_settings!(LoudnessSettings {
    left_mode: MeterMode = MeterMode::TruePeak,
    right_mode: MeterMode = MeterMode::LufsShortTerm,
});
