// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::palette::{ColorSetting, PaletteSettings};
use crate::domain::visuals::VisualKind;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::{fs, io};

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
}

impl std::fmt::Display for ThemeChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.name == BUILTIN_THEME {
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
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub palettes: BTreeMap<VisualKind, PaletteSettings>,
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
        }];
        if let Ok(entries) = fs::read_dir(&self.dir) {
            choices.extend(entries.flatten().filter_map(|entry| {
                let path = entry.path();
                let stem = path.file_stem()?.to_str()?;
                (path.extension().is_some_and(|e| e == "json") && stem != BUILTIN_THEME).then(
                    || ThemeChoice {
                        name: stem.to_owned(),
                    },
                )
            }));
        }
        choices.sort_by_cached_key(|choice| {
            (choice.name != BUILTIN_THEME, choice.name.to_lowercase())
        });
        choices
    }

    pub fn load(&self, name: &str) -> io::Result<ThemeFile> {
        if name == BUILTIN_THEME {
            return Ok(ThemeFile::default());
        }
        let content = fs::read_to_string(self.theme_path(name))?;
        Ok(serde_json::from_str(&content)?)
    }

    pub fn save(&self, name: &str, theme: &ThemeFile) -> io::Result<()> {
        let path = self.theme_path(name);
        let json = serde_json::to_string_pretty(theme)?;
        super::write_json_atomic(&path, &json)
    }

    pub fn update(&self, name: &str, mutate: impl FnOnce(&mut ThemeFile)) -> io::Result<()> {
        if name == BUILTIN_THEME {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "cannot modify built-in theme",
            ));
        }
        let mut theme = self.load(name)?;
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
            palettes: BTreeMap::from([(
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

        store
            .update("test", |theme| theme.author = Some("Author".into()))
            .unwrap();
        let updated = store.load("test").unwrap();
        assert_eq!(updated.name, loaded.name);
        assert_eq!(updated.palettes, loaded.palettes);
        assert_eq!(updated.author.as_deref(), Some("Author"));
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
                .map(|choice| choice.name.as_str())
                .collect::<Vec<_>>(),
            ["default", "alpha", "zebra"]
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
        assert!(store.list().iter().any(|choice| choice.name == name));
        Ok(())
    }

    #[test]
    fn updates_require_a_readable_custom_theme() {
        use io::ErrorKind::{InvalidData, NotFound, PermissionDenied, UnexpectedEof};

        let dir = tempfile::tempdir().unwrap();
        let store = ThemeStore::new(dir.path());
        fs::create_dir_all(&store.dir).unwrap();
        for (name, content, kind) in [
            ("default", None, PermissionDenied),
            ("missing", None, NotFound),
            ("empty", Some(b"".as_slice()), UnexpectedEof),
            ("json", Some(b"!"), InvalidData),
            ("utf8", Some(b"\xff"), InvalidData),
        ] {
            let path = store.theme_path(name);
            if let Some(content) = content {
                fs::write(&path, content).unwrap();
            }
            let err = store
                .update(name, |_| panic!("unreadable theme"))
                .unwrap_err();
            assert_eq!(err.kind(), kind);
            assert_eq!(path.exists(), content.is_some());
            assert_eq!(fs::read(path).ok().as_deref(), content);
        }
    }
}
