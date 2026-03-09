// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo
use crate::domain::routing::CaptureMode;
use serde::{Deserialize, Serialize};

use super::{bar::BarSettings, palette::ColorSetting, visuals::VisualSettings};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct UiSettings {
    pub visuals: VisualSettings,
    #[serde(skip_serializing)]
    pub background_color: Option<ColorSetting>,
    pub decorations: bool,
    pub bar: BarSettings,
    pub capture_mode: CaptureMode,
    pub last_device_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
}
