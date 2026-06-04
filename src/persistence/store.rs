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
    sync::{Mutex, PoisonError, mpsc},
    thread::JoinHandle,
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
                UiSettings::from_json_lossy(&s)
                    .inspect_err(|e| warn!("[settings] parse error {path:?}: {e}"))
                    .ok()
            })
            .unwrap_or_default();
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
type SaverThread = (mpsc::Sender<PersistRequest>, JoinHandle<()>);
const PERSIST_DEBOUNCE: Duration = Duration::from_millis(500);

static SAVER: Mutex<Option<SaverThread>> = Mutex::new(None);

fn schedule_persist(mut path: PathBuf, mut settings: UiSettings) {
    // Theme files own palettes; settings.json stores module config only.
    for module in settings.visuals.modules.values_mut() {
        module.strip_palette();
    }

    let mut saver = SAVER.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((tx, _)) = saver.as_ref() {
        match tx.send((path, settings)) {
            Ok(()) => return,
            Err(mpsc::SendError(failed)) => (path, settings) = failed,
        }
    }

    if let Some((tx, join)) = saver.take() {
        drop(tx);
        let _ = join.join();
    }

    let (tx, rx) = mpsc::channel::<PersistRequest>();
    match std::thread::Builder::new()
        .name("openmeters-settings-saver".into())
        .spawn(move || settings_saver_loop(rx))
    {
        Ok(join) => {
            if tx.send((path, settings)).is_ok() {
                *saver = Some((tx, join));
            } else {
                let _ = join.join();
            }
        }
        Err(err) => tracing::error!("[settings] failed to spawn saver thread: {err}"),
    }
}

fn flush_persist() {
    let Some((tx, join)) = SAVER.lock().unwrap_or_else(PoisonError::into_inner).take() else {
        return;
    };
    drop(tx);
    if join.join().is_err() {
        tracing::warn!("[settings] saver thread panicked during flush");
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
        schedule_persist(manager.path.clone(), manager.data.clone());
        result
    }

    pub fn flush(&self) {
        flush_persist();
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

    #[test]
    fn flush_writes_pending_settings_without_waiting_for_debounce() {
        flush_persist();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let handle = SettingsHandle(Rc::new(RefCell::new(SettingsManager {
            path: path.clone(),
            data: UiSettings::default(),
            theme_store: ThemeStore::new(dir.path()),
        })));

        handle.update(|settings| settings.data.decorations = true);
        handle.flush();

        let saved: UiSettings = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert!(saved.decorations);
    }
}
