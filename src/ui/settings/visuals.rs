use super::palette::{HasPalette, PaletteSettings};
use crate::{
    dsp::{
        oscilloscope::{OscilloscopeConfig, TriggerMode},
        spectrogram::{FrequencyScale, SpectrogramConfig, WindowKind},
        spectrum::{AveragingMode, SpectrumConfig},
        stereometer::StereometerConfig,
        waveform::WaveformConfig,
    },
    ui::visualization::visual_manager::VisualKind,
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
                $(VisualKind::$k => m.config.as_ref().is_none_or(|v| serde_json::from_value::<$t>(v.clone()).is_ok())),+
            }};
        }
        let valid = check!(Spectrogram => SpectrogramSettings, Spectrum => SpectrumSettings,
            Oscilloscope => OscilloscopeSettings, Waveform => WaveformSettings,
            Loudness => LoudnessSettings, Stereometer => StereometerSettings);
        self.modules.retain(valid);
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
        self.config
            .as_ref()
            .and_then(|val| T::deserialize(val).ok())
    }
    pub fn config_or_default<T: DeserializeOwned + Default>(&self) -> T {
        self.config
            .as_ref()
            .and_then(|val| {
                T::deserialize(val)
                    .map_err(|err| warn!("[settings] config parse error: {err}"))
                    .ok()
            })
            .unwrap_or_default()
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

settings_enum!(pub enum ChannelMode {
    Both => "Left + Right", Left => "Left only", Right => "Right only", #[default] Mono => "Mono blend",
});

impl ChannelMode {
    // Returns output channel count for this mode.
    #[inline]
    pub fn output_channels(self, input_channels: usize) -> usize {
        match self {
            Self::Both => input_channels,
            _ => 1,
        }
    }
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
    persistence: f32 = 0.0, channel_mode: ChannelMode = ChannelMode::default(),
});

visual_settings!(WaveformSettings from WaveformConfig {
    scroll_speed: f32,
} extra {
    channel_mode: ChannelMode = ChannelMode::default(),
    color_mode: WaveformColorMode = WaveformColorMode::default(),
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
    fft_size: usize, hop_size: usize, history_length: usize, window: WindowKind, frequency_scale: FrequencyScale,
    use_reassignment: bool,
    zero_padding_factor: usize, display_bin_count: usize,
    reassignment_max_correction_hz: f32,
} extra {
    floor_db: f32 = -96.0,
    piano_roll_overlay: PianoRollOverlay = PianoRollOverlay::default(),
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
