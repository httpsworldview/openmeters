// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{
    schema::UiSettings,
    theme::{BUILTIN_THEME, ThemeFile, ThemeStore},
};
use std::{
    cell::{Ref, RefCell},
    fs,
    path::PathBuf,
    rc::Rc,
    sync::{Mutex, mpsc},
    time::Duration,
};
use tracing::warn;

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("openmeters")
}

pub struct SettingsManager {
    path: PathBuf,
    pub data: UiSettings,
    theme_store: ThemeStore,
}

impl SettingsManager {
    pub fn load_or_default() -> Self {
        let dir = config_dir();
        let path = dir.join("settings.json");
        let mut data: UiSettings = fs::read_to_string(&path)
            .ok()
            .and_then(|s| {
                serde_json::from_str(&s)
                    .inspect_err(|e| warn!("[settings] parse error {path:?}: {e}"))
                    .ok()
            })
            .unwrap_or_default();
        data.visuals.sanitize();
        data.bar.sanitize();
        let theme_store = ThemeStore::new(&dir);
        // Populate background color from the active theme (settings.json no longer stores it).
        if let Some(theme_file) = theme_store.load(data.theme.as_deref().unwrap_or(BUILTIN_THEME))
            && let Some(bg) = theme_file.background
        {
            data.background_color = Some(bg);
        }
        Self {
            path,
            data,
            theme_store,
        }
    }
    pub fn theme_store(&self) -> &ThemeStore {
        &self.theme_store
    }
    pub fn active_theme(&self) -> &str {
        self.data.theme.as_deref().unwrap_or(BUILTIN_THEME)
    }
    pub fn update_active_theme(&mut self, mutate: impl FnOnce(&mut ThemeFile)) {
        let active = self.active_theme().to_owned();
        if active != BUILTIN_THEME {
            if let Err(e) = self.theme_store.update(&active, mutate) {
                warn!("[theme] update failed for {active:?}: {e}");
            }
            return;
        }

        let name = self.theme_store.next_auto_name();
        let mut theme = ThemeFile {
            name: Some(name.clone()),
            ..Default::default()
        };
        mutate(&mut theme);
        if let Err(e) = self.theme_store.save(&name, &theme) {
            warn!("[theme] update failed for {name:?}: {e}");
        } else {
            self.data.theme = Some(name);
        }
    }
}

type PersistRequest = (PathBuf, UiSettings);
const PERSIST_DEBOUNCE: Duration = Duration::from_millis(500);

fn schedule_persist(mut path: PathBuf, mut settings: UiSettings) {
    static SENDER: Mutex<Option<mpsc::Sender<PersistRequest>>> = Mutex::new(None);

    settings.visuals.strip_all_palettes();

    let mut sender = SENDER.lock().unwrap();
    if let Some(tx) = sender.as_ref() {
        match tx.send((path, settings)) {
            Ok(()) => return,
            Err(mpsc::SendError(failed)) => (path, settings) = failed,
        }
        *sender = None;
    }

    let (tx, rx) = mpsc::channel::<PersistRequest>();
    match std::thread::Builder::new()
        .name("openmeters-settings-saver".into())
        .spawn(move || settings_saver_loop(rx))
    {
        Ok(_) => {
            let _ = tx.send((path, settings));
            *sender = Some(tx);
        }
        Err(err) => tracing::error!("[settings] failed to spawn saver thread: {err}"),
    }
}

fn settings_saver_loop(rx: mpsc::Receiver<PersistRequest>) {
    let mut last_written: Option<String> = None;
    while let Ok((mut dest, mut data)) = rx.recv() {
        while let Ok(next) = rx.recv_timeout(PERSIST_DEBOUNCE) {
            (dest, data) = next;
        }

        let Ok(json) = serde_json::to_string_pretty(&data) else {
            tracing::warn!("[settings] serialization failed");
            continue;
        };
        if last_written.as_deref() == Some(&json) {
            continue;
        }
        if let Some(parent) = dest.parent()
            && let Err(err) = fs::create_dir_all(parent)
        {
            tracing::warn!("[settings] failed to create config dir: {err}");
        }
        match super::write_json_atomic(&dest, &json) {
            Ok(()) => last_written = Some(json),
            Err(err) => tracing::warn!("[settings] failed to write settings: {err}"),
        }
    }
}

#[derive(Clone)]
pub struct SettingsHandle(Rc<RefCell<SettingsManager>>);

impl SettingsHandle {
    pub fn load_or_default() -> Self {
        Self(Rc::new(RefCell::new(SettingsManager::load_or_default())))
    }
    pub fn borrow(&self) -> Ref<'_, SettingsManager> {
        self.0.borrow()
    }
    pub fn update<F: FnOnce(&mut SettingsManager) -> R, R>(&self, mutate: F) -> R {
        let mut manager = self.0.borrow_mut();
        let result = mutate(&mut manager);
        manager.data.visuals.sanitize_layout();
        manager.data.bar.sanitize();
        schedule_persist(manager.path.clone(), manager.data.clone());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_theme_updates_create_auto_theme() {
        let dir = tempfile::tempdir().unwrap();
        let mut manager = SettingsManager {
            path: dir.path().join("settings.json"),
            data: UiSettings::default(),
            theme_store: ThemeStore::new(dir.path()),
        };
        manager
            .theme_store
            .save("default-custom", &ThemeFile::default())
            .unwrap();

        manager.update_active_theme(|theme| theme.author = Some("OpenMeters".into()));

        assert_eq!(manager.active_theme(), "default-custom-2");
        assert_eq!(
            manager
                .theme_store
                .load("default-custom-2")
                .unwrap()
                .author
                .as_deref(),
            Some("OpenMeters")
        );
    }
}
