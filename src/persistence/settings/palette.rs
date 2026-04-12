// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::util::color::{
    EPSILON, colors_equal, sanitize_stop_positions, sanitize_stop_spreads, uniform_positions,
};
use iced::Color;
use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use std::array;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorSetting(Color);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyRgba {
    r: f32,
    g: f32,
    b: f32,
    a: f32,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ColorSettingRepr {
    Hex(String),
    Legacy(LegacyRgba),
}

impl Serialize for ColorSetting {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for ColorSetting {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match ColorSettingRepr::deserialize(deserializer)? {
            ColorSettingRepr::Hex(value) => value
                .parse::<Color>()
                .map(ColorSetting)
                .map_err(de::Error::custom),
            ColorSettingRepr::Legacy(LegacyRgba { r, g, b, a }) => {
                Ok(ColorSetting(Color::from_rgba(r, g, b, a)))
            }
        }
    }
}

impl From<Color> for ColorSetting {
    fn from(color: Color) -> Self {
        Self(color)
    }
}
impl From<ColorSetting> for Color {
    fn from(ColorSetting(color): ColorSetting) -> Self {
        color
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PaletteSettings {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stops: Vec<ColorSetting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_positions: Option<Vec<f32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_spreads: Option<Vec<f32>>,
}

impl PaletteSettings {
    pub fn to_array<const N: usize>(&self) -> Option<[Color; N]> {
        (self.stops.len() == N).then(|| array::from_fn(|idx| self.stops[idx].into()))
    }
    // Returns `Some` only if colors differ from defaults (avoids persisting unchanged palettes).
    pub fn if_differs_from(colors: &[Color], defaults: &[Color]) -> Option<Self> {
        let differs = colors_differ(colors, defaults);
        differs.then_some(Self {
            stops: colors.iter().copied().map(Into::into).collect(),
            stop_positions: None,
            stop_spreads: None,
        })
    }

    pub fn from_state(
        colors: &[Color],
        defaults: &[Color],
        positions: &[f32],
        spreads: &[f32],
    ) -> Option<Self> {
        let count = defaults.len();
        let colors_differ = colors_differ(colors, defaults);
        let sanitized_positions = sanitize_stop_positions(Some(positions), count);
        let positions_differ = sanitized_positions
            .iter()
            .zip(uniform_positions(count).iter())
            .any(|(a, b)| (a - b).abs() > EPSILON);
        let sanitized_spreads = sanitize_stop_spreads(Some(spreads), count);
        let spreads_differ = sanitized_spreads.iter().any(|s| (*s - 1.0).abs() > EPSILON);

        let stops = if colors_differ {
            colors.iter().copied().map(Into::into).collect()
        } else {
            Vec::new()
        };
        (colors_differ || positions_differ || spreads_differ).then_some(Self {
            stops,
            stop_positions: (positions_differ && count > 2)
                .then(|| sanitized_positions[1..count - 1].to_vec()),
            stop_spreads: spreads_differ.then_some(sanitized_spreads),
        })
    }
}

fn colors_differ(colors: &[Color], defaults: &[Color]) -> bool {
    colors.len() == defaults.len()
        && colors
            .iter()
            .zip(defaults)
            .any(|(c, d)| !colors_equal(*c, *d))
}

pub trait HasPalette {
    fn palette(&self) -> Option<&PaletteSettings>;
    fn set_palette(&mut self, palette: Option<PaletteSettings>);
}
