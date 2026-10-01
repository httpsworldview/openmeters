// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

mod lossy;
mod palette;
mod schema;
mod store;
mod theme;
mod visuals;

use std::{
    fs,
    io::{self, Write},
    path::Path,
};

fn write_json_atomic(path: &Path, json: &str) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(json.as_bytes())?;
    temp.persist(path).map(|_| ()).map_err(|err| err.error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Barrier, thread};

    #[test]
    fn concurrent_saves_keep_complete_files_and_clean_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/settings.json");
        let documents = ["a".repeat(4096), "b".repeat(8192)]
            .map(|value| serde_json::to_string(&value).unwrap());
        let barrier = Barrier::new(2);
        for _ in 0..32 {
            thread::scope(|scope| {
                for json in &documents {
                    let (path, barrier, documents) = (&path, &barrier, &documents);
                    scope.spawn(move || {
                        barrier.wait();
                        write_json_atomic(path, json).unwrap();
                        assert!(documents.contains(&fs::read_to_string(path).unwrap()));
                    });
                }
            });
        }
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);

        assert!(write_json_atomic(path.parent().unwrap(), "{}").is_err());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

pub mod settings {
    pub use super::palette::PaletteSettings;
    pub use super::schema::{
        BAR_MAX_HEIGHT, BAR_MIN_HEIGHT, BarAlignment, BarSettings, MainWindowSettings,
        VisualFrameRate, clamp_bar_height,
    };
    pub use super::store::{ChangeOrigin, SettingsHandle};
    pub(crate) use super::theme::canonical_theme_name;
    pub use super::theme::{BUILTIN_THEME, ThemeChoice, ThemeFile};
    pub use super::visuals::{
        LoudnessSettings, OscilloscopeSettings, PopoutWindowSettings, SpectrogramSettings,
        SpectrumSettings, StereometerSettings, VisualConfig, VisualSettings, WaveformSettings,
    };
}
