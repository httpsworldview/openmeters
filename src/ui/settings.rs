//! Settings persistence and management.

use crate::dsp::oscilloscope::{OscilloscopeConfig, TriggerMode};
use crate::dsp::spectrogram::{FrequencyScale, SpectrogramConfig, WindowKind};
use crate::dsp::spectrum::SpectrumConfig;
use crate::dsp::stereometer::StereometerConfig;
use crate::dsp::waveform::{DownsampleStrategy, WaveformConfig};
use crate::ui::app::config::CaptureMode;
use crate::ui::theme;
use crate::ui::visualization::loudness::MeterMode;
use crate::ui::visualization::visual_manager::{VisualKind, VisualSnapshot};
use iced::Color;
use serde::de::{self, DeserializeOwned, Deserializer};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::cell::{Ref, RefCell};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::Duration;
use tracing::{error, warn};

const SETTINGS_FILE_NAME: &str = "settings.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct UiSettings {
    #[serde(default)]
    pub visuals: VisualSettings,
    #[serde(default)]
    pub background_color: Option<ColorSetting>,
    #[serde(default)]
    pub decorations: bool,
    #[serde(default)]
    pub capture_mode: CaptureMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VisualSettings {
    #[serde(default)]
    pub modules: HashMap<VisualKind, ModuleSettings>,
    #[serde(default)]
    pub order: Vec<VisualKind>,
}

impl VisualSettings {
    pub fn sanitize(&mut self) {
        self.modules
            .retain(|kind, module| module.matches_kind(*kind));
    }
}

#[derive(Debug, Clone, Default)]
pub struct ModuleSettings {
    pub enabled: Option<bool>,
    raw: Option<Value>,
}

impl ModuleSettings {
    pub fn with_config<T>(config: &T) -> Self
    where
        T: Serialize,
    {
        let mut module = Self::default();
        module.set_config(config);
        module
    }

    pub fn set_config<T>(&mut self, config: &T)
    where
        T: Serialize,
    {
        match serde_json::to_value(config) {
            Ok(value) => self.raw = Some(value),
            Err(err) => error!("[settings] failed to serialize module config: {err}"),
        }
    }

    pub fn config<T>(&self) -> Option<T>
    where
        T: DeserializeOwned,
    {
        let raw = self.raw.as_ref()?;
        T::deserialize(raw).ok()
    }

    fn matches_kind(&self, kind: VisualKind) -> bool {
        match kind {
            VisualKind::Spectrogram => self.validate::<SpectrogramSettings>(),
            VisualKind::Spectrum => self.validate::<SpectrumSettings>(),
            VisualKind::Oscilloscope => self.validate::<OscilloscopeSettings>(),
            VisualKind::Waveform => self.validate::<WaveformSettings>(),
            VisualKind::Loudness => self.validate::<LoudnessSettings>(),
            VisualKind::Stereometer => self.validate::<StereometerSettings>(),
        }
    }

