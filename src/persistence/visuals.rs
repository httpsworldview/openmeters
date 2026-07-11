// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{
    lossy,
    palette::{HasPalette, PaletteSettings},
};
use crate::domain::visuals::VisualKind;
use crate::util::audio::{Channel, FrequencyScale, WindowKind};
use crate::visuals::options::{
    CorrelationMeterMode, CorrelationMeterSide, MeterMode, PianoRollOverlay, SpectrumDisplayMode,
    SpectrumWeightingMode, StereometerMode, StereometerScale, WaveformColorMode,
    WaveformHistoryMode,
};
use crate::visuals::{
    oscilloscope::processor::{OscilloscopeConfig, TriggerMode},
    spectrogram::processor::SpectrogramConfig,
    spectrum::processor::{AveragingMode, SpectrumConfig},
    stereometer::processor::StereometerConfig,
    waveform::processor::{DEFAULT_BAND_DB_FLOOR, WaveformConfig},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use tracing::warn;

fn is_true(value: &bool) -> bool {
    *value
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PopoutWindowSettings {
    pub width: u32,
    pub height: u32,
    #[serde(skip_serializing_if = "is_true")]
    pub popped_out: bool,
}

impl Default for PopoutWindowSettings {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            popped_out: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VisualSettings {
    pub modules: BTreeMap<VisualKind, ModuleSettings>,
    pub order: Vec<VisualKind>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub width_basis: BTreeMap<VisualKind, f32>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub popouts: BTreeMap<VisualKind, PopoutWindowSettings>,
}

impl VisualSettings {
    pub(super) fn from_value_lossy(value: Value) -> Self {
        lossy::settings(value, "visuals", Self::default(), |map, out| {
            if let Some(value) = map.remove("modules") {
                out.modules =
                    visual_map(value, "visuals.modules", ModuleSettings::from_value_lossy);
            }
            if let Some(value) = map.remove("order") {
                out.order = visual_order(value);
            }
            if let Some(value) = map.remove("width_basis") {
                out.width_basis = visual_map(value, "visuals.width_basis", width_basis);
            }
            if let Some(value) = map.remove("popouts") {
                out.popouts = visual_map(value, "visuals.popouts", popout_window);
            }
        })
    }
}

fn visual_map<T>(
    value: Value,
    scope: &str,
    mut parse: impl FnMut(Value, &str) -> Option<T>,
) -> BTreeMap<VisualKind, T> {
    lossy::object(value, scope)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(key, value)| {
            let scope = format!("{scope}.{key}");
            let kind = lossy::value(Value::String(key), &scope)?;
            parse(value, &scope).map(|value| (kind, value))
        })
        .collect()
}

fn visual_order(value: Value) -> Vec<VisualKind> {
    let Value::Array(items) = value else {
        warn!("[settings] visuals.order must be an array");
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|value| lossy::value(value, "visuals.order item"))
        .collect()
}

fn width_basis(value: Value, scope: &str) -> Option<f32> {
    let basis: f32 = lossy::value(value, scope)?;
    if let Some(basis) = crate::util::finite_positive(basis) {
        Some(basis)
    } else {
        warn!("[settings] invalid {scope}: must be finite and greater than zero");
        None
    }
}

fn popout_window(value: Value, scope: &str) -> Option<PopoutWindowSettings> {
    let mut map = lossy::object(value, scope)?;
    let mut out = PopoutWindowSettings::default();
    lossy::fields!(&mut map, out, scope; width, height, popped_out);
    lossy::unknown(scope, &map);
    Some(out)
}

pub(crate) trait SettingsConfig: Default {
    fn from_value_lossy(value: Value, scope: &str) -> Self;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModuleSettings {
    pub enabled: Option<bool>,
    config: Option<Value>,
}

impl ModuleSettings {
    fn from_value_lossy(value: Value, scope: &str) -> Option<Self> {
        let mut map = lossy::object(value, scope)?;
        let mut out = Self::default();
        lossy::field(&mut map, "enabled", &mut out.enabled, scope);
        out.config = map.remove("config");
        lossy::unknown(scope, &map);
        Some(out)
    }

    pub(crate) fn with_config<T: Serialize>(config: &T) -> Self {
        Self {
            enabled: None,
            config: serde_json::to_value(config).ok(),
        }
    }
    pub(crate) fn set_config<T: Serialize>(&mut self, config: &T) {
        self.config = serde_json::to_value(config).ok();
    }
    pub(crate) fn parse_config<T: SettingsConfig>(&self) -> Option<T> {
        self.config
            .clone()
            .filter(|value| !value.is_null())
            .map(|value| T::from_value_lossy(value, "config"))
    }
    pub(crate) fn override_palette(&mut self, palette: Option<&PaletteSettings>) {
        let obj = self
            .config
            .get_or_insert_with(|| Value::Object(serde_json::Map::new()));
        if let Value::Object(map) = obj {
            match palette.and_then(|p| serde_json::to_value(p).ok()) {
                Some(value) => map.insert("palette".into(), value),
                None => map.remove("palette"),
            };
        }
    }

    pub(crate) fn extract_palette(&self) -> Option<PaletteSettings> {
        self.config
            .as_ref()
            .and_then(|v| v.get("palette"))
            .and_then(|pal| PaletteSettings::deserialize(pal).ok())
    }

    pub(super) fn strip_palette(&mut self) {
        if let Some(Value::Object(map)) = &mut self.config {
            map.remove("palette");
        }
    }
}

macro_rules! visual_settings {
    (@impls $name:ident { $($field:ident),* $(,)? }) => {
        impl HasPalette for $name {
            fn palette(&self) -> Option<&PaletteSettings> { self.palette.as_ref() }
            fn set_palette(&mut self, palette: Option<PaletteSettings>) { self.palette = palette; }
        }
        impl SettingsConfig for $name {
            fn from_value_lossy(value: Value, scope: &str) -> Self {
                lossy::settings(value, scope, Self::default(), |map, out| {
                    lossy::fields!(map, out, scope; $($field),*);
                })
            }
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
            pub fn sync_from_config(&mut self, cfg: &$config_ty) { $(self.$field = cfg.$field;)* }
        }
        visual_settings!(@impls $name { $($field,)* $($($extra,)*)? palette });
    };
    ($name:ident { $($field:ident : $ty:ty = $default:expr),* $(,)? }) => {
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(default)]
        pub struct $name { $(pub $field: $ty,)* pub palette: Option<PaletteSettings> }
        impl Default for $name { fn default() -> Self { Self { $($field: $default,)* palette: None } } }
        visual_settings!(@impls $name { $($field,)* palette });
    };
}

visual_settings!(OscilloscopeSettings from OscilloscopeConfig {
    segment_duration: f32, trigger_mode: TriggerMode, trigger_source: Channel,
    channel_1: Channel, channel_2: Channel,
} extra {
    persistence: f32 = 0.0,
    stacked: bool = false,
});

visual_settings!(WaveformSettings from WaveformConfig {
    scroll_speed: f32,
} extra {
    band_db_floor: f32 = DEFAULT_BAND_DB_FLOOR,
    channel_1: Channel = Channel::Mid,
    channel_2: Channel = Channel::None,
    color_mode: WaveformColorMode = WaveformColorMode::default(),
    history_mode: WaveformHistoryMode = WaveformHistoryMode::default(),
});

visual_settings!(SpectrumSettings from SpectrumConfig {
    fft_size: usize, hop_size: usize, window: WindowKind, averaging: AveragingMode,
    source: Channel, secondary_source: Channel,
    frequency_scale: FrequencyScale, reverse_frequency: bool, show_grid: bool, show_peak_label: bool,
    floor_db: f32,
} extra {
    display_mode: SpectrumDisplayMode = SpectrumDisplayMode::default(),
    weighting_mode: SpectrumWeightingMode = SpectrumWeightingMode::default(),
    secondary_weighting_mode: SpectrumWeightingMode = SpectrumWeightingMode::default(),
    bar_count: usize = 64,
    bar_gap: f32 = 0.16,
    highlight_threshold: f32 = 0.52,
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
    dot_radius: f32 = 1.5, mode: StereometerMode = StereometerMode::default(),
    scale: StereometerScale = StereometerScale::default(), rotation: i8 = -1, flip: bool = true,
    unipolar: bool = false,
    correlation_meter: CorrelationMeterMode = CorrelationMeterMode::default(),
    correlation_meter_side: CorrelationMeterSide = CorrelationMeterSide::default(),
});

visual_settings!(LoudnessSettings {
    left_mode: MeterMode = MeterMode::TruePeak,
    right_mode: MeterMode = MeterMode::LufsShortTerm,
});
