// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{
    loudness,
    options::{CorrelationMeterMode, StereometerMode, WaveformColorMode, WaveformHistoryMode},
    oscilloscope, palettes,
    spectrogram::{self, processor::MAX_SPECTROGRAM_HISTORY_COLUMNS},
    spectrum, stereometer, waveform,
};
pub use crate::domain::visuals::VisualKind;
use crate::{
    dsp::AudioBlock,
    infra::pipewire::meter_tap::MeterFormat,
    persistence::settings::{
        self as settings_cfg, ModuleSettings, PaletteSettings, ThemeFile, VisualSettings,
    },
    util::audio::{Channel, DEFAULT_SAMPLE_RATE},
    util::color::{sanitize_stop_positions, sanitize_stop_spreads},
};
use iced::{Color, Element, Length, widget::container};
use std::{cell::RefCell, rc::Rc};

type Shared<T> = Rc<RefCell<T>>;

// too many stops -> keep first N
// too few stops -> copy provided, repeat last
fn resolve_palette<const N: usize>(
    custom: Option<&PaletteSettings>,
    default: &[Color; N],
) -> [Color; N] {
    let Some(custom) = custom else {
        return *default;
    };

    if let Some(colors) = custom.to_array() {
        return colors;
    }

    let mut colors = *default;
    let mut last = None;
    let mut used = 0;
    for (dst, stop) in colors.iter_mut().zip(&custom.stops) {
        let color = (*stop).into();
        *dst = color;
        last = Some(color);
        used += 1;
    }
    if let Some(color) = last
        && used < N
    {
        colors[used..].fill(color);
    }
    colors
}

macro_rules! visuals {
    (@export_palette $state:expr, $default:expr) => {
        PaletteSettings::if_differs_from($state, $default)
    };
    (@apply_config $proc:ident, $settings:ident) => {{
        let mut config = $proc.config();
        $settings.apply_to(&mut config);
        $proc.update_config(config)
    }};
    (@apply_palette $st:expr, $settings:ident, $default:expr) => {
        $st.set_palette(&resolve_palette($settings.palette.as_ref(), $default))
    };
    ($($variant:ident($default_width_basis:expr, $min_w:expr) =>
       $module:ident :: $processor:ident, $config:ident, $state:ident;
       $settings_ty:ty;
       $(pre_ingest($pip:ident, $pis:ident) $pre_ingest_body:expr;)?
       apply($ap:ident, $as:ident, $aset:ident) $apply_body:expr;
       export($ep:ident, $es:ident) $export_body:expr;
    )*) => {
        #[derive(Clone)]
        pub(crate) struct VisualContent(VisualContentInner);

        #[derive(Clone)]
        enum VisualContentInner {
            $($variant(Shared<$module::$state>)),*
        }

        impl VisualContent {
            pub(crate) fn render<M: 'static>(&self) -> Element<'_, M> {
                container(match &self.0 {
                    $(VisualContentInner::$variant(s) => $module::widget(s)),*
                })
                .width(Length::Fill)
                .height(Length::Fill)
                .center(Length::Fill)
                .into()
            }
        }

        const DESCRIPTORS: &[Descriptor] = &[$(Descriptor {
            kind: VisualKind::$variant,
            default_width_basis: $default_width_basis,
            min_width: $min_w,
            build: || Box::new(Visual {
                processor: $module::$processor::new($module::$config {
                    sample_rate: DEFAULT_SAMPLE_RATE,
                    ..Default::default()
                }),
                state: Rc::new(RefCell::new($module::$state::new())),
            }),
        }),*];

        $(impl VisualModule for Visual<$module::$processor, Shared<$module::$state>> {
            fn ingest(&mut self, samples: &[f32], fmt: MeterFormat) {
                $({
                    let ($pip, $pis) = (&mut self.processor, &self.state);
                    $pre_ingest_body
                })?
                if let Some(snap) = self.processor.process_block(&AudioBlock::new(
                    samples,
                    fmt.channels,
                    fmt.sample_rate,
                )) {
                    self.state.borrow_mut().apply_snapshot(snap);
                }
            }

            fn content(&self) -> VisualContent {
                VisualContent(VisualContentInner::$variant(self.state.clone()))
            }

            fn apply(&mut self, module_cfg: &ModuleSettings) {
                let $aset: $settings_ty = module_cfg.parse_config().unwrap_or_default();
                let ($ap, $as) = (&mut self.processor, &self.state);
                $apply_body
            }

            fn export(&self) -> ModuleSettings {
                let ($ep, $es) = (&self.processor, &self.state);
                ModuleSettings::with_config(&{ let out: $settings_ty = $export_body; out })
            }
        })*
    };
}

