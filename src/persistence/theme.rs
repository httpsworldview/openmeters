// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::document::Stored;
use super::palette::{ColorSetting, PaletteSettings};
use super::{lossy, visuals::visual_map};
use crate::domain::visuals::VisualKind;
use crate::visuals::palettes::Palette;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const THEMES_DIR: &str = "themes";
const AUTO_THEME_BASE: &str = "default-custom";
pub const BUILTIN_THEME: &str = "default";

/// The stem of a theme file: never empty, the built-in theme's name, or a path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ThemeName(String);

impl ThemeName {
    /// The theme file `name` refers to, or `None` for the built-in theme.
    pub fn new(name: &str) -> Option<Self> {
        let name = name.replace(['/', '\\', '\0'], "");
        let name = name.trim();
        (!name.is_empty() && name != BUILTIN_THEME).then(|| Self(name.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ThemeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A theme to pick; `None` is the built-in one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeChoice(pub Option<ThemeName>);

impl std::fmt::Display for ThemeChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Some(name) => name.fmt(f),
            None => write!(f, "{BUILTIN_THEME} (built-in)"),
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
    const SCOPE: &'static str = "theme";

    fn read_fields(map: &mut Map<String, Value>, out: &mut Self, scope: &str) {
        lossy::fields!(map, out, scope; name, author, background);
        let palettes = &mut out.palettes;
        visual_map(map, scope, "palettes", palettes, |kind, value, scope| {
            PaletteSettings::decode(value, &Palette::for_kind(kind), scope)
        });
    }
}

pub(super) struct ThemeStore {
    dir: PathBuf,
}

impl ThemeStore {
    pub(super) fn new(config_dir: &Path) -> Self {
        Self {
            dir: config_dir.join(THEMES_DIR),
        }
    }

    pub(super) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(super) fn list(&self) -> Vec<ThemeChoice> {
        let mut choices = vec![ThemeChoice(None)];
        if let Ok(entries) = fs::read_dir(&self.dir) {
            choices.extend(entries.flatten().filter_map(|entry| {
                let path = entry.path();
                let stem = path.file_stem()?.to_str()?;
                let json = path.extension().is_some_and(|e| e == "json");
                // A file whose stem is not already a theme name could not be opened by it.
                let name = ThemeName::new(stem).filter(|name| json && name.0 == stem)?;
                Some(ThemeChoice(Some(name)))
            }));
        }
        choices.sort_by_cached_key(|ThemeChoice(name)| name.as_ref().map(|n| n.0.to_lowercase()));
        choices
    }

    pub(super) fn path(&self, name: &ThemeName) -> PathBuf {
        self.dir.join(format!("{name}.json"))
    }

    pub(super) fn next_auto_name(&self) -> ThemeName {
        std::iter::once(AUTO_THEME_BASE.to_owned())
            .chain((2_u64..).map(|i| format!("{AUTO_THEME_BASE}-{i}")))
            .map(ThemeName)
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

        let names = store.list().into_iter().map(|choice| choice.to_string());
        assert_eq!(
            names.collect::<Vec<_>>(),
            ["default (built-in)", "alpha", "zebra"]
        );
    }

    #[test]
    fn names_are_stripped_then_trimmed_and_never_built_in() {
        let name = ThemeName::new(" custom/theme\\name\0 ").unwrap();
        assert_eq!(name.as_str(), "customthemename");
        for built_in in ["", " ", "/", "default", "default/", " default "] {
            assert_eq!(ThemeName::new(built_in), None, "{built_in:?}");
        }
    }
}
