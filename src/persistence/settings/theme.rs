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
const AUTO_THEME_BASE: &str = "default-custom";
pub const BUILTIN_THEME: &str = "default";

pub(crate) fn canonical_theme_name(name: &str) -> String {
    name.replace(['/', '\\', '\0'], "")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeChoice {
    pub name: String,
    pub builtin: bool,
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

pub struct ThemeStore {
    dir: PathBuf,
}

impl ThemeStore {
    pub fn new(config_dir: &Path) -> Self {
        Self {
            dir: config_dir.join(THEMES_DIR),
        }
    }

    pub fn list(&self) -> Vec<ThemeChoice> {
        let mut choices = vec![ThemeChoice {
            name: BUILTIN_THEME.to_owned(),
            builtin: true,
        }];
        if let Ok(entries) = fs::read_dir(&self.dir) {
            choices.extend(entries.flatten().filter_map(|entry| {
                let path = entry.path();
                let stem = path.file_stem()?.to_str()?;
                (path.extension().is_some_and(|e| e == "json") && stem != BUILTIN_THEME).then(
                    || ThemeChoice {
                        name: stem.to_owned(),
                        builtin: false,
                    },
                )
            }));
        }
        choices.sort_by_cached_key(|choice| (!choice.builtin, choice.name.to_lowercase()));
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
        fs::create_dir_all(&self.dir)?;
        let path = self.theme_path(name);
        let json = serde_json::to_string_pretty(theme)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        super::write_json_atomic(&path, &json)
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

    pub(super) fn next_auto_name(&self) -> String {
        let mut i = 1_u64;
        loop {
            let name = match i {
                1 => AUTO_THEME_BASE.to_owned(),
                _ => format!("{AUTO_THEME_BASE}-{i}"),
            };
            if !self.theme_path(&name).exists() {
                return name;
            }
            i += 1;
        }
    }

    fn theme_path(&self, name: &str) -> PathBuf {
        let safe = canonical_theme_name(name);
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
        assert_eq!(
            names
                .iter()
                .map(|choice| (choice.name.as_str(), choice.builtin))
                .collect::<Vec<_>>(),
            vec![("default", true), ("alpha", false), ("zebra", false)]
        );
    }

    #[test]
    fn canonical_names_match_saved_file_stems() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ThemeStore::new(dir.path());
        let raw = " custom/theme\\name\0 ";
        let name = canonical_theme_name(raw);

        assert_eq!(name, " customthemename ");
        store.save(raw, &ThemeFile::default())?;
        assert!(
            store
                .list()
                .iter()
                .any(|choice| choice.name == name && !choice.builtin)
        );
        Ok(())
    }

    #[test]
    fn update_builtin_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThemeStore::new(dir.path());
        let err = store.update("default", |_| {}).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }
}
