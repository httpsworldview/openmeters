use crate::ui::app::config::CaptureMode;
use serde::{Deserialize, Serialize};

use super::{bar::BarSettings, palette::ColorSetting, visuals::VisualSettings};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct UiSettings {
    pub visuals: VisualSettings,
    pub background_color: Option<ColorSetting>,
    pub decorations: bool,
    pub bar: BarSettings,
    pub capture_mode: CaptureMode,
    pub last_device_name: Option<String>,
}
