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
        lossy::value(value, scope).map(|stored: StoredPalette| {
            Self::fit(
                stored.stops,
                stored.stop_positions.as_deref(),
                stored.stop_spreads.as_deref(),
                palette,
            )
        })
    }

    // Whether the colors, stop positions and stop spreads each differ from the defaults.
    fn differences(
        palette: &Palette,
        colors: &[Color],
        positions: &[f32],
        spreads: &[f32],
    ) -> [bool; 3] {
        [
            !palettes_equal(colors, palette.defaults),
            positions
                .iter()
                .zip(palette.default_positions)
                .any(|(a, b)| (a - b).abs() > EPSILON),
            spreads.iter().any(|s| (*s - 1.0).abs() > EPSILON),
        ]
    }

    // None while everything matches the defaults.
    pub fn from_state(
        palette: &Palette,
        colors: &[Color],
        positions: &[f32],
        spreads: &[f32],
    ) -> Option<Self> {
        let [colors_differ, positions_differ, spreads_differ] =
            Self::differences(palette, colors, positions, spreads);
        (colors_differ || positions_differ || spreads_differ).then(|| {
            let stops = if colors_differ {
                colors.iter().copied().map(Into::into).collect()
            } else {
                Vec::new()
            };
            Self::fit(
                stops,
                positions_differ.then_some(positions),
                spreads_differ.then_some(spreads),
                palette,
            )
        })
    }

    fn fit(
        mut stops: Vec<ColorSetting>,
        positions: Option<&[f32]>,
        spreads: Option<&[f32]>,
        palette: &Palette,
    ) -> Self {
        if let Some(&last) = stops.last() {
            stops.resize(palette.defaults.len(), last);
        }
        let (ramp, count) = (palette.default_positions, palette.default_positions.len());
        Self(StoredPalette {
            stops,
            stop_positions: positions.filter(|_| count > 2).map(|raw| {
                let mut interior = sanitize_stop_positions(raw, ramp);
                interior.pop();
                interior.remove(0);
                interior
            }),
            stop_spreads: spreads
                .filter(|_| count > 0)
                .map(|raw| sanitize_stop_spreads(raw, count)),
        })
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

    /// Fills `out`, which is as long as `defaults`.
    pub fn write_positions(palette: Option<&Self>, defaults: &[f32], out: &mut [f32]) {
        match palette.and_then(|Self(stored)| stored.stop_positions.as_deref()) {
            Some(interior) => {
                let end = out.len() - 1;
                out[0] = 0.0;
                out[1..end].copy_from_slice(interior);
                out[end] = 1.0;
            }
            None => out.copy_from_slice(defaults),
        }
    }

    /// Fills `out`, which holds one spread per stop.
    pub fn write_spreads(palette: Option<&Self>, out: &mut [f32]) {
        match palette.and_then(|Self(stored)| stored.stop_spreads.as_deref()) {
            Some(spreads) => out.copy_from_slice(spreads),
            None => out.fill(1.0),
        }
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
        let (mut positions, mut spreads) = ([f32::NAN; 4], [f32::NAN; 4]);
        PaletteSettings::write_positions(Some(&decoded), &RAMP, &mut positions);
        PaletteSettings::write_spreads(Some(&decoded), &mut spreads);
        assert_eq!(positions, [0.0, 0.25, 0.5, 1.0]);
        assert_eq!(spreads, [2.0, 1.0, 1.0, 1.0]);
    }
}
