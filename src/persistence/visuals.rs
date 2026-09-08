// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{lossy, palette::PaletteSettings};
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
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use tracing::warn;

crate::macros::default_struct! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
    pub struct PopoutWindowSettings {
        pub width: u32 = 0,
        pub height: u32 = 0,
        #[serde(skip_serializing_if = "std::clone::Clone::clone")]
        pub popped_out: bool = true,
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct VisualSettings {
    modules: BTreeMap<VisualKind, ModuleSettings>,
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
                out.width_basis = visual_map(value, "visuals.width_basis", |_, value, scope| {
                    width_basis(value, scope)
                });
            }
            if let Some(value) = map.remove("popouts") {
                out.popouts = visual_map(value, "visuals.popouts", |_, value, scope| {
                    popout_window(value, scope)
                });
            }
        })
    }

    pub(crate) fn module_config(&self, kind: VisualKind) -> (VisualConfig, bool) {
        match self.modules.get(&kind) {
            Some(module) => (module.config.clone(), module.enabled),
            None => (VisualConfig::default_for(kind), false),
        }
    }

    pub(crate) fn set_enabled(&mut self, kind: VisualKind, enabled: bool) {
        self.modules
            .entry(kind)
            .or_insert_with(|| ModuleSettings::new(kind))
            .enabled = enabled;
    }

    pub(crate) fn set_config(&mut self, config: VisualConfig) {
        let kind = config.kind();
        self.modules
            .entry(kind)
            .or_insert_with(|| ModuleSettings::new(kind))
            .config = config.normalized();
    }
}

fn visual_map<T>(
    value: Value,
    scope: &str,
    mut parse: impl FnMut(VisualKind, Value, &str) -> Option<T>,
) -> BTreeMap<VisualKind, T> {
    lossy::object(value, scope)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(key, value)| {
            let scope = format!("{scope}.{key}");
            let kind = lossy::value(Value::String(key), &scope)?;
            parse(kind, value, &scope).map(|value| (kind, value))
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
    crate::util::finite_positive(basis).or_else(|| {
        warn!("[settings] invalid {scope}: must be finite and greater than zero");
        None
    })
}

fn popout_window(value: Value, scope: &str) -> Option<PopoutWindowSettings> {
    let mut map = lossy::object(value, scope)?;
    let mut out = PopoutWindowSettings::default();
    lossy::fields!(&mut map, out, scope; width, height, popped_out);
    lossy::unknown(scope, &map);
    Some(out)
}

#[derive(Debug, Clone, Serialize)]
struct ModuleSettings {
    enabled: bool,
    config: VisualConfig,
}

impl ModuleSettings {
    fn new(kind: VisualKind) -> Self {
        Self {
            enabled: false,
            config: VisualConfig::default_for(kind),
        }
    }

    fn from_value_lossy(kind: VisualKind, value: Value, scope: &str) -> Option<Self> {
        let mut map = lossy::object(value, scope)?;
        let mut enabled = false;
        lossy::field(&mut map, "enabled", &mut enabled, scope);
        let config = VisualConfig::from_value_lossy(
            kind,
            map.remove("config").unwrap_or_default(),
            &format!("{scope}.config"),
        );
        lossy::unknown(scope, &map);
        Some(Self { enabled, config })
    }
}

macro_rules! visual_configs {
    ($($variant:ident($settings:ident)),* $(,)?) => {
        #[derive(Debug, Clone, PartialEq, Serialize)]
        #[serde(untagged)]
        pub enum VisualConfig {
            $($variant($settings)),*
        }

        impl VisualConfig {
            pub(crate) fn kind(&self) -> VisualKind {
                match self { $(Self::$variant(_) => VisualKind::$variant),* }
            }

            pub(crate) fn default_for(kind: VisualKind) -> Self {
                match kind { $(VisualKind::$variant => Self::$variant($settings::default())),* }
            }

            fn from_value_lossy(kind: VisualKind, value: Value, scope: &str) -> Self {
                if value.is_null() {
                    return Self::default_for(kind);
                }
                match kind {
                    $(VisualKind::$variant => Self::$variant($settings::from_value_lossy(value, scope))),*
                }.normalized()
            }

            pub(crate) fn palette(&self) -> Option<&PaletteSettings> {
                match self { $(Self::$variant(settings) => settings.palette.as_ref()),* }
            }

            pub(crate) fn palette_mut(&mut self) -> &mut Option<PaletteSettings> {
                match self { $(Self::$variant(settings) => &mut settings.palette),* }
            }

            /// Replaces non-finite settings fields with defaults, leaving palettes unchanged.
            /// Processors and visual state enforce runtime constraints.
            pub(crate) fn normalized(mut self) -> Self {
                match &mut self { $(Self::$variant(settings) => settings.normalize()),* }
                self
            }
        }
    };
}

visual_configs! {
    Loudness(LoudnessSettings),
    Oscilloscope(OscilloscopeSettings),
    Waveform(WaveformSettings),
    Spectrogram(SpectrogramSettings),
    Spectrum(SpectrumSettings),
    Stereometer(StereometerSettings),
}

macro_rules! visual_settings {
    (@normalize f32, $value:expr, $default:expr) => {
        $value = crate::util::finite_or($value, $default);
    };
    (@normalize AveragingMode, $value:expr, $default:expr) => {
        if matches!($value, AveragingMode::Exponential { factor: value }
            | AveragingMode::PeakHold { decay_per_second: value } if !value.is_finite())
        {
            $value = $default;
        }
    };
    (@normalize $ty:ident, $value:expr, $default:expr) => {};
    ($name:ident from $config_ty:ty { $($field:ident : $ty:ident),* $(,)? } $(extra { $($extra:ident : $extra_ty:ident = $default:expr),* $(,)? })?) => {
        visual_settings!($name {
            $($field: $ty = <$config_ty>::default().$field,)*
            $($($extra: $extra_ty = $default,)*)?
        });
        impl $name {
            pub fn apply_to(&self, cfg: &mut $config_ty) { $(cfg.$field = self.$field;)* }
            pub fn sync_from_config(&mut self, cfg: &$config_ty) { $(self.$field = cfg.$field;)* }
        }
    };
    ($name:ident { $($field:ident : $ty:ident = $default:expr),* $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Serialize)]
        pub struct $name { $(pub $field: $ty,)*
            #[serde(skip_serializing)]
            pub palette: Option<PaletteSettings>
        }
        impl Default for $name { fn default() -> Self { Self { $($field: $default,)* palette: None } } }
        impl $name {
            fn from_value_lossy(value: Value, scope: &str) -> Self {
                lossy::settings(value, scope, Self::default(), |map, out| {
                    lossy::fields!(map, out, scope; $($field,)* palette);
                })
            }
            fn normalize(&mut self) {
                $(visual_settings!(@normalize $ty, self.$field, $default);)*
            }
        }
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
    source: Channel, secondary_source: Channel, floor_db: f32,
} extra {
    frequency_scale: FrequencyScale = FrequencyScale::Logarithmic,
    reverse_frequency: bool = false, show_grid: bool = true, show_peak_label: bool = true,
    display_mode: SpectrumDisplayMode = SpectrumDisplayMode::default(),
    weighting_mode: SpectrumWeightingMode = SpectrumWeightingMode::default(),
    secondary_weighting_mode: SpectrumWeightingMode = SpectrumWeightingMode::default(),
    bar_count: usize = 64,
    bar_gap: f32 = 0.16,
    highlight_threshold: f32 = 0.52,
});

