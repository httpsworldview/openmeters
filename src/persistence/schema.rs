// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo
use super::{lossy, palette::ColorSetting, visuals::VisualSettings};
use crate::domain::routing::{CaptureConfig, CaptureMode, DeviceSelection, StreamIdentity};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, time::Duration};

const MAIN_WINDOW_DEFAULT_WIDTH: u32 = 420;
const MAIN_WINDOW_DEFAULT_HEIGHT: u32 = 520;

pub const BAR_MIN_HEIGHT: u32 = 24;
pub const BAR_MAX_HEIGHT: u32 = 800;
pub const BAR_DEFAULT_HEIGHT: u32 = 180;

pub fn clamp_bar_height(height: u32) -> u32 {
    height.clamp(BAR_MIN_HEIGHT, BAR_MAX_HEIGHT)
}

crate::macros::choice_enum!(all pub enum BarAlignment { #[default] Top => "Top", Bottom => "Bottom" });
crate::macros::choice_enum!(all pub enum VisualFrameRate {
    Fps30 => "30 FPS",
    #[default] Fps60 => "60 FPS",
    Fps120 => "120 FPS",
    Display => "Match main display",
});

impl VisualFrameRate {
    pub const fn interval(self) -> Option<Duration> {
        let fps = match self {
            Self::Fps30 => 30,
            Self::Fps60 => 60,
            Self::Fps120 => 120,
            Self::Display => return None,
        };
        Some(Duration::from_nanos(1_000_000_000_u64.div_ceil(fps)))
    }
}

crate::macros::default_struct! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(default)]
    pub struct MainWindowSettings {
        pub width: u32 = MAIN_WINDOW_DEFAULT_WIDTH,
        pub height: u32 = MAIN_WINDOW_DEFAULT_HEIGHT,
    }
}

