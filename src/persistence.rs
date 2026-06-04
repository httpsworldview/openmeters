// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

mod bar;
mod lossy;
mod palette;
mod schema;
mod store;
mod theme;
mod visuals;

use std::{fs, io, path::Path};

fn write_json_atomic(path: &Path, json: &str) -> io::Result<()> {
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, json)?;
    fs::rename(&temp, path)
}

pub mod settings {
    pub use super::bar::{
        BAR_MAX_HEIGHT, BAR_MIN_HEIGHT, BarAlignment, BarSettings, clamp_bar_height,
    };
    pub use super::palette::{HasPalette, PaletteSettings};
    pub use super::store::SettingsHandle;
    pub(crate) use super::theme::canonical_theme_name;
    pub use super::theme::{BUILTIN_THEME, ThemeChoice, ThemeFile, ThemeOrigin};
    pub(crate) use super::visuals::SettingsConfig;
    pub use super::visuals::{
        LoudnessSettings, ModuleSettings, OscilloscopeSettings, SpectrogramSettings,
        SpectrumSettings, StereometerSettings, VisualSettings, WaveformSettings,
    };
}
