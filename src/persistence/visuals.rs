// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::lossy;
use crate::domain::visuals::VisualKind;
use crate::util::Finite;
use crate::util::audio::{Channel, FftSize, FrequencyScale, NegativeDb, Seconds, WindowKind};
use crate::visuals::options::{
    CorrelationMeterMode, CorrelationMeterSide, MeterMode, PianoRollOverlay, SpectrumDisplayMode,
    SpectrumWeightingMode, StereometerMode, StereometerScale, WaveformColorMode,
    WaveformHistoryMode,
};
use crate::visuals::{
    oscilloscope::processor::{OscilloscopeConfig, TriggerMode},
    spectrogram::processor::SpectrogramConfig,
    spectrum::processor::{AveragingMode, SpectrumConfig},
    stereometer::processor::{Bands as StereometerBands, StereometerConfig},
    waveform::processor::{
        Bands as WaveformBands, DEFAULT_BAND_DB_FLOOR, ScrollSpeed, WaveformConfig,
    },
};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use tracing::warn;

lossy::settings_struct! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
    pub struct PopoutWindowSettings {
        pub width: u32 = 0,
        pub height: u32 = 0,
        #[serde(skip_serializing_if = "std::clone::Clone::clone")]
        pub popped_out: bool = true,
    }
}

crate::macros::default_struct! {
    #[derive(Debug, Clone, PartialEq, Serialize)]
    pub struct VisualSettings {
        modules: BTreeMap<VisualKind, ModuleSettings> = VisualKind::ALL.iter().map(|&kind| {
            (kind, ModuleSettings { enabled: true, ..ModuleSettings::new(kind) })
        }).collect(),
        pub order: Vec<VisualKind> = Vec::new(),
        #[serde(skip_serializing_if = "BTreeMap::is_empty")]
        pub width_basis: BTreeMap<VisualKind, f32> = BTreeMap::new(),
        #[serde(skip_serializing_if = "BTreeMap::is_empty")]
        pub popouts: BTreeMap<VisualKind, PopoutWindowSettings> = BTreeMap::new(),
    }
}

impl VisualSettings {
    pub(super) fn from_value_lossy(value: Value) -> Self {
        lossy::settings(value, "visuals", |map, out: &mut Self, scope| {
            let modules = ModuleSettings::from_value_lossy;
            visual_map(map, scope, "modules", &mut out.modules, modules);
            if let Some(value) = map.remove("order") {
                out.order = visual_order(value);
            }
            visual_map(map, scope, "width_basis", &mut out.width_basis, width_basis);
            visual_map(
                map,
                scope,
                "popouts",
                &mut out.popouts,
                |_, value, scope| {
                    let popout = PopoutWindowSettings::default();
                    lossy::read(value, scope, popout, PopoutWindowSettings::read_fields)
                },
            );
        })
    }

    pub(crate) fn config(&self, kind: VisualKind) -> VisualConfig {
        self.modules.get(&kind).map_or_else(
            || VisualConfig::default_for(kind),
            |module| module.config.clone(),
        )
    }

    pub(crate) fn is_enabled(&self, kind: VisualKind) -> bool {
        self.modules.get(&kind).is_some_and(|module| module.enabled)
    }

    pub(crate) fn popped_out(&self, kind: VisualKind) -> bool {
        self.popouts
            .get(&kind)
            .is_some_and(|popout| popout.popped_out)
    }

    pub(crate) fn popout_mut(&mut self, kind: VisualKind) -> &mut PopoutWindowSettings {
        self.popouts.entry(kind).or_default()
    }

    fn module_mut(&mut self, kind: VisualKind) -> &mut ModuleSettings {
        (self.modules.entry(kind)).or_insert_with(|| ModuleSettings::new(kind))
    }

    pub(crate) fn set_enabled(&mut self, kind: VisualKind, enabled: bool) {
        self.module_mut(kind).enabled = enabled;
    }

    pub(crate) fn set_config(&mut self, config: VisualConfig) {
        let kind = config.kind();
        self.module_mut(kind).config = config;
    }
}

