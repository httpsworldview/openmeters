// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::palette::{ColorSetting, PaletteSettings};
use crate::domain::visuals::VisualKind;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::{fs, io};
use tracing::warn;

use serde::{Deserialize, Serialize};

const THEMES_DIR: &str = "themes";
pub const BUILTIN_THEME: &str = "default";

#[derive(Debug, Clone, Eq)]
pub struct ThemeChoice {
    pub name: String,
    pub builtin: bool,
}

impl PartialEq for ThemeChoice {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl std::fmt::Display for ThemeChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.builtin {
            write!(f, "{} (built-in)", self.name)
        } else {
            f.write_str(&self.name)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ThemeFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<ColorSetting>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub palettes: HashMap<VisualKind, PaletteSettings>,
}

#[derive(Debug)]
pub struct ThemeStore {
    dir: PathBuf,
}

impl ThemeStore {
    pub fn new(config_dir: &Path) -> Self {
        Self {
            dir: config_dir.join(THEMES_DIR),
        }
    }

    fn ensure_dir(&self) -> io::Result<()> {
        fs::create_dir_all(&self.dir)
    }

    pub fn list(&self) -> Vec<ThemeChoice> {
        let mut choices = vec![ThemeChoice {
            name: BUILTIN_THEME.to_owned(),
            builtin: true,
        }];
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "json")
                    && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                    && stem != BUILTIN_THEME
                {
                    choices.push(ThemeChoice {
                        name: stem.to_owned(),
                        builtin: false,
                    });
                }
            }
        }
        choices.sort_by(|a, b| match (a.builtin, b.builtin) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });
        choices
    }

    pub fn load(&self, name: &str) -> Option<ThemeFile> {
        if name == BUILTIN_THEME {
            return Some(ThemeFile::default());
        }
        let path = self.theme_path(name);
        let content = fs::read_to_string(&path)
            .inspect_err(|e| warn!("[theme] failed to read {path:?}: {e}"))
            .ok()?;
        serde_json::from_str(&content)
            .inspect_err(|e| warn!("[theme] parse error in {path:?}: {e}"))
            .ok()
    }

    pub fn save(&self, name: &str, theme: &ThemeFile) -> io::Result<()> {
        self.ensure_dir()?;
        let path = self.theme_path(name);
        let json = serde_json::to_string_pretty(theme)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let temp = path.with_extension("json.tmp");
        fs::write(&temp, json)?;
        fs::rename(&temp, &path)
    }

    pub fn update(&self, name: &str, mutate: impl FnOnce(&mut ThemeFile)) -> io::Result<()> {
        if name == BUILTIN_THEME {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "cannot modify built-in theme",
            ));
        }
        let mut theme = self.load(name).unwrap_or_default();
        mutate(&mut theme);
        self.save(name, &theme)
    }

    fn theme_path(&self, name: &str) -> PathBuf {
        let safe = name.replace(['/', '\\', '\0'], "");
        self.dir.join(format!("{safe}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::Color;
    use std::fs;

    #[test]
    fn roundtrip_partial_theme() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThemeStore::new(dir.path());

        let theme = ThemeFile {
            name: Some("Test".into()),
            palettes: HashMap::from([(
                VisualKind::Spectrum,
                PaletteSettings {
                    stops: vec![Color::WHITE.into(), Color::BLACK.into()],
                    ..Default::default()
                },
            )]),
            ..Default::default()
        };

        store.save("test", &theme).unwrap();
        let loaded = store.load("test").unwrap();
        assert_eq!(loaded.name.as_deref(), Some("Test"));
        assert!(loaded.palettes.contains_key(&VisualKind::Spectrum));
        // missing visuals get None
        assert!(!loaded.palettes.contains_key(&VisualKind::Oscilloscope));
    }

    #[test]
    fn list_sorted_with_default_first() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThemeStore::new(dir.path());
        let themes_dir = dir.path().join(THEMES_DIR);
        fs::create_dir_all(&themes_dir).unwrap();
        fs::write(themes_dir.join("zebra.json"), "{}").unwrap();
        fs::write(themes_dir.join("alpha.json"), "{}").unwrap();

        let names = store.list();
        let mk = |n: &str, b| ThemeChoice {
            name: n.to_owned(),
            builtin: b,
        };
        assert_eq!(
            names,
            vec![mk("default", true), mk("alpha", false), mk("zebra", false)]
        );
    }

    #[test]
    fn update_builtin_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThemeStore::new(dir.path());
        let err = store.update("default", |_| {}).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }
}
