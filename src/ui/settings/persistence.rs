use super::{
    bar::{BarAlignment, clamp_bar_height},
    data::UiSettings,
};
use crate::ui::{app::config::CaptureMode, visualization::visual_manager::VisualKind};
use iced::Color;
use serde::Serialize;
use std::{
    cell::{Ref, RefCell},
    fs,
    path::PathBuf,
    rc::Rc,
    sync::{OnceLock, mpsc},
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

#[derive(Debug)]
pub struct SettingsManager {
    path: PathBuf,
    pub data: UiSettings,
}

impl SettingsManager {
    pub fn load_or_default() -> Self {
        let path = config_dir().join("settings.json");
        let mut data: UiSettings = fs::read_to_string(&path)
            .ok()
            .and_then(|s| {
                serde_json::from_str(&s)
                    .map_err(|e| warn!("[settings] parse error {path:?}: {e}"))
                    .ok()
            })
            .unwrap_or_default();
        data.visuals.sanitize();
        data.bar.height = clamp_bar_height(data.bar.height);
        Self { path, data }
    }
    pub fn settings(&self) -> &UiSettings {
        &self.data
    }
    pub fn set_visual_enabled(&mut self, kind: VisualKind, enabled: bool) {
        self.data.visuals.modules.entry(kind).or_default().enabled = Some(enabled);
    }
    pub fn set_module_config<T: Serialize>(&mut self, kind: VisualKind, config: &T) {
        self.data
            .visuals
            .modules
            .entry(kind)
            .or_default()
            .set_config(config);
    }
    pub fn set_visual_order(&mut self, order: impl IntoIterator<Item = VisualKind>) {
        self.data.visuals.order = order.into_iter().collect();
    }
    pub fn set_background_color(&mut self, c: Option<Color>) {
        self.data.background_color = c.map(Into::into);
    }
    pub fn set_decorations(&mut self, e: bool) {
        self.data.decorations = e;
    }
    pub fn set_bar_enabled(&mut self, enabled: bool) {
        self.data.bar.enabled = enabled;
    }
    pub fn set_bar_alignment(&mut self, alignment: BarAlignment) {
        self.data.bar.alignment = alignment;
    }
    pub fn set_bar_height(&mut self, height: u32) {
        self.data.bar.height = clamp_bar_height(height);
    }
    pub fn set_capture_mode(&mut self, m: CaptureMode) {
        self.data.capture_mode = m;
    }
    pub fn set_last_device_name(&mut self, name: Option<String>) {
        self.data.last_device_name = name;
    }
}

fn schedule_persist(path: PathBuf, mut settings: UiSettings) {
    static SENDER: OnceLock<Option<mpsc::Sender<(PathBuf, UiSettings)>>> = OnceLock::new();
    settings.visuals.sanitize();
    settings.bar.height = clamp_bar_height(settings.bar.height);
    if let Some(sender) = SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<(PathBuf, UiSettings)>();
        std::thread::Builder::new()
            .name("openmeters-settings-saver".into())
            .spawn(move || {
                let mut last_written: Option<String> = None;
                while let Ok((mut dest, mut data)) = rx.recv() {
                    // Coalesce rapid updates by draining pending messages.
                    while let Ok((new_dest, new_data)) = rx.recv_timeout(Duration::from_millis(500))
                    {
                        dest = new_dest;
                        data = new_data;
                    }
                    data.visuals.sanitize();
                    let Ok(json) = serde_json::to_string_pretty(&data) else {
                        continue;
                    };
                    if last_written.as_deref() == Some(&json) {
                        continue;
                    }
                    if let Some(parent) = dest.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let temp_path = dest.with_extension("json.tmp");
                    if fs::write(&temp_path, &json)
                        .and_then(|()| fs::rename(&temp_path, &dest))
                        .is_ok()
                    {
                        last_written = Some(json);
                    }
                }
            })
            .ok()
            .map(|_| tx)
    }) {
        let _ = sender.send((path, settings));
    }
}

#[derive(Debug, Clone)]
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
        manager.data.visuals.sanitize();
        schedule_persist(manager.path.clone(), manager.data.clone());
        result
    }
}