pub(super) fn visual_map<T>(
    map: &mut Map<String, Value>,
    scope: &str,
    key: &str,
    out: &mut BTreeMap<VisualKind, T>,
    mut parse: impl FnMut(VisualKind, Value, &str) -> Option<T>,
) {
    let Some(value) = map.remove(key) else {
        return;
    };
    let scope = format!("{scope}.{key}");
    let Some(map) = lossy::object(value, &scope) else {
        return;
    };
    *out = map
        .into_iter()
        .filter_map(|(key, value)| {
            let scope = format!("{scope}.{key}");
            let kind = lossy::value(Value::String(key), &scope)?;
            parse(kind, value, &scope).map(|value| (kind, value))
        })
        .collect();
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

fn width_basis(_: VisualKind, value: Value, scope: &str) -> Option<f32> {
    let basis: f32 = lossy::value(value, scope)?;
    crate::util::finite_positive(basis).or_else(|| {
        warn!("[settings] invalid {scope}: must be finite and greater than zero");
        None
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
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
        lossy::read(value, scope, Self::new(kind), |map, out, scope| {
            lossy::field(map, "enabled", &mut out.enabled, scope);
            if let Some(value) = map.remove("config") {
                let scope = format!("{scope}.config");
                out.config = VisualConfig::from_value_lossy(kind, value, &scope);
            }
        })
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
                match kind {
                    $(VisualKind::$variant => {
                        Self::$variant(lossy::settings(value, scope, $settings::read_fields))
                    })*
                }
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
    ($name:ident from $config_ty:ty {
        $($field:ident: $ty:ty),* $(,)?
    } $(extra {
        $($extra:ident: $extra_ty:ty = $default:expr),* $(,)?
    })?) => {
        visual_settings!($name {
            $($field: $ty = <$config_ty>::default().$field,)*
            $($($extra: $extra_ty = $default,)*)?
        });
        impl $name {
            pub fn apply_to(&self, cfg: &mut $config_ty) { $(cfg.$field = self.$field;)* }
        }
    };
    ($name:ident { $($field:ident: $ty:ty = $default:expr),* $(,)? }) => {
        lossy::settings_struct! {
            #[derive(Debug, Clone, PartialEq, Serialize)]
            pub struct $name {
                $(pub $field: $ty = $default,)*
            }
        }
    };
}

visual_settings!(OscilloscopeSettings from OscilloscopeConfig {
    segment_duration: Finite, trigger_mode: TriggerMode, trigger_source: Channel,
    channel_1: Channel, channel_2: Channel,
} extra {
    persistence_time_constant: Seconds = Seconds::must(0.0),
    stacked: bool = false,
});

visual_settings!(WaveformSettings from WaveformConfig {
    scroll_speed: ScrollSpeed,
} extra {
    band_db_floor: NegativeDb = NegativeDb::must(DEFAULT_BAND_DB_FLOOR),
    channel_1: Channel = Channel::Mid,
    channel_2: Channel = Channel::None,
    color_mode: WaveformColorMode = WaveformColorMode::default(),
    history_mode: WaveformHistoryMode = WaveformHistoryMode::default(),
});

impl WaveformSettings {
    pub(crate) fn bands(&self) -> WaveformBands {
        if self.history_mode != WaveformHistoryMode::Off {
            WaveformBands::History
        } else if self.color_mode == WaveformColorMode::Frequency {
            WaveformBands::Colors
        } else {
            WaveformBands::Off
        }
    }
}

visual_settings!(SpectrumSettings from SpectrumConfig {
    fft_size: FftSize, hop_size: NonZeroUsize, window: WindowKind, averaging: AveragingMode,
    source: Channel, secondary_source: Channel, floor_db: NegativeDb,
} extra {
    frequency_scale: FrequencyScale = FrequencyScale::Logarithmic,
    reverse_frequency: bool = false, rotation: i8 = 0, show_grid: bool = true,
    show_peak_label: bool = false,
    display_mode: SpectrumDisplayMode = SpectrumDisplayMode::default(),
    weighting_mode: SpectrumWeightingMode = SpectrumWeightingMode::default(),
    secondary_weighting_mode: SpectrumWeightingMode = SpectrumWeightingMode::default(),
    bar_count: usize = 64,
    bar_gap: Finite = Finite::must(0.16),
});

visual_settings!(SpectrogramSettings from SpectrogramConfig {
    fft_size: FftSize, hop_size: NonZeroUsize, window: WindowKind, use_reassignment: bool,
    zero_padding_factor: usize, source: Channel,
} extra {
    frequency_scale: FrequencyScale = FrequencyScale::default(),
    floor_db: NegativeDb = NegativeDb::must(-120.0),
    tilt_db: Finite = Finite::must(4.5),
    piano_roll_overlay: PianoRollOverlay = PianoRollOverlay::default(),
    rotation: i8 = 0,
});

visual_settings!(StereometerSettings from StereometerConfig {
    segment_duration: Finite, target_sample_count: usize, correlation_window: Seconds,
} extra {
    dot_radius: Finite = Finite::must(1.0), mode: StereometerMode = StereometerMode::default(),
    scale: StereometerScale = StereometerScale::default(), rotation: i8 = -1, flip: bool = true,
    unipolar: bool = true,
    correlation_meter: CorrelationMeterMode = CorrelationMeterMode::default(),
    correlation_meter_side: CorrelationMeterSide = CorrelationMeterSide::default(),
});

impl StereometerSettings {
    pub(crate) fn bands(&self) -> StereometerBands {
        if self.mode == StereometerMode::DotCloudBands {
            StereometerBands::Points
        } else if self.correlation_meter == CorrelationMeterMode::MultiBand {
            StereometerBands::Correlations
        } else {
            StereometerBands::Off
        }
    }
}

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
            "waveform": {"enabled": true, "config": false},
            "spectrogram": {"enabled": true, "config": {"source": "side"}}
        }}));
        assert_eq!(
            settings.config(VisualKind::Waveform),
            VisualConfig::Waveform(WaveformSettings::default())
        );
        assert!(settings.is_enabled(VisualKind::Waveform));
        settings.set_enabled(VisualKind::Spectrum, false);
        settings.set_enabled(VisualKind::Loudness, true);
        settings.set_config(VisualConfig::Waveform(WaveformSettings {
            scroll_speed: ScrollSpeed::must(72.0),
            ..Default::default()
        }));
        let saved = serde_json::to_value(&settings).unwrap();
        let modules = &saved["modules"];
        assert_eq!(modules["spectrum"]["enabled"], false);
        assert_eq!(modules["spectrum"]["config"]["fft_size"], 2048);
        assert_eq!(
            modules["spectrum"]["config"]["floor_db"],
            SpectrumSettings::default().floor_db.get()
        );
        assert!(modules["spectrum"]["config"].get("future_option").is_none());
        assert_eq!(modules["spectrogram"]["config"]["source"], "side");
        assert_eq!(modules["waveform"]["enabled"], true);
        assert_eq!(modules["waveform"]["config"]["scroll_speed"], 72.0);
        assert_eq!(modules["loudness"]["enabled"], true);
        assert_eq!(modules["loudness"]["config"]["left_mode"], "true_peak");
        assert_eq!(modules["spectrum"]["config"]["rotation"], 0);
        let loaded = VisualSettings::from_value_lossy(saved.clone());
        assert_eq!(serde_json::to_value(loaded).unwrap(), saved);

        for (rotation, expected) in [(json!(-2), -2), (json!(1.5), 0), (json!(300), 0)] {
            let settings = VisualSettings::from_value_lossy(json!({"modules": {
                "spectrum": {"enabled": true, "config": {"rotation": rotation, "fft_size": 2048}}
            }}));
            let VisualConfig::Spectrum(config) = settings.config(VisualKind::Spectrum) else {
                panic!("expected spectrum settings");
            };
            assert_eq!((config.rotation, config.fft_size.get()), (expected, 2048));
        }
    }
}
