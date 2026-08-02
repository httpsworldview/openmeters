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
    dsp::{AudioBlock, AudioFormat},
    persistence::settings::{
        self as settings_cfg, ModuleSettings, PaletteSettings, ThemeFile, VisualSettings,
    },
    util::audio::Channel,
    util::color::{sanitize_stop_positions, sanitize_stop_spreads},
};
use iced::{Color, Element};
use std::{cell::RefCell, rc::Rc};

type Shared<T> = Rc<RefCell<T>>;

trait PrepareProcessor {
    fn prepare(&mut self) {}
}

impl PrepareProcessor for loudness::LoudnessProcessor {}
impl PrepareProcessor for oscilloscope::OscilloscopeProcessor {}
impl PrepareProcessor for stereometer::StereometerProcessor {}
impl PrepareProcessor for spectrogram::SpectrogramProcessor {
    fn prepare(&mut self) {
        spectrogram::SpectrogramProcessor::prepare(self);
    }
}
impl PrepareProcessor for spectrum::SpectrumProcessor {
    fn prepare(&mut self) {
        spectrum::SpectrumProcessor::prepare(self);
    }
}
impl PrepareProcessor for waveform::WaveformProcessor {
    fn prepare(&mut self) {
        waveform::WaveformProcessor::prepare(self);
    }
}

// too many stops -> keep first N
// too few stops -> copy provided, repeat last
fn resolve_palette<const N: usize>(
    custom: Option<&PaletteSettings>,
    default: &[Color; N],
) -> [Color; N] {
    let Some(custom) = custom else {
        return *default;
    };
    let Some(last) = custom.stops.last() else {
        return *default;
    };

    let mut colors = *default;
    for (color, stop) in colors
        .iter_mut()
        .zip(custom.stops.iter().chain(std::iter::repeat(last)))
    {
        *color = (*stop).into();
    }
    colors
}

