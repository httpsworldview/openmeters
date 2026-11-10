// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

mod document;
mod lossy;
mod palette;
mod schema;
mod store;
mod theme;
mod visuals;
mod watch;

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

/// Follows `path` through symlinks, even dangling ones: the links on the way, and
/// where they lead.
fn resolve_links(path: &Path) -> (Vec<PathBuf>, PathBuf) {
    let (mut links, mut target) = (Vec::new(), path.to_owned());
    while links.len() < 40
        && let Ok(link) = fs::read_link(&target)
    {
        let next = target.parent().unwrap_or_else(|| Path::new(".")).join(link);
        links.push(std::mem::replace(&mut target, next));
    }
    (links, target)
}

struct PendingWrite {
    temp: tempfile::NamedTempFile,
    target: PathBuf,
}

impl PendingWrite {
    fn new(path: &Path, json: &str) -> io::Result<Self> {
        // Write through symlinks so linked dotfiles stay linked.
        let (_, target) = resolve_links(path);
        let parent = target.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(json.as_bytes())?;
        temp.as_file().sync_all()?;
        Ok(Self { temp, target })
    }

    fn commit(self) -> io::Result<()> {
        self.temp
            .persist(&self.target)
            .map(|_| ())
            .map_err(|err| err.error)
    }
}

pub mod settings {
    pub use super::palette::PaletteSettings;
    pub use super::schema::{
        BarAlignment, BarHeight, MainWindowSettings, UiSettings, VisualFrameRate, clamp_bar_height,
    };
    pub use super::store::{SettingsHandle, Snapshot};
    pub use super::theme::{BUILTIN_THEME, ThemeChoice, ThemeFile, theme_name};
    pub use super::visuals::{
        LoudnessSettings, OscilloscopeSettings, PopoutWindowSettings, SpectrogramSettings,
        SpectrumSettings, StereometerSettings, VisualConfig, VisualSettings, WaveformSettings,
    };
    pub use super::watch::changes as settings_changes;
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
                        PendingWrite::new(path, json).unwrap().commit().unwrap();
                        assert!(documents.contains(&fs::read_to_string(path).unwrap()));
                    });
                }
            });
        }
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);

        let directory = path.parent().unwrap();
        assert!(
            PendingWrite::new(directory, "{}")
                .unwrap()
                .commit()
                .is_err()
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
