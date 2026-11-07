// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{
    document::Document,
    schema::UiSettings,
    theme::{BUILTIN_THEME, ThemeChoice, ThemeFile, ThemeStore, theme_name},
    watch::SettingsWatcher,
};
use crate::util::set_if_changed;
use std::{
    cell::{Ref, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
};
use tracing::warn;

fn config_dir() -> PathBuf {
    // XDG: an empty or relative value is invalid and means unset.
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("openmeters")
}

/// Settings and the active theme, as a whole.
#[derive(Clone)]
pub struct Snapshot {
    pub settings: UiSettings,
    pub theme: ThemeFile,
}

pub struct SettingsManager {
    pub data: UiSettings,
    theme: ThemeFile,
    // What the running app shows; edits and reloads move `data` and `theme` ahead of it.
    applied: Snapshot,
    changed: bool,
    document: Document<UiSettings>,
    // None while the theme is built in.
    theme_document: Option<Document<ThemeFile>>,
    theme_store: ThemeStore,
    watcher: SettingsWatcher,
}

fn open_theme(
    store: &ThemeStore,
    watcher: &SettingsWatcher,
    name: Option<&str>,
) -> (Option<Document<ThemeFile>>, ThemeFile) {
    let Some(name) = name else {
        return (None, ThemeFile::default());
    };
    let (document, theme) = Document::open(store.path(name), watcher);
    if theme.is_none() {
        warn!("[settings] showing built-in colors for theme {name:?}");
    }
    (Some(document), theme.unwrap_or_default())
}

impl SettingsManager {
    fn load_from_dir(dir: &Path) -> Self {
        let theme_store = ThemeStore::new(dir);
        let watcher = SettingsWatcher::new([dir.to_owned(), theme_store.dir().to_owned()]);
        let (document, data) = Document::open(dir.join("settings.json"), &watcher);
        let data: UiSettings = data.unwrap_or_default();
        let (theme_document, theme) = open_theme(&theme_store, &watcher, data.theme.as_deref());
        Self {
            applied: Snapshot {
                settings: data.clone(),
                theme: theme.clone(),
            },
            data,
            theme,
            changed: false,
            document,
            theme_document,
            theme_store,
            watcher,
        }
    }

    pub fn themes(&self) -> Vec<ThemeChoice> {
        self.theme_store.list()
    }
    pub fn active_theme(&self) -> &str {
        self.data.theme.as_deref().unwrap_or(BUILTIN_THEME)
    }
    pub fn theme(&self) -> &ThemeFile {
        &self.theme
    }

    fn edit(&mut self, edit: impl FnOnce(&mut UiSettings)) {
        let before = self.data.clone();
        edit(&mut self.data);
        if self.data != before {
            self.changed = true;
            self.document.save(&self.data, true);
            self.follow_theme();
        }
    }

    fn observe(&mut self, observe: impl Fn(&mut UiSettings)) {
        observe(&mut self.applied.settings);
        let before = self.data.clone();
        observe(&mut self.data);
        if self.data != before {
            self.document.save(&self.data, false);
        }
    }

    fn edit_theme(&mut self, edit: impl Fn(&mut ThemeFile)) {
        let mut theme = self.theme.clone();
        edit(&mut theme);
        if theme == self.theme {
            return;
        }
        edit(&mut self.applied.theme);
        if let Some(document) = &mut self.theme_document {
            // An edit is not consent to replace a theme file that failed to load.
            document.write(&theme, false);
            self.theme = theme;
            return;
        }
        // Edits to the built-in theme start a new one once it is saved.
        let name = self.theme_store.next_auto_name();
        let (mut document, _) = Document::open(self.theme_store.path(&name), &self.watcher);
        theme.name = Some(name.clone());
        if !document.write(&theme, false) {
            theme.name = None;
            self.theme = theme;
            return;
        }
        self.applied.theme.name.clone_from(&theme.name);
        self.theme_document = Some(document);
        self.theme = theme;
        self.edit(|data| data.theme = Some(name));
    }

    fn save_theme_as(&mut self, name: &str) {
        let Some(name) = theme_name(name) else {
            return warn!("[settings] invalid theme name {name:?}");
        };
        let path = self.theme_store.path(&name);
        let mut opened = None;
        // The active file may hold outside edits not reloaded yet; they must not become the base.
        let (document, stored) = match &mut self.theme_document {
            Some(active) if active.path() == path => (active, Some(self.theme.clone())),
            _ => {
                let (document, stored) = Document::open(path, &self.watcher);
                (opened.insert(document), stored)
            }
        };
        let stored = stored.unwrap_or_default();
        let theme = ThemeFile {
            name: stored.name.or_else(|| Some(name.clone())),
            author: stored.author,
            ..self.theme.clone()
        };
        // Naming the file is consent to replace contents that failed to load.
        if !document.write(&theme, true) {
            return;
        }
        if let Some(document) = opened {
            self.theme_document = Some(document);
        }
        self.changed |= set_if_changed(&mut self.theme, theme);
        self.edit(|data| data.theme = Some(name));
    }

    fn follow_theme(&mut self) {
        let name = self.data.theme.as_deref();
        let path = name.map(|name| self.theme_store.path(name));
        if self.theme_document.as_ref().map(Document::path) == path.as_deref() {
            return;
        }
        let (document, theme) = open_theme(&self.theme_store, &self.watcher, name);
        self.theme_document = document;
        self.changed |= set_if_changed(&mut self.theme, theme);
    }

    fn reload(&mut self) {
        if let Some(settings) = self.document.reload() {
            self.changed |= set_if_changed(&mut self.data, settings);
            self.follow_theme();
        }
        if let Some(theme) = self.theme_document.as_mut().and_then(Document::reload) {
            self.changed |= set_if_changed(&mut self.theme, theme);
        }
    }

    fn take_change(&mut self) -> Option<(Snapshot, Snapshot)> {
        if !std::mem::take(&mut self.changed) {
            return None;
        }
        let current = Snapshot {
            settings: self.data.clone(),
            theme: self.theme.clone(),
        };
        Some((
            std::mem::replace(&mut self.applied, current.clone()),
            current,
        ))
    }
}

#[derive(Clone)]
pub struct SettingsHandle(Rc<RefCell<SettingsManager>>);

impl SettingsHandle {
    pub fn load_or_default() -> Self {
        Self::load_from(&config_dir())
    }

    pub(crate) fn load_from(dir: &Path) -> Self {
        Self(Rc::new(RefCell::new(SettingsManager::load_from_dir(dir))))
    }

    pub fn borrow(&self) -> Ref<'_, SettingsManager> {
        self.0.borrow()
    }

    /// Changes settings for the app to apply.
    pub fn edit(&self, edit: impl FnOnce(&mut UiSettings)) {
        self.0.borrow_mut().edit(edit);
    }

    /// Records what the running app already shows, like a window's size. Runs twice.
    pub fn observe(&self, observe: impl Fn(&mut UiSettings)) {
        self.0.borrow_mut().observe(observe);
    }

    /// Saves an edit the app already shows to the active theme, starting a new theme if it
    /// is built in. Runs twice.
    pub fn edit_theme(&self, edit: impl Fn(&mut ThemeFile)) {
        self.0.borrow_mut().edit_theme(edit);
    }

    /// Saves the active theme's colors as theme `name`, keeping that file's name and author,
    /// and makes it active.
    pub fn save_theme_as(&self, name: &str) {
        self.0.borrow_mut().save_theme_as(name);
    }

    /// Adopts edits made outside since the last reload or save.
    pub fn reload(&self) {
        self.0.borrow_mut().reload();
    }

    /// What the app last applied and what it should show now, after any change.
    pub fn take_change(&self) -> Option<(Snapshot, Snapshot)> {
        self.0.borrow_mut().take_change()
    }

    /// Watches the settings and theme files for outside edits, for the rest of the run.
    pub fn watch_files(&self) {
        self.0.borrow().watcher.start();
    }

    pub fn watcher(&self) -> SettingsWatcher {
        self.0.borrow().watcher.clone()
    }

    /// Writes a pending save now.
    pub fn flush(&self) {
        self.0.borrow_mut().document.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{document::Stored, schema::VisualFrameRate, watch::changes};
    use std::fs;

    fn json(path: &Path) -> serde_json::Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    fn theme_file(path: &Path) -> ThemeFile {
        ThemeFile::from_json_lossy(&fs::read(path).unwrap()).unwrap()
    }

    fn theme_changed(manager: &mut SettingsManager) -> bool {
        manager.reload();
        let change = manager.take_change();
        change.is_some_and(|(old, new)| old.theme != new.theme)
    }

    #[test]
    fn theme_edits_start_auto_themes_and_outside_edits_win() {
        let dir = tempfile::tempdir().unwrap();
        let themes = dir.path().join("themes");
        let path = themes.join("default-custom-2.json");
        let mut manager = SettingsManager::load_from_dir(dir.path());

        // A theme that cannot be saved does not become active.
        fs::write(&themes, "").unwrap();
        manager.edit_theme(|theme| theme.author = Some("Unsaved".into()));
        assert_eq!(manager.active_theme(), BUILTIN_THEME);
        fs::remove_file(&themes).unwrap();
        fs::create_dir(&themes).unwrap();
        fs::write(themes.join("default-custom.json"), "{}").unwrap();

        // An edit that changes nothing starts no theme.
        manager.edit_theme(|theme| theme.background = None);
        assert_eq!(manager.active_theme(), BUILTIN_THEME);
        manager.edit_theme(|theme| theme.author = Some("OpenMeters".into()));
        assert_eq!(manager.active_theme(), "default-custom-2");
        assert_eq!(&theme_file(&path), manager.theme());
        assert!(!theme_changed(&mut manager));

        // An outside edit wins over an app edit saved before the reload.
        let mut outside = json(&path);
        outside["name"] = "External".into();
        fs::write(&path, outside.to_string()).unwrap();
        manager.edit_theme(|theme| theme.author = Some("Edited".into()));
        assert_eq!(json(&path), outside);
        // The refused save asks for a reload, even without a file watcher.
        assert!(changes(&manager.watcher).try_recv().is_ok());
        assert!(theme_changed(&mut manager));
        assert_eq!(manager.theme().name.as_deref(), Some("External"));
        assert_eq!(manager.theme().author.as_deref(), Some("OpenMeters"));
        assert!(!theme_changed(&mut manager));

        // Edits never replace invalid contents; they stay until fixed or removed.
        fs::write(&path, "{ \"name\": ").unwrap();
        assert!(!theme_changed(&mut manager));
        assert_eq!(manager.theme().name.as_deref(), Some("External"));
        manager.edit_theme(|theme| theme.author = Some("Unsaved".into()));
        assert_eq!(fs::read(&path).unwrap(), b"{ \"name\": ");

        fs::remove_file(&path).unwrap();
        assert!(!theme_changed(&mut manager));
        manager.edit_theme(|theme| theme.author = Some("Recreated".into()));
        assert_eq!(&theme_file(&path), manager.theme());
        assert_eq!(manager.theme().name.as_deref(), Some("External"));
    }

    #[test]
    fn broken_themes_show_built_in_colors_and_save_as_keeps_the_target_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let themes = dir.path().join("themes");
        fs::create_dir_all(&themes).unwrap();
        fs::write(themes.join("broken.json"), "{").unwrap();
        fs::create_dir(themes.join("unwritable.json")).unwrap();
        let kept = themes.join("kept.json");
        fs::write(&kept, r#"{"name":"Kept","author":"Someone"}"#).unwrap();
        let mut manager = SettingsManager::load_from_dir(dir.path());

        manager.edit(|s| s.theme = Some("broken".into()));
        assert_eq!(manager.theme(), &ThemeFile::default());
        manager.edit_theme(|theme| theme.background = Some(iced::Color::WHITE.into()));
        assert_eq!(fs::read(themes.join("broken.json")).unwrap(), b"{");
        for unsaved in ["default/", " ", "unwritable"] {
            manager.save_theme_as(unsaved);
            assert_eq!(manager.active_theme(), "broken");
        }
        manager.save_theme_as(" kept ");
        assert_eq!(manager.active_theme(), "kept");
        let saved = theme_file(&kept);
        assert_eq!(saved.name.as_deref(), Some("Kept"));
        assert_eq!(saved.author.as_deref(), Some("Someone"));
        assert_eq!(saved.background, Some(iced::Color::WHITE.into()));
        assert_eq!(&saved, manager.theme());
    }

    #[test]
    fn saving_requires_a_valid_load_or_explicit_change() {
        let window = serde_json::json!({"width":949,"height":514});
        for (original, can_save, decorations) in [
            (None, true, false),
            (Some(br#"{"decorations":true}"#.as_slice()), true, true),
            (Some(b"{ malformed settings"), false, false),
            (Some(b"\xff\xfe"), false, false),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.json");
            if let Some(original) = original {
                fs::write(&path, original).unwrap();
            }
            let handle = SettingsHandle::load_from(dir.path());
            handle.edit(|s| s.decorations = decorations);
            assert!(handle.take_change().is_none());
            handle.observe(|s| s.main_window.width = 949);
            handle.observe(|s| s.main_window.height = 514);
            handle.flush();
            if can_save {
                assert_eq!(json(&path)["main_window"], window);
            } else {
                assert_eq!(fs::read(&path).unwrap(), original.unwrap());
            }

            handle.edit(|s| s.decorations = !decorations);
            handle.flush();
            assert_eq!(json(&path)["decorations"], !decorations);
            assert_eq!(json(&path)["main_window"], window);

            handle.observe(|s| s.main_window.height = 515);
            handle.flush();
            assert_eq!(json(&path)["main_window"]["height"], 515);
        }
    }

    #[test]
    fn outside_edits_win_over_unsaved_changes_and_pending_saves() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut manager = SettingsManager::load_from_dir(dir.path());
        manager.edit(|s| s.decorations = true);
        assert!(manager.take_change().is_some());
        manager.document.flush();
        manager.reload();
        assert!(manager.take_change().is_none());

        let theme = ThemeFile {
            author: Some("Elsewhere".into()),
            ..Default::default()
        };
        fs::create_dir_all(dir.path().join("themes")).unwrap();
        let theme_json = serde_json::to_string(&theme).unwrap();
        fs::write(dir.path().join("themes/elsewhere.json"), theme_json).unwrap();
        let mut outside = json(&path);
        outside["visual_frame_rate"] = "fps30".into();
        outside["theme"] = "elsewhere".into();
        manager.edit(|s| s.bar.enabled = false);
        fs::write(&path, outside.to_string()).unwrap();
        manager.document.flush();
        assert_eq!(json(&path), outside);
        assert!(theme_changed(&mut manager));
        assert_eq!(manager.theme(), &theme);
        assert_eq!(manager.data.visual_frame_rate, VisualFrameRate::Fps30);
        assert!(manager.data.bar.enabled);

        outside["visual_frame_rate"] = "fps120".into();
        manager.edit(|s| s.visual_frame_rate = VisualFrameRate::Fps60);
        fs::write(&path, outside.to_string()).unwrap();
        manager.reload();
        manager.document.flush();
        assert_eq!(manager.data.visual_frame_rate, VisualFrameRate::Fps120);
        assert_eq!(json(&path), outside);

        // Saves requested before invalid contents arrived must not replace them either.
        manager.observe(|s| s.main_window.height = 300);
        fs::write(&path, "{").unwrap();
        manager.reload();
        manager.document.flush();
        assert_eq!(fs::read(&path).unwrap(), b"{");
        assert_eq!(manager.data.visual_frame_rate, VisualFrameRate::Fps120);
        manager.observe(|s| s.main_window.width = 640);
        manager.document.flush();
        assert_eq!(fs::read(&path).unwrap(), b"{");
        manager.edit(|s| s.decorations = false);
        manager.document.flush();
        assert_eq!(json(&path)["main_window"]["width"], 640);

        manager.edit(|s| s.decorations = true);
        fs::remove_file(&path).unwrap();
        manager.document.flush();
        assert_eq!(json(&path)["decorations"], true);
    }
}
