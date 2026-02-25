use serde::{Deserialize, Serialize};

pub const BAR_MIN_HEIGHT: u32 = 24;
pub const BAR_MAX_HEIGHT: u32 = 800;
pub const BAR_DEFAULT_HEIGHT: u32 = 180;

pub fn clamp_bar_height(height: u32) -> u32 {
    height.clamp(BAR_MIN_HEIGHT, BAR_MAX_HEIGHT)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum BarAlignment {
    #[default]
    Top,
    Bottom,
}

impl std::fmt::Display for BarAlignment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Top => "Top",
            Self::Bottom => "Bottom",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BarSettings {
    pub enabled: bool,
    pub alignment: BarAlignment,
    pub height: u32,
}

impl Default for BarSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            alignment: BarAlignment::default(),
            height: BAR_DEFAULT_HEIGHT,
        }
    }
}
