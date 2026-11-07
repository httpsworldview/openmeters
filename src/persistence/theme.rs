// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::document::Stored;
use super::palette::{ColorSetting, PaletteSettings};
use super::{lossy, visuals::visual_map};
use crate::domain::visuals::VisualKind;
use crate::visuals::palettes::Palette;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const THEMES_DIR: &str = "themes";
const AUTO_THEME_BASE: &str = "default-custom";
pub const BUILTIN_THEME: &str = "default";

/// The theme file a name refers to, or `None` for the built-in theme.
pub fn theme_name(name: &str) -> Option<String> {
    let name = name.replace(['/', '\\', '\0'], "");
    let name = name.trim();
    (!name.is_empty() && name != BUILTIN_THEME).then(|| name.to_owned())
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

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
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

impl Stored for ThemeFile {
    fn from_json_lossy(raw: &[u8]) -> serde_json::Result<Self> {
        serde_json::from_slice(raw).map(|value| {
            lossy::settings::<Self>(value, "theme", |map, out| {
                lossy::fields!(map, out, "theme"; name, author, background);
                visual_map(map, "palettes", &mut out.palettes, |kind, value, scope| {
                    PaletteSettings::decode(value, &Palette::for_kind(kind), scope)
                });
            })
        })
    }
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

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn list(&self) -> Vec<ThemeChoice> {
        let mut choices = vec![ThemeChoice {
            name: BUILTIN_THEME.to_owned(),
        }];
        if let Ok(entries) = fs::read_dir(&self.dir) {
            choices.extend(entries.flatten().filter_map(|entry| {
                let path = entry.path();
                let stem = path.file_stem()?.to_str()?;
                let json = path.extension().is_some_and(|e| e == "json");
                (json && theme_name(stem).as_deref() == Some(stem)).then(|| ThemeChoice {
                    name: stem.to_owned(),
                })
            }));
        }
        choices.sort_by_cached_key(|choice| {
            (choice.name != BUILTIN_THEME, choice.name.to_lowercase())
        });
        choices
    }

    pub(super) fn path(&self, name: &str) -> PathBuf {
        debug_assert_eq!(
            theme_name(name).as_deref(),
            Some(name),
            "unchecked theme name"
        );
        self.dir.join(format!("{name}.json"))
    }

    pub(super) fn next_auto_name(&self) -> String {
        std::iter::once(AUTO_THEME_BASE.to_owned())
            .chain((2_u64..).map(|i| format!("{AUTO_THEME_BASE}-{i}")))
            .find(|name| fs::symlink_metadata(self.path(name)).is_err())
            .expect("auto theme names are unbounded")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::Color;

    #[test]
    fn partial_themes_survive_saving() {
        let theme = ThemeFile {
            name: Some("Test".into()),
            palettes: BTreeMap::from([(
                VisualKind::Spectrum,
                PaletteSettings::from_state(
                    &Palette::for_kind(VisualKind::Spectrum),
                    &[Color::WHITE, Color::BLACK],
                    &[],
                    &[],
                )
                .unwrap(),
            )]),
            ..Default::default()
        };
        let saved = serde_json::to_vec_pretty(&theme).unwrap();
        assert_eq!(ThemeFile::from_json_lossy(&saved).unwrap(), theme);
    }

    #[test]
    fn list_sorted_with_default_first_and_only_checked_names() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThemeStore::new(dir.path());
        fs::create_dir_all(&store.dir).unwrap();
        for file in [
            "zebra.json",
            "alpha.json",
            " padded.json",
            "default.json",
            ".json",
        ] {
            fs::write(store.dir.join(file), "{}").unwrap();
        }

        let names = store.list().into_iter().map(|choice| choice.name);
        assert_eq!(names.collect::<Vec<_>>(), ["default", "alpha", "zebra"]);
    }

    #[test]
    fn names_are_stripped_then_trimmed_and_never_built_in() {
        assert_eq!(
            theme_name(" custom/theme\\name\0 ").as_deref(),
            Some("customthemename")
        );
        for built_in in ["", " ", "/", "default", "default/", " default "] {
            assert_eq!(theme_name(built_in), None, "{built_in:?}");
        }
    }
}
