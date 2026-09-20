// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{
    schema::UiSettings,
    theme::{BUILTIN_THEME, ThemeFile, ThemeStore},
};
use std::{
    cell::{Ref, RefCell},
    fs, io,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Mutex, mpsc},
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ChangeOrigin {
    User,
    Automatic,
}

pub struct SettingsManager {
    path: PathBuf,
    pub data: UiSettings,
    theme_store: ThemeStore,
    can_persist: bool,
}

impl SettingsManager {
    fn load_from_dir(dir: &Path) -> Self {
        let path = dir.join("settings.json");
        let loaded = fs::read_to_string(&path)
            .and_then(|raw| UiSettings::from_json_lossy(&raw).map_err(io::Error::other));
        let (mut data, can_persist) = match loaded {
            Ok(data) => (data, true),
            Err(err) => {
                let missing = err.kind() == io::ErrorKind::NotFound;
                if !missing {
                    warn!("[settings] load error {path:?}: {err}");
                }
                (UiSettings::default(), missing)
            }
        };
        let theme_store = ThemeStore::new(dir);
        if let Some(theme_file) = theme_store.load(data.theme.as_deref().unwrap_or(BUILTIN_THEME))
            && let Some(bg) = theme_file.background
        {
            data.background_color = Some(bg);
        }
        Self {
            path,
            data,
            theme_store,
            can_persist,
        }
    }

    fn persist(&mut self, origin: ChangeOrigin) {
        // Incidental updates must not replace a file that failed to load.
        self.can_persist |= origin == ChangeOrigin::User;
        if self.can_persist {
            schedule_persist(self.path.clone(), self.data.clone());
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

static SAVER: Mutex<Option<(mpsc::Sender<PersistRequest>, JoinHandle<()>)>> = Mutex::new(None);

fn schedule_persist(mut path: PathBuf, mut settings: UiSettings) {
    let mut saver = crate::util::unpoison(SAVER.lock());
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
    tx.send((path, settings)).expect("new saver receiver");
    match std::thread::Builder::new()
        .name("openmeters-settings-saver".into())
        .spawn(move || settings_saver_loop(rx))
    {
        Ok(join) => *saver = Some((tx, join)),
        Err(err) => tracing::error!("[settings] failed to spawn saver thread: {err}"),
    }
}

fn settings_saver_loop(rx: mpsc::Receiver<PersistRequest>) {
    let mut last_written = String::new();
    while let Ok((mut dest, mut data)) = rx.recv() {
        while let Ok(next) = rx.recv_timeout(PERSIST_DEBOUNCE) {
            (dest, data) = next;
        }

        let Ok(json) = serde_json::to_string_pretty(&data) else {
            tracing::warn!("[settings] serialization failed");
            continue;
        };
        if last_written == json {
            continue;
        }
        match super::write_json_atomic(&dest, &json) {
            Ok(()) => last_written = json,
            Err(err) => tracing::warn!("[settings] failed to write settings: {err}"),
        }
    }
}

#[derive(Clone)]
pub struct SettingsHandle(Rc<RefCell<SettingsManager>>);

impl SettingsHandle {
    pub fn load_or_default() -> Self {
        Self(Rc::new(RefCell::new(SettingsManager::load_from_dir(
            &config_dir(),
        ))))
    }

    pub fn borrow(&self) -> Ref<'_, SettingsManager> {
        self.0.borrow()
    }
    pub fn update(&self, origin: ChangeOrigin, mutate: impl FnOnce(&mut SettingsManager)) {
        let mut manager = self.0.borrow_mut();
        mutate(&mut manager);
        manager.persist(origin);
    }

    pub(crate) fn set<T: PartialEq>(
        &self,
        origin: ChangeOrigin,
        select: impl FnOnce(&mut UiSettings) -> &mut T,
        value: T,
    ) -> bool {
        let mut manager = self.0.borrow_mut();
        if !crate::util::set_if_changed(select(&mut manager.data), value) {
            return false;
        }
        manager.persist(origin);
        true
    }

    pub fn flush() {
        let Some((tx, join)) = crate::util::unpoison(SAVER.lock()).take() else {
            return;
        };
        drop(tx);
        if join.join().is_err() {
            tracing::warn!("[settings] saver thread panicked during flush");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ChangeOrigin::*, *};

    #[test]
    fn builtin_theme_updates_create_auto_theme() {
        let dir = tempfile::tempdir().unwrap();
        let mut manager = SettingsManager::load_from_dir(dir.path());
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
            let manager = SettingsManager::load_from_dir(dir.path());
            let handle = SettingsHandle(Rc::new(RefCell::new(manager)));
            let saved =
                || serde_json::from_slice::<serde_json::Value>(&fs::read(&path).unwrap()).unwrap();
            let changed = handle.set(User, |s| &mut s.decorations, decorations);
            handle.set(Automatic, |s| &mut s.main_window.width, 949);
            handle.update(Automatic, |s| s.data.main_window.height = 514);
            SettingsHandle::flush();
            assert!(!changed);
            if can_save {
                assert_eq!(saved()["main_window"], window);
            } else {
                assert_eq!(fs::read(&path).unwrap(), original.unwrap());
            }

            handle.set(User, |s| &mut s.decorations, !decorations);
            SettingsHandle::flush();
            assert_eq!(saved()["decorations"], !decorations);
            assert_eq!(saved()["main_window"], window);

            handle.update(Automatic, |s| s.data.main_window.height = 515);
            SettingsHandle::flush();
            assert_eq!(saved()["main_window"]["height"], 515);
        }
    }
}