    fn validate<T>(&self) -> bool
    where
        T: DeserializeOwned,
    {
        self.raw
            .as_ref()
            .is_none_or(|v| serde_json::from_value::<T>(v.clone()).is_ok())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ColorSetting {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl From<Color> for ColorSetting {
    fn from(c: Color) -> Self {
        Self {
            r: c.r,
            g: c.g,
            b: c.b,
            a: c.a,
        }
    }
}

impl From<ColorSetting> for Color {
    fn from(c: ColorSetting) -> Self {
        Self {
            r: c.r,
            g: c.g,
            b: c.b,
            a: c.a,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct PaletteSettings {
    pub stops: Vec<ColorSetting>,
}

impl PaletteSettings {
    pub fn to_array<const N: usize>(&self) -> Option<[Color; N]> {
        (self.stops.len() == N).then(|| std::array::from_fn(|i| self.stops[i].into()))
    }

    pub fn maybe_from_colors(colors: &[Color], defaults: &[Color]) -> Option<Self> {
        // Only persist if colors differ from defaults
        let differs = colors.len() == defaults.len()
            && colors
                .iter()
                .zip(defaults)
                .any(|(c, d)| !theme::colors_equal(*c, *d));

        differs.then(|| Self {
            stops: colors.iter().copied().map(ColorSetting::from).collect(),
        })
    }
}

/// Defines the `HasPalette` trait and implements it for types with a `palette` field.
macro_rules! define_has_palette {
    ($($ty:ty),+ $(,)?) => {
        pub trait HasPalette {
            fn palette(&self) -> Option<&PaletteSettings>;

            fn set_palette(&mut self, palette: Option<PaletteSettings>);

            fn palette_array<const N: usize>(&self) -> Option<[Color; N]> {
                self.palette().and_then(PaletteSettings::to_array::<N>)
            }
        }

        $(
            impl HasPalette for $ty {
                fn palette(&self) -> Option<&PaletteSettings> {
                    self.palette.as_ref()
                }

                fn set_palette(&mut self, palette: Option<PaletteSettings>) {
                    self.palette = palette;
                }
            }
        )+
    };
}

/// Implements `Display` for an enum by mapping variants to string labels.
macro_rules! display_enum {
    ($ty:ty { $($variant:ident => $label:expr),+ $(,)? }) => {
        impl std::fmt::Display for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(match self {
                    $(Self::$variant => $label,)+
                })
            }
        }
    };
}

impl Serialize for ModuleSettings {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(None)?;

        if let Some(enabled) = self.enabled {
            map.serialize_entry("enabled", &enabled)?;
        }

        match &self.raw {
            Some(Value::Object(object)) => {
                for (key, value) in object {
                    map.serialize_entry(key, value)?;
                }
            }
            Some(value) => {
                map.serialize_entry("config", value)?;
            }
            None => {}
        }

        map.end()
    }
}

impl<'de> Deserialize<'de> for ModuleSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let Value::Object(mut object) = value else {
            return Err(de::Error::custom("module settings must be an object"));
        };

        let enabled = object
            .remove("enabled")
            .map(|value| bool::deserialize(value).map_err(de::Error::custom))
            .transpose()?;

        let raw = match object.remove("config") {
            Some(value) => Some(merge_config(value, object)),
            None if object.is_empty() => None,
            None => Some(Value::Object(object)),
        };

        Ok(Self { enabled, raw })
    }
}

fn merge_config(config: Value, mut remainder: Map<String, Value>) -> Value {
    match config {
        Value::Object(mut inner) => {
            if !remainder.is_empty() {
                inner.extend(remainder);
            }
            Value::Object(inner)
        }
        other => {
            if remainder.is_empty() {
                other
            } else {
                remainder.insert("config".to_owned(), other);
                Value::Object(remainder)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChannelMode {
    Both,
    Left,
    Right,
    #[default]
    Mono,
}

display_enum!(ChannelMode {
    Both => "Left + Right",
    Left => "Left only",
    Right => "Right only",
    Mono => "Mono blend",
});

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OscilloscopeSettings {
    pub segment_duration: f32,
    #[serde(default)]
    pub trigger_mode: TriggerMode,
    pub persistence: f32,
    #[serde(default)]
    pub channel_mode: ChannelMode,
    #[serde(default)]
    pub palette: Option<PaletteSettings>,
}

impl Default for OscilloscopeSettings {
    fn default() -> Self {
        Self {
            segment_duration: OscilloscopeConfig::default().segment_duration,
            trigger_mode: TriggerMode::default(),
            persistence: 0.0,
            channel_mode: ChannelMode::default(),
            palette: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WaveformSettings {
    pub scroll_speed: f32,
    pub downsample: DownsampleStrategy,
    #[serde(default)]
    pub channel_mode: ChannelMode,
    #[serde(default)]
    pub palette: Option<PaletteSettings>,
}

impl Default for WaveformSettings {
    fn default() -> Self {
        Self::from_config(&WaveformConfig::default())
    }
}

impl WaveformSettings {
    pub fn from_config(config: &WaveformConfig) -> Self {
        Self {
            scroll_speed: config.scroll_speed,
            downsample: config.downsample,
            channel_mode: ChannelMode::default(),
            palette: None,
        }
    }

    pub fn apply_to(&self, config: &mut WaveformConfig) {
        config.scroll_speed = self.scroll_speed;
        config.downsample = self.downsample;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpectrumSettings {
    #[serde(flatten)]
    pub config: SpectrumConfig,
    #[serde(default)]
    pub palette: Option<PaletteSettings>,
    #[serde(default)]
    pub smoothing_radius: usize,
    #[serde(default)]
    pub smoothing_passes: usize,
}

impl Default for SpectrumSettings {
    fn default() -> Self {
        Self::from_config(&SpectrumConfig::default())
    }
}

impl SpectrumSettings {
    pub fn from_config(config: &SpectrumConfig) -> Self {
        Self {
            config: config.normalized(),
            palette: None,
            smoothing_radius: 0,
            smoothing_passes: 0,
        }
    }

    pub fn apply_to(&self, config: &mut SpectrumConfig) {
        *config = self.config.normalized();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoudnessSettings {
    pub left_mode: MeterMode,
    pub right_mode: MeterMode,
    #[serde(default)]
    pub palette: Option<PaletteSettings>,
}

impl LoudnessSettings {
    pub fn new(left_mode: MeterMode, right_mode: MeterMode) -> Self {
        Self {
            left_mode,
            right_mode,
            palette: None,
        }
    }
}

impl Default for LoudnessSettings {
    fn default() -> Self {
        Self::new(MeterMode::TruePeak, MeterMode::LufsShortTerm)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StereometerMode {
    Lissajous,
    #[default]
    DotCloud,
}

display_enum!(StereometerMode {
    Lissajous => "Lissajous",
    DotCloud => "Dot Cloud",
});

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StereometerScale {
    Linear,
    #[default]
    Exponential,
}

display_enum!(StereometerScale {
    Linear => "Linear",
    Exponential => "Exponential",
});

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StereometerSettings {
    pub segment_duration: f32,
    pub target_sample_count: usize,
    pub persistence: f32,
    #[serde(default)]
    pub mode: StereometerMode,
    #[serde(default)]
    pub scale: StereometerScale,
    #[serde(default = "default_scale_range")]
    pub scale_range: f32,
    #[serde(default)]
    pub rotation: i8,
    #[serde(default)]
    pub flip: bool,
    #[serde(default)]
    pub palette: Option<PaletteSettings>,
}

fn default_scale_range() -> f32 {
    15.0
}

impl Default for StereometerSettings {
    fn default() -> Self {
        Self::from_config(&StereometerConfig::default())
    }
}

impl StereometerSettings {
    pub fn from_config(config: &StereometerConfig) -> Self {
        Self {
            segment_duration: config.segment_duration,
            target_sample_count: config.target_sample_count,
            persistence: 0.85,
            mode: StereometerMode::default(),
            scale: StereometerScale::default(),
            scale_range: default_scale_range(),
            rotation: -1,
            flip: true,
            palette: None,
        }
    }

    pub fn apply_to(&self, config: &mut StereometerConfig) {
        config.segment_duration = self.segment_duration;
        config.target_sample_count = self.target_sample_count;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SpectrogramSettings {
    pub fft_size: usize,
    pub hop_size: usize,
    pub history_length: usize,
    pub window: WindowKind,
    pub frequency_scale: FrequencyScale,
    pub use_reassignment: bool,
    pub reassignment_power_floor_db: f32,
    pub reassignment_low_bin_limit: usize,
    pub zero_padding_factor: usize,
    pub display_bin_count: usize,
    pub display_min_hz: f32,
    #[serde(default)]
    pub palette: Option<PaletteSettings>,
}

impl Default for SpectrogramSettings {
    fn default() -> Self {
        Self::from_config(&SpectrogramConfig::default())
    }
}

impl SpectrogramSettings {
    pub fn from_config(config: &SpectrogramConfig) -> Self {
        Self {
            fft_size: config.fft_size,
            hop_size: config.hop_size,
            history_length: config.history_length,
            window: config.window,
            frequency_scale: config.frequency_scale,
            use_reassignment: config.use_reassignment,
            reassignment_power_floor_db: config.reassignment_power_floor_db,
            reassignment_low_bin_limit: config.reassignment_low_bin_limit,
            zero_padding_factor: config.zero_padding_factor,
            display_bin_count: config.display_bin_count,
            display_min_hz: config.display_min_hz,
            palette: None,
        }
    }

    pub fn apply_to(&self, config: &mut SpectrogramConfig) {
        config.fft_size = self.fft_size.max(128);
        config.hop_size = self.hop_size.max(1);
        config.history_length = self.history_length.max(1);
        config.window = self.window;
        config.frequency_scale = self.frequency_scale;
        config.use_reassignment = self.use_reassignment;
        config.reassignment_power_floor_db = self.reassignment_power_floor_db.clamp(-160.0, 0.0);
        config.reassignment_low_bin_limit = self.reassignment_low_bin_limit;
        config.zero_padding_factor = self.zero_padding_factor.max(1);
        config.display_bin_count = self.display_bin_count.max(1);
        config.display_min_hz = self.display_min_hz.max(1.0);
    }
}

define_has_palette!(
    OscilloscopeSettings,
    WaveformSettings,
    SpectrumSettings,
    LoudnessSettings,
    StereometerSettings,
    SpectrogramSettings,
);

fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(dir).join("openmeters")
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".config").join("openmeters")
    } else {
        PathBuf::from(".openmeters")
    }
}

#[derive(Debug)]
pub struct SettingsManager {
    path: PathBuf,
    data: UiSettings,
}

impl SettingsManager {
    pub fn load_or_default() -> Self {
        let path = config_dir().join(SETTINGS_FILE_NAME);
        let mut data = Self::load_from_disk(&path).unwrap_or_default();
        data.visuals.sanitize();
        Self { path, data }
    }

    fn load_from_disk(path: &Path) -> Option<UiSettings> {
        let contents = fs::read_to_string(path).ok()?;
        match serde_json::from_str(&contents) {
            Ok(settings) => Some(settings),
            Err(err) => {
                warn!("[settings] failed to parse {path:?}: {err}");
                None
            }
        }
    }

    pub fn settings(&self) -> &UiSettings {
        &self.data
    }

    pub fn set_visual_enabled(&mut self, kind: VisualKind, enabled: bool) {
        let entry = self.data.visuals.modules.entry(kind).or_default();
        entry.enabled = Some(enabled);
    }

    pub fn set_module_config<T>(&mut self, kind: VisualKind, config: &T)
    where
        T: Serialize,
    {
        let entry = self.data.visuals.modules.entry(kind).or_default();
        entry.set_config(config);
    }

    pub fn set_visual_order(&mut self, snapshot: &VisualSnapshot) {
        self.data.visuals.order = snapshot.slots.iter().map(|s| s.kind).collect();
    }

    pub fn set_background_color(&mut self, color: Option<Color>) {
        self.data.background_color = color.map(ColorSetting::from);
    }

    pub fn set_decorations(&mut self, enabled: bool) {
        self.data.decorations = enabled;
    }

    pub fn set_capture_mode(&mut self, mode: CaptureMode) {
        self.data.capture_mode = mode;
    }
}

#[derive(Debug, Clone)]
struct SaveRequest {
    path: PathBuf,
    data: UiSettings,
}

fn persist_settings_sync(path: &Path, mut data: UiSettings) {
    data.visuals.sanitize();
    match serde_json::to_string_pretty(&data) {
        Ok(json) => {
            if let Err(err) = write_settings_atomic(path, &json) {
                error!("[settings] failed to persist UI settings (sync fallback): {err}");
            }
        }
        Err(err) => error!("[settings] failed to serialize UI settings (sync fallback): {err}"),
    }
}

fn enqueue_save(path: PathBuf, data: UiSettings) {
    static SAVE_TX: OnceLock<Option<mpsc::Sender<SaveRequest>>> = OnceLock::new();

    let tx = SAVE_TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<SaveRequest>();

        match std::thread::Builder::new()
            .name("openmeters-settings-saver".to_owned())
            .spawn(move || {
                // Debounce frequent settings changes (e.g., slider drags).
                const DEBOUNCE: Duration = Duration::from_millis(500);

                let mut last_written_json: Option<String> = None;

                while let Ok(mut req) = rx.recv() {
                    // Coalesce updates until we've been idle for DEBOUNCE.
                    loop {
                        match rx.recv_timeout(DEBOUNCE) {
                            Ok(next) => req = next,
                            Err(mpsc::RecvTimeoutError::Timeout) => break,
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    }

                    let mut data = req.data;
                    data.visuals.sanitize();

                    let json = match serde_json::to_string_pretty(&data) {
                        Ok(json) => json,
                        Err(err) => {
                            error!("[settings] failed to serialize UI settings for save: {err}");
                            continue;
                        }
                    };

                    if last_written_json.as_deref() == Some(&json) {
                        continue;
                    }

                    if let Err(err) = write_settings_atomic(&req.path, &json) {
                        error!("[settings] failed to persist UI settings: {err}");
                        continue;
                    }

                    last_written_json = Some(json);
                }
            }) {
            Ok(_) => Some(tx),
            Err(err) => {
                // Fall back to synchronous writes in enqueue_save.
                error!("[settings] failed to spawn background saver thread: {err}");
                None
            }
        }
    });

    let Some(tx) = tx.as_ref() else {
        // Extremely rare, but avoids losing settings if we cannot spawn threads.
        persist_settings_sync(&path, data);
        return;
    };

    if let Err(err) = tx.send(SaveRequest {
        path: path.clone(),
        data: data.clone(),
    }) {
        // If the receiver ever dies unexpectedly, fall back to a synchronous write.
        error!("[settings] failed to enqueue settings save: {err}");
        persist_settings_sync(&path, data);
    }
}

fn write_settings_atomic(path: &Path, json: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, json)?;
    fs::rename(&tmp_path, path)
}

#[derive(Debug, Clone)]
pub struct SettingsHandle {
    inner: Rc<RefCell<SettingsManager>>,
}

impl SettingsHandle {
    pub fn load_or_default() -> Self {
        Self {
            inner: Rc::new(RefCell::new(SettingsManager::load_or_default())),
        }
    }

    pub fn borrow(&self) -> Ref<'_, SettingsManager> {
        self.inner.borrow()
    }

    pub fn update<F, R>(&self, mutator: F) -> R
    where
        F: FnOnce(&mut SettingsManager) -> R,
    {
        let mut manager = self.inner.borrow_mut();
        let result = mutator(&mut manager);

        // Keep in-memory state immediately consistent, but debounce disk IO.
        manager.data.visuals.sanitize();
        enqueue_save(manager.path.clone(), manager.data.clone());
        result
    }
}