crate::macros::default_struct! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(default)]
    pub struct BarSettings {
        pub enabled: bool = false,
        pub alignment: BarAlignment = BarAlignment::default(),
        pub height: u32 = BAR_DEFAULT_HEIGHT,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub monitor: Option<String> = None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct UiSettings {
    pub visuals: VisualSettings,
    pub visual_frame_rate: VisualFrameRate,
    #[serde(skip_serializing)]
    pub background_color: Option<ColorSetting>,
    pub decorations: bool,
    pub main_window: MainWindowSettings,
    pub bar: BarSettings,
    pub capture_mode: CaptureMode,
    pub last_device_name: Option<String>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub disabled_streams: BTreeSet<StreamIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
}

impl UiSettings {
    pub(crate) fn capture_config(&self) -> CaptureConfig {
        CaptureConfig {
            mode: self.capture_mode,
            device: DeviceSelection::from_token(self.last_device_name.as_deref()),
            disabled_streams: self.disabled_streams.iter().cloned().collect(),
        }
    }

    pub(super) fn from_json_lossy(raw: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(raw).map(Self::from_value_lossy)
    }

    fn from_value_lossy(value: serde_json::Value) -> Self {
        lossy::settings(value, "settings", Self::default(), |map, out| {
            if let Some(value) = map.remove("visuals") {
                out.visuals = VisualSettings::from_value_lossy(value);
            }
            if let Some(value) = map.remove("main_window") {
                out.main_window = lossy::settings(
                    value,
                    "main_window",
                    MainWindowSettings::default(),
                    |map, out| {
                        lossy::fields!(map, out, "main_window"; width, height);
                    },
                );
            }
            if let Some(value) = map.remove("bar") {
                out.bar = lossy::settings(value, "bar", BarSettings::default(), |map, out| {
                    lossy::fields!(map, out, "bar"; enabled, alignment, height, monitor);
                });
            }
            lossy::fields!(map, out, "settings";
                visual_frame_rate, background_color, decorations, capture_mode, last_device_name,
                disabled_streams, theme
            );
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::visuals::{PopoutWindowSettings, SpectrumSettings};
    use super::*;
    use crate::domain::visuals::VisualKind;

    #[test]
    fn visual_frame_rate_defaults_to_60_fps() {
        let default = UiSettings::from_json_lossy("{}").unwrap().visual_frame_rate;
        let display = UiSettings::from_json_lossy(r#"{"visual_frame_rate":"display"}"#)
            .unwrap()
            .visual_frame_rate;
        assert_eq!(default, VisualFrameRate::Fps60);
        assert_eq!(display, VisualFrameRate::Display);
        assert_eq!(display.label(), "Match main display");
        assert_eq!(default.interval(), Some(Duration::from_nanos(16_666_667)));
        assert_eq!(display.interval(), None);
    }

    #[test]
    fn persisted_container_defaults_are_stable() {
        let main = MainWindowSettings::default();
        assert_eq!((main.width, main.height), (420, 520));

        let bar = BarSettings::default();
        assert_eq!(
            (bar.enabled, bar.alignment, bar.height, bar.monitor),
            (false, BarAlignment::Top, 180, None)
        );

        let popout = PopoutWindowSettings::default();
        assert_eq!(
            (popout.width, popout.height, popout.popped_out),
            (0, 0, true)
        );
    }

    #[test]
    fn popout_json_omits_default_active_state() {
        let mut settings = UiSettings::default();
        settings.visuals.popouts.insert(
            VisualKind::Spectrum,
            PopoutWindowSettings {
                width: 640,
                height: 360,
                popped_out: true,
            },
        );
        settings.visuals.popouts.insert(
            VisualKind::Waveform,
            PopoutWindowSettings {
                width: 320,
                height: 200,
                popped_out: false,
            },
        );
        settings
            .disabled_streams
            .insert(StreamIdentity::new("app.id"));

        let value = serde_json::to_value(&settings).unwrap();
        let popouts = &value["visuals"]["popouts"];
        assert!(popouts["spectrum"].get("popped_out").is_none());
        assert_eq!(popouts["waveform"]["popped_out"], false);
        assert_eq!(value["disabled_streams"], serde_json::json!(["app.id"]));
    }

    #[test]
    fn lossy_value_ignores_invalid_fields_at_their_scope() {
        let settings = UiSettings::from_value_lossy(serde_json::json!({
            "decorations": true,
            "visual_frame_rate": "not_a_rate",
            "capture_mode": "not_a_mode",
            "main_window": {
                "width": 640,
                "height": "tall",
            },
            "bar": {
                "enabled": true,
                "alignment": "bottom",
                "height": "tall",
                "monitor": "HDMI-A-1",
            },
            "visuals": {
                "modules": {
                    "spectrum": {
                        "enabled": true,
                        "config": {
                            "fft_size": 2048,
                            "floor_db": "quiet",
                            "show_grid": false,
                        },
                    },
                    "made_up": { "enabled": true },
                },
                "order": ["spectrum", "made_up", 4],
                "width_basis": {
                    "spectrum": 320.0,
                    "waveform": "wide",
                    "loudness": -1.0,
                    "made_up": 1.0,
                },
                "popouts": {
                    "spectrum": { "width": 640, "height": "tall" },
                    "oscilloscope": { "width": 300, "height": 200, "popped_out": false },
                    "waveform": "wide",
                    "made_up": { "width": 300, "height": 200 }
                },
            },
        }));

        assert!(settings.decorations);
        assert_eq!(settings.visual_frame_rate, VisualFrameRate::Fps60);
        assert_eq!(settings.capture_mode, CaptureMode::default());
        assert_eq!(settings.main_window.width, 640);
        assert_eq!(settings.main_window.height, MAIN_WINDOW_DEFAULT_HEIGHT);
        assert!(settings.bar.enabled);
        assert_eq!(settings.bar.alignment, BarAlignment::Bottom);
        assert_eq!(settings.bar.height, BAR_DEFAULT_HEIGHT);
        assert_eq!(settings.bar.monitor.as_deref(), Some("HDMI-A-1"));

        assert_eq!(settings.visuals.order, vec![VisualKind::Spectrum]);
        assert_eq!(settings.visuals.width_basis.len(), 1);
        assert_eq!(settings.visuals.width_basis[&VisualKind::Spectrum], 320.0);
        assert_eq!(settings.visuals.popouts.len(), 2);
        assert_eq!(settings.visuals.popouts[&VisualKind::Spectrum].width, 640);
        assert_eq!(settings.visuals.popouts[&VisualKind::Spectrum].height, 0);
        assert!(settings.visuals.popouts[&VisualKind::Spectrum].popped_out);
        assert_eq!(
            settings.visuals.popouts[&VisualKind::Oscilloscope].width,
            300
        );
        assert!(!settings.visuals.popouts[&VisualKind::Oscilloscope].popped_out);

        assert_eq!(settings.visuals.modules.len(), 1);
        let module = settings.visuals.modules.get(&VisualKind::Spectrum).unwrap();
        assert_eq!(module.enabled, Some(true));

        let spectrum = module.parse_config::<SpectrumSettings>().unwrap();
        assert_eq!(
            (spectrum.fft_size, spectrum.floor_db, spectrum.show_grid),
            (2048, SpectrumSettings::default().floor_db, false)
        );
    }
}