macro_rules! visuals {
    (@sync_export none, $out:ident, $processor:ident) => {};
    (@sync_export config, $out:ident, $processor:ident) => {
        $out.sync_from_config(&$processor.config());
    };
    (@export_palette spectrogram, $state:ident) => {
        PaletteSettings::from_state(
            &$state.palette,
            &palettes::spectrogram::COLORS,
            &$state.stop_positions,
            &palettes::spectrogram::DEFAULT_POSITIONS,
            &$state.stop_spreads,
        )
    };
    (@export_palette $module:ident, $state:ident) => {
        PaletteSettings::if_differs_from(&$state.palette, &palettes::$module::COLORS)
    };
    (@apply_config $proc:ident, $settings:ident) => {{
        let mut config = $proc.config();
        $settings.apply_to(&mut config);
        $proc.update_config(config)
    }};
    (@apply_palette spectrogram, $state:ident, $palette:ident) => {{
        $state.set_stop_positions(&sanitize_stop_positions(
            $palette.and_then(|palette| palette.stop_positions.as_deref()),
            &palettes::spectrogram::DEFAULT_POSITIONS,
        ));
        $state.set_stop_spreads(&sanitize_stop_spreads(
            $palette.and_then(|palette| palette.stop_spreads.as_deref()),
            palettes::spectrogram::SIZE,
        ));
    }};
    (@apply_palette $module:ident, $state:ident, $palette:ident) => {};
    ($($variant:ident($default_width_basis:expr, $min_w:expr) =>
       $module:ident :: $processor:ident, $config:ident, $state:ident.$state_settings:ident;
       $settings_ty:ty;
       $(pre_ingest($pip:ident, $pis:ident) $pre_ingest_body:expr;)?
       apply($ap:ident, $as:ident, $aset:ident) $apply_body:expr;
       export($ep:ident, $es:ident) $sync:ident;
    )*) => {
        #[derive(Clone)]
        pub(crate) struct VisualContent(VisualContentInner);

        #[derive(Clone)]
        enum VisualContentInner {
            $($variant(Shared<$module::$state>)),*
        }

        impl VisualContent {
            pub(crate) fn render<M: 'static>(&self) -> Element<'_, M> {
                match &self.0 {
                    $(VisualContentInner::$variant(s) => $module::widget(s)),*
                }
            }
        }

        const DESCRIPTORS: &[Descriptor] = &[$(Descriptor {
            kind: VisualKind::$variant,
            default_width_basis: $default_width_basis,
            min_width: $min_w,
            build: || Box::new(Visual {
                processor: $module::$processor::new($module::$config::default()),
                state: Rc::new(RefCell::new($module::$state::new())),
            }),
        }),*];

        $(impl VisualModule for Visual<$module::$processor, Shared<$module::$state>> {
            fn ingest(&mut self, block: &AudioBlock<'_>) {
                $({
                    let ($pip, $pis) = (&mut self.processor, &self.state);
                    $pre_ingest_body
                })?
                if let Some(snap) = self.processor.process_block(block) {
                    self.state.borrow_mut().apply_snapshot(snap);
                }
            }

            fn reset_audio(&mut self) {
                self.processor.reset_audio();
                self.state.borrow_mut().reset_audio();
            }

            fn prepare(&mut self) {
                PrepareProcessor::prepare(&mut self.processor);
            }

            fn content(&self) -> VisualContent {
                VisualContent(VisualContentInner::$variant(self.state.clone()))
            }

            fn apply(&mut self, module_cfg: &ModuleSettings) {
                let $aset: $settings_ty = module_cfg.parse_config();
                let ($ap, $as) = (&mut self.processor, &self.state);
                $apply_body
                self.apply_palette($aset.palette.as_ref());
            }

            fn export(&self) -> ModuleSettings {
                let ($ep, $es) = (&self.processor, &self.state);
                let st = $es.borrow();
                let mut out: $settings_ty = st.$state_settings.clone();
                visuals!(@sync_export $sync, out, $ep);
                out.palette = visuals!(@export_palette $module, st);
                ModuleSettings::with_config(&out)
            }

            fn export_palette(&self) -> Option<PaletteSettings> {
                let st = self.state.borrow();
                visuals!(@export_palette $module, st)
            }

            fn apply_palette(&mut self, palette: Option<&PaletteSettings>) {
                let mut state = self.state.borrow_mut();
                state.set_palette(&resolve_palette(palette, &palettes::$module::COLORS));
                visuals!(@apply_palette $module, state, palette);
            }
        })*
    };
}

visuals! {
    Loudness(140.0, 80.0) =>
        loudness::LoudnessProcessor, LoudnessConfig, LoudnessState.settings;
        settings_cfg::LoudnessSettings;
        apply(_p, s, set) {
            s.borrow_mut().set_modes(set.left_mode, set.right_mode);
        };
        export(_p, s) none;

    Oscilloscope(150.0, 100.0) =>
        oscilloscope::OscilloscopeProcessor, OscilloscopeConfig, OscilloscopeState.settings;
        settings_cfg::OscilloscopeSettings;
        apply(p, s, set) { visuals!(@apply_config p, set); let reset = [set.channel_1, set.channel_2] == [Channel::None; 2];
            s.borrow_mut().update_view_settings(&set, reset);
        };
        export(p, s) config;

    Waveform(220.0, 220.0) =>
        waveform::WaveformProcessor, WaveformConfig, WaveformState.settings;
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
            s.borrow_mut().update_view_settings(&set);
        };
        export(p, s) config;

    Spectrogram(320.0, 300.0) =>
        spectrogram::SpectrogramProcessor, SpectrogramConfig, SpectrogramState.settings;
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
        apply(p, s, set) { visuals!(@apply_config p, set);
            s.borrow_mut().update_view_settings(&set); };
        export(p, s) config;

    Spectrum(400.0, 400.0) =>
        spectrum::SpectrumProcessor, SpectrumConfig, SpectrumState.style;
        settings_cfg::SpectrumSettings;
        apply(p, s, set) { visuals!(@apply_config p, set); let cfg = p.config();
            s.borrow_mut().update_view_settings(&set, cfg.floor_db);
        };
        export(p, s) config;

    Stereometer(150.0, 100.0) =>
        stereometer::StereometerProcessor, StereometerConfig, StereometerState.settings;
        settings_cfg::StereometerSettings;
        apply(p, s, set) {
            let mut cfg = p.config();
            set.apply_to(&mut cfg);
            cfg.emit_band_points = set.mode == StereometerMode::DotCloudBands;
            cfg.analyze_bands = cfg.emit_band_points
                || set.correlation_meter == CorrelationMeterMode::MultiBand;
            p.update_config(cfg);
            s.borrow_mut().update_view_settings(&set);
        };
        export(p, s) config;
}

