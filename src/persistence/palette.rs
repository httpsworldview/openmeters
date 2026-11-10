// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::lossy;
use crate::util::color::{
    EPSILON, palettes_equal, parse_hex_color, sanitize_stop_positions, sanitize_stop_spreads,
    to_hex,
};
use crate::visuals::palettes::Palette;
use iced::Color;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ColorSetting(Color);

impl TryFrom<String> for ColorSetting {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        parse_hex_color(&value).map(Self::from)
    }
}

impl From<ColorSetting> for String {
    fn from(ColorSetting(color): ColorSetting) -> Self {
        to_hex(color)
    }
}

// Files store 8-bit channels; holding only those keeps themes equal to their files.
impl From<Color> for ColorSetting {
    fn from(color: Color) -> Self {
        let [r, g, b, a] = color.into_rgba8();
        Self(Color::from_rgba8(r, g, b, f32::from(a) / 255.0))
    }
}
impl From<ColorSetting> for Color {
    fn from(ColorSetting(color): ColorSetting) -> Self {
        color
    }
}

// The theme-file form. Only `PaletteSettings` constructors read it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StoredPalette {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    stops: Vec<ColorSetting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stop_positions: Option<Vec<f32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stop_spreads: Option<Vec<f32>>,
}

// Always fitted to its visual's palette: stops are empty or complete, and a
// ramp has its interior positions and one spread per stop.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct PaletteSettings(StoredPalette);

impl PaletteSettings {
    pub(super) fn decode(value: Value, palette: &Palette, scope: &str) -> Option<Self> {
        lossy::value(value, scope).map(|stored| Self::fit(stored, palette))
    }

    // None while everything matches the defaults.
    pub fn from_state(
        palette: &Palette,
        colors: &[Color],
        positions: &[f32],
        spreads: &[f32],
    ) -> Option<Self> {
        let stops = (!palettes_equal(colors, palette.defaults))
            .then(|| colors.iter().copied().map(Into::into).collect());
        let positions_differ = positions
            .iter()
            .zip(palette.default_positions)
            .any(|(a, b)| (a - b).abs() > EPSILON);
        let spreads_differ = spreads.iter().any(|s| (*s - 1.0).abs() > EPSILON);

        (stops.is_some() || positions_differ || spreads_differ).then(|| {
            let stored = StoredPalette {
                stops: stops.unwrap_or_default(),
                stop_positions: positions_differ.then(|| positions.to_vec()),
                stop_spreads: spreads_differ.then(|| spreads.to_vec()),
            };
            Self::fit(stored, palette)
        })
    }

    fn fit(mut stored: StoredPalette, palette: &Palette) -> Self {
        if let Some(&last) = stored.stops.last() {
            stored.stops.resize(palette.defaults.len(), last);
        }
        let (ramp, count) = (palette.default_positions, palette.default_positions.len());
        stored.stop_positions = stored
            .stop_positions
            .filter(|_| count > 2)
            .map(|raw| sanitize_stop_positions(&raw, ramp)[1..count - 1].to_vec());
        stored.stop_spreads = stored
            .stop_spreads
            .filter(|_| count > 0)
            .map(|raw| sanitize_stop_spreads(&raw, count));
        Self(stored)
    }

    pub fn stops(&self) -> impl Iterator<Item = Color> + '_ {
        self.0.stops.iter().map(|&stop| stop.into())
    }

    pub fn colors<const N: usize>(palette: Option<&Self>, defaults: &[Color; N]) -> [Color; N] {
        match palette {
            Some(Self(stored)) if !stored.stops.is_empty() => {
                std::array::from_fn(|index| stored.stops[index].into())
            }
            _ => *defaults,
        }
    }

    pub fn positions(palette: Option<&Self>, defaults: &[f32]) -> Vec<f32> {
        match palette.and_then(|Self(stored)| stored.stop_positions.as_deref()) {
            Some(interior) => [&[0.0], interior, &[1.0]].concat(),
            None => defaults.to_vec(),
        }
    }

    pub fn spreads(palette: Option<&Self>, count: usize) -> Vec<f32> {
        let spreads = palette.and_then(|Self(stored)| stored.stop_spreads.clone());
        spreads.unwrap_or_else(|| vec![1.0; count])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_palettes_fit_the_visual_palette() {
        const DEFAULTS: [Color; 4] = [Color::BLACK; 4];
        const RAMP: [f32; 4] = [0.0, 0.25, 0.75, 1.0];
        let palette = Palette::new(&DEFAULTS, &RAMP, &[]);
        let stops = [1, 2, 3, 4, 5].map(|value| Color::from_rgb8(value, value, value));

        assert_eq!(PaletteSettings::colors(None, &DEFAULTS), DEFAULTS);
        for (len, expected) in [
            (0, DEFAULTS),
            (1, [stops[0]; 4]),
            (2, [stops[0], stops[1], stops[1], stops[1]]),
            (5, [stops[0], stops[1], stops[2], stops[3]]),
        ] {
            let hex: Vec<_> = stops[..len].iter().copied().map(to_hex).collect();
            let value = serde_json::json!({ "stops": hex });
            let decoded = PaletteSettings::decode(value, &palette, "palette").unwrap();
            assert_eq!(PaletteSettings::colors(Some(&decoded), &DEFAULTS), expected);
        }

        // Numbers too large for f32 fall back one entry at a time.
        let value = serde_json::json!({
            "stops": [to_hex(stops[0])],
            "stop_positions": [1e39, 0.5],
            "stop_spreads": [2.0, 1e39, 1.0, 1.0],
        });
        let decoded = PaletteSettings::decode(value, &palette, "palette").unwrap();
        assert_eq!(
            PaletteSettings::colors(Some(&decoded), &DEFAULTS),
            [stops[0]; 4]
        );
        assert_eq!(
            PaletteSettings::positions(Some(&decoded), &RAMP),
            [0.0, 0.25, 0.5, 1.0]
        );
        assert_eq!(
            PaletteSettings::spreads(Some(&decoded), 4),
            [2.0, 1.0, 1.0, 1.0]
        );
    }
}