visuals! {
    Loudness(140.0, 80.0) =>
        loudness::LoudnessProcessor, LoudnessConfig, LoudnessState;
        settings_cfg::LoudnessSettings;
        apply(_p, s, set) { let mut st = s.borrow_mut();
            st.set_modes(set.left_mode, set.right_mode);
            visuals!(@apply_palette st, set, &palettes::loudness::COLORS); };
        export(_p, s) { let st = s.borrow(); let mut out = st.export_settings();
            out.palette = visuals!(@export_palette &st.palette, &palettes::loudness::COLORS); out };

    Oscilloscope(150.0, 100.0) =>
        oscilloscope::OscilloscopeProcessor, OscilloscopeConfig, OscilloscopeState;
        settings_cfg::OscilloscopeSettings;
        apply(p, s, set) { visuals!(@apply_config p, set); let reset = [set.channel_1, set.channel_2] == [Channel::None; 2];
            let mut st = s.borrow_mut(); st.update_view_settings(&set, reset);
            visuals!(@apply_palette st, set, &palettes::oscilloscope::COLORS); };
        export(p, s) { let st = s.borrow(); let mut out = st.export_settings(); out.sync_from_config(&p.config());
            out.palette = visuals!(@export_palette &st.colors, &palettes::oscilloscope::COLORS); out };

    Waveform(220.0, 220.0) =>
        waveform::WaveformProcessor, WaveformConfig, WaveformState;
        settings_cfg::WaveformSettings;
        pre_ingest(p, s) {
            let max_columns = s.borrow().view_columns().min(waveform::processor::MAX_COLUMN_CAPACITY);
            let mut cfg = p.config();
            if cfg.max_columns != max_columns {
                cfg.max_columns = max_columns;
                p.update_config(cfg);
            }
        };
        apply(p, s, set) {
            let mut cfg = p.config();
            set.apply_to(&mut cfg);
            cfg.track_history = set.history_mode != WaveformHistoryMode::Off;
            cfg.analyze_bands = set.color_mode == WaveformColorMode::Frequency || cfg.track_history;
            p.update_config(cfg);
            let mut st = s.borrow_mut(); st.update_view_settings(&set);
            visuals!(@apply_palette st, set, &palettes::waveform::COLORS); };
        export(p, s) { let st = s.borrow(); let mut out = st.export_settings(); out.sync_from_config(&p.config());
            out.palette = visuals!(@export_palette &st.style.palette, &palettes::waveform::COLORS); out };

    Spectrogram(320.0, 300.0) =>
        spectrogram::SpectrogramProcessor, SpectrogramConfig, SpectrogramState;
        settings_cfg::SpectrogramSettings;
        pre_ingest(p, s) {
            let vw = { s.borrow().view_width };
            if vw > 0 {
                let mut cfg = p.config();
                let tw = (vw as usize).min(MAX_SPECTROGRAM_HISTORY_COLUMNS);
                if cfg.history_length != tw {
                    cfg.history_length = tw;
                    p.update_config(cfg);
                }
            }
        };
        apply(p, s, set) { visuals!(@apply_config p, set); let mut st = s.borrow_mut();
            visuals!(@apply_palette st, set, &palettes::spectrogram::COLORS);
            st.set_stop_positions(&sanitize_stop_positions(
                set.palette.as_ref().and_then(|p| p.stop_positions.as_deref()),
                &palettes::spectrogram::DEFAULT_POSITIONS));
            st.set_stop_spreads(&sanitize_stop_spreads(
                set.palette.as_ref().and_then(|p| p.stop_spreads.as_deref()),
                palettes::spectrogram::COLORS.len()));
            st.update_view_settings(&set); };
        export(p, s) { let st = s.borrow(); let mut out = st.export_settings(); out.sync_from_config(&p.config());
            out.palette = PaletteSettings::from_state(&st.palette, &palettes::spectrogram::COLORS, &st.stop_positions, &palettes::spectrogram::DEFAULT_POSITIONS, &st.stop_spreads); out };

    Spectrum(400.0, 400.0) =>
        spectrum::SpectrumProcessor, SpectrumConfig, SpectrumState;
        settings_cfg::SpectrumSettings;
        apply(p, s, set) { visuals!(@apply_config p, set); let cfg = p.config(); let mut st = s.borrow_mut();
            st.update_view_settings(&set, cfg.floor_db);
            visuals!(@apply_palette st, set, &palettes::spectrum::COLORS); };
        export(p, s) { let st = s.borrow(); let mut out = st.export_settings(); out.sync_from_config(&p.config());
            out.palette = visuals!(@export_palette &st.spectrum_palette, &palettes::spectrum::COLORS); out };

    Stereometer(150.0, 100.0) =>
        stereometer::StereometerProcessor, StereometerConfig, StereometerState;
        settings_cfg::StereometerSettings;
        apply(p, s, set) {
            let mut cfg = p.config();
            set.apply_to(&mut cfg);
            cfg.emit_band_points = set.mode == StereometerMode::DotCloudBands;
            cfg.analyze_bands = cfg.emit_band_points
                || set.correlation_meter == CorrelationMeterMode::MultiBand;
            p.update_config(cfg);
            let mut st = s.borrow_mut();
            st.update_view_settings(&set);
            visuals!(@apply_palette st, set, &palettes::stereometer::COLORS);
        };
        export(p, s) { let st = s.borrow(); let mut out = st.export_settings(); out.sync_from_config(&p.config());
            out.palette = visuals!(@export_palette &st.palette, &palettes::stereometer::COLORS); out };
}