struct Visual<P, S> {
    processor: P,
    state: S,
}

pub trait VisualModule {
    fn ingest(&mut self, block: &AudioBlock<'_>);
    fn reset_audio(&mut self);
    fn prepare(&mut self);
    fn content(&self) -> VisualContent;
    fn apply(&mut self, settings: &ModuleSettings);
    fn export(&self) -> ModuleSettings;
    fn export_palette(&self) -> Option<PaletteSettings>;
    fn apply_palette(&mut self, palette: Option<&PaletteSettings>);
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
        if self.enabled {
            self.module.prepare();
        }
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if enabled {
            self.module.prepare();
        }
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
    format_generation: Option<u64>,
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
            format_generation: None,
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
    pub fn module_settings(&self, kind: VisualKind) -> ModuleSettings {
        let entry = &self.entries[self
            .position(kind)
            .expect("visual kind missing from registry")];
        let mut settings = entry.module.export();
        settings.enabled.get_or_insert(entry.enabled);
        settings
    }
    pub fn theme_palettes(&self) -> impl Iterator<Item = (VisualKind, PaletteSettings)> + '_ {
        self.entries.iter().filter_map(|entry| {
            entry
                .module
                .export_palette()
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
            self.entries[index].set_enabled(enabled);
        }
    }
    pub fn has_enabled(&self) -> bool {
        self.entries.iter().any(|entry| entry.enabled)
    }
    pub fn reset_audio(&mut self) {
        self.format_generation = None;
        for entry in &mut self.entries {
            entry.module.reset_audio();
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
            entry
                .module
                .apply_palette(theme.palettes.get(&entry.descriptor.kind));
        }
    }
    pub fn ingest_samples(&mut self, samples: &[f32], format: AudioFormat) {
        if samples.is_empty() {
            return;
        }
        if self
            .format_generation
            .replace(format.generation)
            .is_some_and(|generation| generation != format.generation)
        {
            for entry in &mut self.entries {
                entry.module.reset_audio();
            }
        }
        let block = AudioBlock::with_positions(
            samples,
            format.channels,
            format.sample_rate,
            format.positions,
        );
        for entry in &mut self.entries {
            if entry.enabled {
                entry.module.ingest(&block);
            }
        }
    }
}

pub(crate) type VisualManagerHandle = Shared<VisualManager>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palettes_fit_the_visual_stop_count() {
        let stops = [1, 2, 3, 4, 5].map(|value| Color::from_rgb8(value, value, value));
        let defaults = [Color::BLACK; 4];

        assert_eq!(resolve_palette(None, &defaults), defaults);
        for (len, expected) in [
            (0, defaults),
            (2, [stops[0], stops[1], stops[1], stops[1]]),
            (5, [stops[0], stops[1], stops[2], stops[3]]),
        ] {
            let palette = PaletteSettings {
                stops: stops[..len].iter().copied().map(Into::into).collect(),
                ..Default::default()
            };
            assert_eq!(resolve_palette(Some(&palette), &defaults), expected);
        }
    }
}