visual_settings!(SpectrogramSettings from SpectrogramConfig {
    fft_size: usize, hop_size: usize, window: WindowKind, use_reassignment: bool,
    zero_padding_factor: usize,
} extra {
    frequency_scale: FrequencyScale = FrequencyScale::default(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn saving_writes_canonical_typed_settings() {
        let mut settings = VisualSettings::from_value_lossy(json!({"modules": {
            "spectrum": {"enabled": true, "config": {
                "fft_size": 2048, "floor_db": "quiet", "future_option": 42
            }},
            "waveform": {"enabled": true, "config": false}
        }}));
        assert_eq!(
            settings.module_config(VisualKind::Waveform),
            (VisualConfig::Waveform(WaveformSettings::default()), true)
        );
        settings.set_enabled(VisualKind::Spectrum, false);
        settings.set_enabled(VisualKind::Loudness, true);
        settings.set_config(VisualConfig::Waveform(WaveformSettings {
            scroll_speed: 72.0,
            ..Default::default()
        }));
        let saved = serde_json::to_value(&settings).unwrap();
        let modules = &saved["modules"];
        assert_eq!(modules["spectrum"]["enabled"], false);
        assert_eq!(modules["spectrum"]["config"]["fft_size"], 2048);
        assert_eq!(
            modules["spectrum"]["config"]["floor_db"],
            SpectrumSettings::default().floor_db
        );
        assert!(modules["spectrum"]["config"].get("future_option").is_none());
        assert_eq!(modules["waveform"]["enabled"], true);
        assert_eq!(modules["waveform"]["config"]["scroll_speed"], 72.0);
        assert_eq!(modules["loudness"]["enabled"], true);
        assert_eq!(modules["loudness"]["config"]["left_mode"], "true_peak");
        let loaded = VisualSettings::from_value_lossy(saved.clone());
        assert_eq!(serde_json::to_value(loaded).unwrap(), saved);
    }
}
