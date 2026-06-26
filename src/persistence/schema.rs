// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo
use super::{lossy, palette::ColorSetting, visuals::VisualSettings};
use crate::domain::routing::CaptureMode;
use serde::{Deserialize, Serialize};

const MAIN_WINDOW_DEFAULT_WIDTH: u32 = 420;
const MAIN_WINDOW_DEFAULT_HEIGHT: u32 = 520;

pub const BAR_MIN_HEIGHT: u32 = 24;
pub const BAR_MAX_HEIGHT: u32 = 800;
pub const BAR_DEFAULT_HEIGHT: u32 = 180;

pub fn clamp_bar_height(height: u32) -> u32 {
    height.clamp(BAR_MIN_HEIGHT, BAR_MAX_HEIGHT)
}

crate::macros::choice_enum!(all pub enum BarAlignment { #[default] Top => "Top", Bottom => "Bottom" });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MainWindowSettings {
    pub width: u32,
    pub height: u32,
}

impl Default for MainWindowSettings {
    fn default() -> Self {
        Self {
            width: MAIN_WINDOW_DEFAULT_WIDTH,
            height: MAIN_WINDOW_DEFAULT_HEIGHT,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BarSettings {
    pub enabled: bool,
    pub alignment: BarAlignment,
    pub height: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor: Option<String>,
}

impl Default for BarSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            alignment: BarAlignment::default(),
            height: BAR_DEFAULT_HEIGHT,
            monitor: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct UiSettings {
    pub visuals: VisualSettings,
    #[serde(skip_serializing)]
    pub background_color: Option<ColorSetting>,
    pub decorations: bool,
    pub main_window: MainWindowSettings,
    pub bar: BarSettings,
    pub capture_mode: CaptureMode,
    pub last_device_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
}

impl UiSettings {
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
                background_color, decorations, capture_mode, last_device_name, theme
            );
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::visuals::SpectrumSettings;
    use super::*;
    use crate::domain::visuals::VisualKind;

    #[test]
    fn lossy_value_ignores_invalid_fields_at_their_scope() {
        let settings = UiSettings::from_value_lossy(serde_json::json!({
            "decorations": true,
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
            },
        }));

        assert!(settings.decorations);
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
