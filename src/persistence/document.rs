// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{PendingWrite, lossy, watch::SettingsWatcher};
use crate::util::unpoison;
use serde::Serialize;
use serde_json::{Map, Value};
use std::{
    fs, io,
    marker::PhantomData,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};
use tracing::{debug, warn};

const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

pub(super) trait Stored: Serialize + Default {
    const SCOPE: &'static str;

    fn read_fields(map: &mut Map<String, Value>, out: &mut Self, scope: &str);

    // Fails unless `raw` is a JSON object; its invalid fields keep their defaults.
    fn from_json_lossy(raw: &[u8]) -> serde_json::Result<Self> {
        let value = Value::Object(serde_json::from_slice(raw)?);
        Ok(lossy::settings(value, Self::SCOPE, Self::read_fields))
    }
}

pub(super) fn read_file(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// The file as this app last read or wrote it.
struct Disk {
    bytes: Option<Vec<u8>>,
    generation: u64,
}

// Shared with the saver thread.
struct File {
    path: PathBuf,
    disk: Mutex<Disk>,
    watcher: SettingsWatcher,
}

type SaveRequest = (String, u64);

/// A JSON file that outside programs may edit too.
pub(super) struct Document<T> {
    file: Arc<File>,
    writable: bool,
    unreadable: bool,
    saver: Option<(mpsc::Sender<SaveRequest>, JoinHandle<()>)>,
    value: PhantomData<fn() -> T>,
}

impl<T: Stored> Document<T> {
    pub(super) fn open(path: PathBuf, watcher: &SettingsWatcher) -> (Self, Option<T>) {
        let file = Arc::new(File {
            path,
            disk: Mutex::new(Disk {
                bytes: None,
                generation: 0,
            }),
            watcher: watcher.clone(),
        });
        let mut document = Self {
            file,
            writable: true,
            unreadable: false,
            saver: None,
            value: PhantomData,
        };
        let value = document.reload();
        (document, value)
    }

    pub(super) fn reload(&mut self) -> Option<T> {
        let path = &self.file.path;
        let mut disk = unpoison(self.file.disk.lock());
        let bytes = match read_file(path) {
            Ok(bytes) => bytes,
            Err(err) => {
                if !std::mem::replace(&mut self.unreadable, true) {
                    warn!("[settings] cannot read {path:?}: {err}");
                }
                return None;
            }
        };
        self.unreadable = false;
        if bytes == disk.bytes {
            return None;
        }
        let parsed = bytes.as_deref().map(T::from_json_lossy);
        disk.bytes = bytes;
        let Some(parsed) = parsed else {
            // Nothing is left to protect
            self.writable = true;
            return None;
        };
        disk.generation += 1;
        self.writable = parsed.is_ok();
        parsed
            .inspect_err(|err| warn!("[settings] ignoring invalid {path:?}: {err}"))
            .ok()
    }

    pub(super) fn save(&mut self, value: &T, explicit: bool) {
        let Some(request) = self.request(value, explicit) else {
            return;
        };
        if self.saver.is_none() {
            let (sender, requests) = mpsc::channel();
            let file = Arc::clone(&self.file);
            match thread::Builder::new()
                .name("openmeters-saver".into())
                .spawn(move || save_loop(&file, &requests))
            {
                Ok(join) => self.saver = Some((sender, join)),
                Err(err) => warn!("[settings] saving without debounce: {err}"),
            }
        }
        match &self.saver {
            Some((sender, _)) => drop(sender.send(request)),
            None => {
                self.file.commit(request);
            }
        }
    }

    /// Saves now, returning whether the file holds `value`.
    pub(super) fn write(&mut self, value: &T, explicit: bool) -> bool {
        self.flush(); // older pending save must not land after this
        self.request(value, explicit)
            .is_some_and(|request| self.file.commit(request))
    }

    fn request(&mut self, value: &T, explicit: bool) -> Option<SaveRequest> {
        self.writable |= explicit;
        if !self.writable {
            return None;
        }
        let json = serde_json::to_string_pretty(value)
            .inspect_err(|err| warn!("[settings] serialization failed: {err}"))
            .ok()?;
        Some((json, unpoison(self.file.disk.lock()).generation))
    }
}

impl<T> Document<T> {
    pub(super) fn path(&self) -> &Path {
        &self.file.path
    }

    pub(super) fn flush(&mut self) {
        let Some((sender, join)) = self.saver.take() else {
            return;
        };
        drop(sender);
        if join.join().is_err() {
            warn!("[settings] saver thread panicked");
        }
    }
}

impl<T> Drop for Document<T> {
    fn drop(&mut self) {
        self.flush();
    }
}

fn save_loop(file: &File, requests: &mpsc::Receiver<SaveRequest>) {
    while let Ok(mut latest) = requests.recv() {
        while let Ok(next) = requests.recv_timeout(SAVE_DEBOUNCE) {
            latest = next;
        }
        file.commit(latest);
    }
}

impl File {
    /// Returns whether the file holds the request's JSON.
    fn commit(&self, request: SaveRequest) -> bool {
        self.try_commit(request).unwrap_or_else(|err| {
            warn!("[settings] cannot save {:?}: {err}", self.path);
            false
        })
    }

    fn try_commit(&self, (json, generation): SaveRequest) -> io::Result<bool> {
        let path = &self.path;
        let superseded = |disk: &Disk| disk.generation != generation;
        {
            let disk = unpoison(self.disk.lock());
            if superseded(&disk) {
                return Ok(false);
            }
            if disk.bytes.as_deref() == Some(json.as_bytes()) {
                return Ok(true);
            }
        }
        let write = PendingWrite::new(path, &json)?;
        let mut disk = unpoison(self.disk.lock());
        if superseded(&disk) {
            return Ok(false);
        }
        // outside edits take priority
        let found = read_file(path)?;
        if found.is_some() && found != disk.bytes {
            debug!("[settings] {path:?} changed on disk; reloading instead of saving");
            self.watcher.wake();
            return Ok(false);
        }
        write.commit()?;
        disk.bytes = Some(json.into_bytes());
        Ok(true)
    }
}