struct Visual<P, S> {
    processor: P,
    state: S,
}

pub trait VisualModule {
    fn ingest(&mut self, samples: &[f32], format: MeterFormat);
    fn content(&self) -> VisualContent;
    fn apply(&mut self, settings: &ModuleSettings);
    fn export(&self) -> ModuleSettings;
}

struct Descriptor {
    kind: VisualKind,
    default_width_basis: f32,
    min_width: f32,
    build: fn() -> Box<dyn VisualModule>,
}

struct Entry {
    descriptor: &'static Descriptor,
    enabled: bool,
    module: Box<dyn VisualModule>,
}
impl Entry {
    fn apply_settings(&mut self, settings: &ModuleSettings) {
        if let Some(enabled) = settings.enabled {
            self.enabled = enabled;
        }
        self.module.apply(settings);
    }
}

#[derive(Clone)]
pub(crate) struct VisualSlotSnapshot {
    pub kind: VisualKind,
    pub enabled: bool,
    pub default_width_basis: f32,
    pub min_width: f32,
    pub content: VisualContent,
}

pub(crate) struct VisualManager {
    entries: Vec<Entry>,
}
impl Default for VisualManager {
    fn default() -> Self {
        Self {
            entries: DESCRIPTORS
                .iter()
                .map(|descriptor| Entry {
                    descriptor,
                    enabled: false,
                    module: (descriptor.build)(),
                })
                .collect(),
        }
    }
}
impl VisualManager {
    fn position(&self, kind: VisualKind) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.descriptor.kind == kind)
    }
    pub fn move_to(&mut self, kind: VisualKind, target: usize) {
        let Some(current) = self.position(kind) else {
            return;
        };
        let target = target.min(self.entries.len().saturating_sub(1));
        if current != target {
            let entry = self.entries.remove(current);
            self.entries.insert(target, entry);
        }
    }
    pub fn snapshot(&self) -> Vec<VisualSlotSnapshot> {
        self.entries
            .iter()
            .map(|entry| VisualSlotSnapshot {
                kind: entry.descriptor.kind,
                enabled: entry.enabled,
                default_width_basis: entry.descriptor.default_width_basis,
                min_width: entry.descriptor.min_width,
                content: entry.module.content(),
            })
            .collect()
    }
    pub fn order(&self) -> Vec<VisualKind> {
        self.entries
            .iter()
            .map(|entry| entry.descriptor.kind)
            .collect()
    }
    pub fn module_settings(&self, kind: VisualKind) -> Option<ModuleSettings> {
        let entry = &self.entries[self.position(kind)?];
        let mut settings = entry.module.export();
        settings.enabled.get_or_insert(entry.enabled);
        Some(settings)
    }
    pub fn theme_palettes(&self) -> impl Iterator<Item = (VisualKind, PaletteSettings)> + '_ {
        self.entries.iter().filter_map(|entry| {
            entry
                .module
                .export()
                .extract_palette()
                .map(|palette| (entry.descriptor.kind, palette))
        })
    }
    pub fn apply_module_settings(&mut self, kind: VisualKind, settings: &ModuleSettings) {
        let index = self
            .position(kind)
            .expect("visual kind missing from registry");
        self.entries[index].apply_settings(settings);
    }
    pub fn set_enabled(&mut self, kind: VisualKind, enabled: bool) {
        if let Some(index) = self.position(kind) {
            self.entries[index].enabled = enabled;
        }
    }
    pub fn apply_visual_settings(&mut self, settings: &VisualSettings) {
        let default_settings = ModuleSettings::default();
        for entry in &mut self.entries {
            entry.apply_settings(
                settings
                    .modules
                    .get(&entry.descriptor.kind)
                    .unwrap_or(&default_settings),
            );
        }
        self.reorder(&settings.order);
    }
    pub fn reorder(&mut self, order: &[VisualKind]) {
        for (position, kind) in order.iter().copied().take(self.entries.len()).enumerate() {
            self.move_to(kind, position);
        }
    }
    pub fn apply_theme(&mut self, theme: &ThemeFile) {
        for entry in &mut self.entries {
            let mut settings = entry.module.export();
            settings.override_palette(theme.palettes.get(&entry.descriptor.kind));
            entry.module.apply(&settings);
        }
    }
    pub fn ingest_samples(&mut self, samples: &[f32], format: MeterFormat) {
        if samples.is_empty() {
            return;
        }

        for entry in &mut self.entries {
            if entry.enabled {
                entry.module.ingest(samples, format);
            }
        }
    }
}

pub(crate) type VisualManagerHandle = Shared<VisualManager>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_palettes_extend_with_last_stop() {
        let a = Color::from_rgb8(1, 2, 3);
        let b = Color::from_rgb8(4, 5, 6);
        let palette = PaletteSettings {
            stops: vec![a.into(), b.into()],
            ..Default::default()
        };

        assert_eq!(
            resolve_palette(Some(&palette), &[Color::BLACK; 4]),
            [a, b, b, b]
        );
    }
}
