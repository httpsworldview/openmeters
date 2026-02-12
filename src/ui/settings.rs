// Settings persistence and management.

use crate::{
    dsp::{
        oscilloscope::{OscilloscopeConfig, TriggerMode},
        spectrogram::{FrequencyScale, SpectrogramConfig, WindowKind},
        spectrum::{AveragingMode, SpectrumConfig},
        stereometer::StereometerConfig,
        waveform::WaveformConfig,
    },
    ui::{app::config::CaptureMode, theme, visualization::visual_manager::VisualKind},
};
use iced::Color;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    array,
    cell::{Ref, RefCell},
    collections::HashMap,
    fs,
    path::PathBuf,
    rc::Rc,
    sync::{OnceLock, mpsc},
    time::Duration,
};
use tracing::warn;

pub const BAR_MIN_HEIGHT: u32 = 24;
pub const BAR_MAX_HEIGHT: u32 = 800;
pub const BAR_DEFAULT_HEIGHT: u32 = 180;

pub fn clamp_bar_height(height: u32) -> u32 {
    height.clamp(BAR_MIN_HEIGHT, BAR_MAX_HEIGHT)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct UiSettings {
    pub visuals: VisualSettings,
    pub background_color: Option<ColorSetting>,
    pub decorations: bool,
    pub bar: BarSettings,
    pub capture_mode: CaptureMode,
    pub last_device_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VisualSettings {
    pub modules: HashMap<VisualKind, ModuleSettings>,
    pub order: Vec<VisualKind>,
}

impl VisualSettings {
    pub fn sanitize(&mut self) {
        macro_rules! check {
            ($($k:ident => $t:ty),+) => {|k: &VisualKind, m: &mut ModuleSettings| match k {
                $(VisualKind::$k => m.config.as_ref().is_none_or(|v| serde_json::from_value::<$t>(v.clone()).is_ok())),+
            }};
        }
        let valid = check!(Spectrogram => SpectrogramSettings, Spectrum => SpectrumSettings,
            Oscilloscope => OscilloscopeSettings, Waveform => WaveformSettings,
            Loudness => LoudnessSettings, Stereometer => StereometerSettings);
        self.modules.retain(valid);
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModuleSettings {
    pub enabled: Option<bool>,
    // Module configuration payload.
    //
    // Persisted format is `{ "enabled": bool?, "config": <json>? }`.
    config: Option<Value>,
}

impl ModuleSettings {
    pub fn with_config<T: Serialize>(config: &T) -> Self {
        Self {
            enabled: None,
            config: serde_json::to_value(config).ok(),
        }
    }
    pub fn set_config<T: Serialize>(&mut self, config: &T) {
        self.config = serde_json::to_value(config).ok();
    }
    pub fn parse_config<T: DeserializeOwned>(&self) -> Option<T> {
        self.config
            .as_ref()
            .and_then(|val| T::deserialize(val).ok())
    }
    pub fn config_or_default<T: DeserializeOwned + Default>(&self) -> T {
        self.config
            .as_ref()
            .and_then(|val| {
                T::deserialize(val)
                    .map_err(|err| warn!("[settings] config parse error: {err}"))
                    .ok()
            })
            .unwrap_or_default()
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
    fn from(Color { r, g, b, a }: Color) -> Self {
        Self { r, g, b, a }
    }
}
impl From<ColorSetting> for Color {
    fn from(ColorSetting { r, g, b, a }: ColorSetting) -> Self {
        Self { r, g, b, a }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct PaletteSettings {
    pub stops: Vec<ColorSetting>,
}

impl PaletteSettings {
    pub fn to_array<const N: usize>(&self) -> Option<[Color; N]> {
        (self.stops.len() == N).then(|| array::from_fn(|idx| self.stops[idx].into()))
    }
    // Returns `Some` only if colors differ from defaults (avoids persisting unchanged palettes).
    pub fn if_differs_from(colors: &[Color], defaults: &[Color]) -> Option<Self> {
        let differs = colors.len() == defaults.len()
            && colors
                .iter()
                .zip(defaults)
                .any(|(col, def)| !theme::colors_equal(*col, *def));
        differs.then(|| Self {
            stops: colors.iter().copied().map(Into::into).collect(),
        })
    }
}

pub trait HasPalette {
    fn palette(&self) -> Option<&PaletteSettings>;
    fn set_palette(&mut self, palette: Option<PaletteSettings>);
}

macro_rules! settings_enum {
    ($(#[$attr:meta])* $vis:vis enum $name:ident { $($(#[$var_attr:meta])* $variant:ident => $label:expr),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
        #[serde(rename_all = "snake_case")] $(#[$attr])*
        $vis enum $name { $($(#[$var_attr])* $variant,)+ }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(match self { $(Self::$variant => $label),+ })
            }
        }
    };
}

settings_enum!(pub enum BarAlignment {
    #[default]
    Top => "Top",
    Bottom => "Bottom",
});

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BarSettings {
    pub enabled: bool,
    pub alignment: BarAlignment,
    pub height: u32,
}

impl Default for BarSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            alignment: BarAlignment::default(),
            height: BAR_DEFAULT_HEIGHT,
        }
    }
}

macro_rules! visual_settings {
    (@has_palette $name:ident) => {
        impl HasPalette for $name {
            fn palette(&self) -> Option<&PaletteSettings> { self.palette.as_ref() }
            fn set_palette(&mut self, palette: Option<PaletteSettings>) { self.palette = palette; }
        }
    };
    ($name:ident from $config_ty:ty { $($field:ident : $ty:ty),* $(,)? } $(extra { $($extra:ident : $extra_ty:ty = $default:expr),* $(,)? })?) => {
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(default)]
        pub struct $name { $(pub $field: $ty,)* $($(pub $extra: $extra_ty,)*)? pub palette: Option<PaletteSettings> }
        impl Default for $name { fn default() -> Self { Self::from_config(&<$config_ty>::default()) } }
        impl $name {
            pub fn from_config(cfg: &$config_ty) -> Self { Self { $($field: cfg.$field,)* $($($extra: $default,)*)? palette: None } }
            pub fn apply_to(&self, cfg: &mut $config_ty) { $(cfg.$field = self.$field;)* }
        }
        visual_settings!(@has_palette $name);
    };
    ($name:ident { $($field:ident : $ty:ty = $default:expr),* $(,)? }) => {
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(default)]
        pub struct $name { $(pub $field: $ty,)* pub palette: Option<PaletteSettings> }
        impl Default for $name { fn default() -> Self { Self { $($field: $default,)* palette: None } } }
        visual_settings!(@has_palette $name);
    };
}

settings_enum!(pub enum ChannelMode {
    Both => "Left + Right", Left => "Left only", Right => "Right only", #[default] Mono => "Mono blend",
});

impl ChannelMode {
    // Returns output channel count for this mode.
    #[inline]
    pub fn output_channels(self, input_channels: usize) -> usize {
        match self {
            Self::Both => input_channels,
            _ => 1,
        }
    }
}
settings_enum!(pub enum StereometerMode  { Lissajous => "Lissajous", #[default] DotCloud => "Dot Cloud" });
settings_enum!(pub enum StereometerScale { Linear => "Linear", #[default] Exponential => "Exponential" });
settings_enum!(pub enum CorrelationMeterMode { Off => "Off", SingleBand => "Single Band", #[default] MultiBand => "Multi Band" });
settings_enum!(pub enum CorrelationMeterSide { Left => "Left", #[default] Right => "Right" });
settings_enum!(pub enum PianoRollOverlay { #[default] Off => "Off", Right => "Right", Left => "Left" });
settings_enum!(pub enum MeterMode {
    #[default] LufsShortTerm => "LUFS Short-term",
    LufsMomentary => "LUFS Momentary",
    RmsFast => "RMS Fast",
    RmsSlow => "RMS Slow",
    TruePeak => "True Peak",
});

impl MeterMode {
    pub const ALL: &'static [MeterMode] = &[
        MeterMode::LufsShortTerm,
        MeterMode::LufsMomentary,
        MeterMode::RmsFast,
        MeterMode::RmsSlow,
        MeterMode::TruePeak,
    ];

    pub fn unit_label(self) -> &'static str {
        match self {
            MeterMode::LufsShortTerm | MeterMode::LufsMomentary => "LUFS",
            MeterMode::RmsFast | MeterMode::RmsSlow | MeterMode::TruePeak => "dB",
        }
    }
}
settings_enum!(pub enum SpectrumDisplayMode { #[default] Line => "Line", Bar => "Bar" });
settings_enum!(pub enum SpectrumWeightingMode { #[default] AWeighted => "A-Weighted", Raw => "Raw" });
settings_enum!(pub enum WaveformColorMode { #[default] Frequency => "Frequency", Loudness => "Loudness", Static => "Static" });

visual_settings!(OscilloscopeSettings from OscilloscopeConfig {
    segment_duration: f32, trigger_mode: TriggerMode,
} extra { persistence: f32 = 0.0, channel_mode: ChannelMode = ChannelMode::default() });

visual_settings!(WaveformSettings from WaveformConfig {
    scroll_speed: f32,
} extra { channel_mode: ChannelMode = ChannelMode::default(), color_mode: WaveformColorMode = WaveformColorMode::default() });

visual_settings!(SpectrumSettings from SpectrumConfig {
    fft_size: usize, hop_size: usize, window: WindowKind, averaging: AveragingMode,
    frequency_scale: FrequencyScale, reverse_frequency: bool, show_grid: bool, show_peak_label: bool,
} extra {
    smoothing_radius: usize = 0, smoothing_passes: usize = 0,
    display_mode: SpectrumDisplayMode = SpectrumDisplayMode::default(),
    weighting_mode: SpectrumWeightingMode = SpectrumWeightingMode::default(),
    show_secondary_line: bool = true,
    bar_count: usize = 64,
    bar_gap: f32 = 0.2,
    highlight_threshold: f32 = 0.45,
});

visual_settings!(SpectrogramSettings from SpectrogramConfig {
    fft_size: usize, hop_size: usize, history_length: usize, window: WindowKind, frequency_scale: FrequencyScale,
    use_reassignment: bool,
    zero_padding_factor: usize, display_bin_count: usize,
    reassignment_max_correction_hz: f32,
} extra {
    floor_db: f32 = -96.0,
    piano_roll_overlay: PianoRollOverlay = PianoRollOverlay::default(),
});

visual_settings!(StereometerSettings from StereometerConfig {
    segment_duration: f32, target_sample_count: usize, correlation_window: f32,
} extra {
    persistence: f32 = 0.0, mode: StereometerMode = StereometerMode::default(),
    scale: StereometerScale = StereometerScale::default(), scale_range: f32 = 15.0, rotation: i8 = -1, flip: bool = true,
    correlation_meter: CorrelationMeterMode = CorrelationMeterMode::default(),
    correlation_meter_side: CorrelationMeterSide = CorrelationMeterSide::default(),
});

visual_settings!(LoudnessSettings {
    left_mode: MeterMode = MeterMode::TruePeak,
    right_mode: MeterMode = MeterMode::LufsShortTerm,
});

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
                    // Coalesce rapid updates by draining pending messages
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
