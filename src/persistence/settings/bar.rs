// OpenMeters - an audio analysis and visualization tool
// Copyright (C) 2026  Maika Namuo
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

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
