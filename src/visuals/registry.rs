// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{loudness, oscilloscope, palettes, spectrogram, spectrum, stereometer, waveform};
pub use crate::domain::visuals::VisualKind;
use crate::{
    infra::pipewire::meter_tap::{self, MeterFormat},
    persistence::settings::{
        self as settings_cfg, ModuleSettings, PaletteSettings, ThemeFile, VisualSettings,
    },
    util::audio::DEFAULT_SAMPLE_RATE,
    util::color::{sanitize_stop_positions, sanitize_stop_spreads},
};
use iced::{Color, Element, Length, widget::container};
use std::{
    cell::{Ref, RefCell, RefMut},
    rc::Rc,
};

type Shared<T> = Rc<RefCell<T>>;

fn resolve_palette<const N: usize>(
    custom: &Option<PaletteSettings>,
    default: &[Color; N],
) -> [Color; N] {
    custom
        .as_ref()
        .and_then(PaletteSettings::to_array)
        .unwrap_or(*default)
}

// dear future me/future maintainers: I'm sorry for this macro.
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
        $st.set_palette(&resolve_palette(&$settings.palette, $default))
    };
    ($($variant:ident($name:expr, $width:expr, $height:expr, $min_w:expr $(, max=$max_w:expr)?) =>
       $module:ident :: $processor:ident, $state:ident;
       $settings_ty:ty, $default_palette:expr;
       $(pre_ingest($pip:ident, $pis:ident) $pre_ingest_body:expr;)?
       apply($ap:ident, $as:ident, $aset:ident) $apply_body:expr;
       export($ep:ident, $es:ident) $export_body:expr;
    )*) => {
        #[derive(Debug, Clone)]
        pub struct VisualContent(VisualContentInner);

        #[derive(Debug, Clone)]
        enum VisualContentInner { $($variant(Shared<$module::$state>)),* }

        impl VisualContent {
            pub fn render<M: 'static>(&self, meta: VisualMetadata) -> Element<'_, M> {
                let elem: Element<'_, M> = match &self.0 {
                    $(VisualContentInner::$variant(s) => $module::widget(s)),*
                };
                meta.wrap(elem)
            }
        }

        const DESCRIPTORS: &[Descriptor] = &[$(Descriptor {
            kind: VisualKind::$variant,
            meta: VisualMetadata {
                display_name: $name, preferred_width: $width, preferred_height: $height,
                min_width: $min_w, $( max_width: $max_w, )? ..DEFAULT_METADATA
            },
            build: || Box::new(Visual {
                processor: $module::$processor::new(DEFAULT_SAMPLE_RATE),
                state: Rc::new(RefCell::new($module::$state::new())),
            }),
        }),*];

        $(impl VisualModule for Visual<$module::$processor, Shared<$module::$state>> {
            fn ingest(&mut self, samples: &[f32], fmt: MeterFormat) {
                $({
                    let ($pip, $pis) = (&mut self.processor, &self.state);
                    $pre_ingest_body
                })?
                if let Some(snap) = self.processor.ingest(samples, fmt) {
                    self.state.borrow_mut().apply_snapshot(&snap);
                }
            }

            fn content(&self) -> VisualContent {
                VisualContent(VisualContentInner::$variant(self.state.clone()))
            }

            fn apply(&mut self, module_cfg: &ModuleSettings) {
                let $aset: $settings_ty = module_cfg.config_or_default();
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
    Loudness("Loudness", 140.0, 300.0, 80.0, max=140.0) =>
        loudness::LoudnessProcessor, LoudnessState;
        settings_cfg::LoudnessSettings, &palettes::loudness::COLORS;
        apply(_p, s, set) { let mut st = s.borrow_mut();
            st.left_mode = set.left_mode; st.right_mode = set.right_mode;
            visuals!(@apply_palette st, set, &palettes::loudness::COLORS); };
        export(_p, s) { let st = s.borrow(); settings_cfg::LoudnessSettings { left_mode: st.left_mode, right_mode: st.right_mode,
            palette: visuals!(@export_palette &st.palette, &palettes::loudness::COLORS) } };

    Oscilloscope("Oscilloscope", 150.0, 160.0, 100.0) =>
        oscilloscope::OscilloscopeProcessor, OscilloscopeState;
        settings_cfg::OscilloscopeSettings, &palettes::oscilloscope::COLORS;
        apply(p, s, set) { visuals!(@apply_config p, set); let mut st = s.borrow_mut();
            st.update_view_settings(set.persistence, set.channel_1, set.channel_2);
            visuals!(@apply_palette st, set, &palettes::oscilloscope::COLORS); };
        export(p, s) { let st = s.borrow(); let mut out = settings_cfg::OscilloscopeSettings::from_config(&p.config());
            out.persistence = st.persistence; out.channel_1 = st.channel_1; out.channel_2 = st.channel_2;
            out.palette = visuals!(@export_palette &st.style.colors, &palettes::oscilloscope::COLORS); out };

    Waveform("Waveform", 220.0, 180.0, 220.0) =>
        waveform::WaveformProcessor, WaveformState;
        settings_cfg::WaveformSettings, &palettes::waveform::COLORS;
        apply(p, s, set) { visuals!(@apply_config p, set);
            let mut st = s.borrow_mut(); st.set_channels(set.channel_1, set.channel_2);
            st.color_mode = set.color_mode; st.show_peak_history = set.show_peak_history;
            visuals!(@apply_palette st, set, &palettes::waveform::COLORS); };
        export(p, s) { let st = s.borrow(); let mut out = settings_cfg::WaveformSettings::from_config(&p.config());
            out.channel_1 = st.channel_1; out.channel_2 = st.channel_2; out.color_mode = st.color_mode;
            out.show_peak_history = st.show_peak_history;
            out.palette = visuals!(@export_palette &st.style.palette, &palettes::waveform::COLORS); out };

    Spectrogram("Spectrogram", 320.0, 220.0, 300.0) =>
        spectrogram::SpectrogramProcessor, SpectrogramState;
        settings_cfg::SpectrogramSettings, &palettes::spectrogram::COLORS;
        pre_ingest(p, s) {
            let vw = { s.borrow().view_width };
            if vw > 0 {
                let mut cfg = p.config();
                let tw = (vw as usize).min(8192);
                if cfg.history_length != tw {
                    cfg.history_length = tw;
                    p.update_config(cfg);
                }
            }
        };
        apply(p, s, set) { visuals!(@apply_config p, set); let mut st = s.borrow_mut();
            visuals!(@apply_palette st, set, &palettes::spectrogram::COLORS);
            let count = palettes::spectrogram::COLORS.len();
            st.set_stop_positions(&sanitize_stop_positions(
                set.palette.as_ref().and_then(|p| p.stop_positions.as_deref()), count));
            st.set_stop_spreads(&sanitize_stop_spreads(
                set.palette.as_ref().and_then(|p| p.stop_spreads.as_deref()), count));
            st.piano_roll_overlay = set.piano_roll_overlay;
            st.set_floor_db(set.floor_db);
            st.set_tilt_db(set.tilt_db);
            st.set_rotation(set.rotation); };
        export(p, s) { let st = s.borrow(); let mut out = settings_cfg::SpectrogramSettings::from_config(&p.config());
            out.palette = PaletteSettings::from_state(&st.palette, &palettes::spectrogram::COLORS, &st.stop_positions, &st.stop_spreads);
            out.piano_roll_overlay = st.piano_roll_overlay;
            out.floor_db = st.style.floor_db;
            out.tilt_db = st.style.tilt_db;
            out.rotation = st.rotation; out };

    Spectrum("Spectrum analyzer", 400.0, 180.0, 400.0) =>
        spectrum::SpectrumProcessor, SpectrumState;
        settings_cfg::SpectrumSettings, &palettes::spectrum::COLORS;
        apply(p, s, set) { visuals!(@apply_config p, set); let mut st = s.borrow_mut();
            visuals!(@apply_palette st, set, &palettes::spectrum::COLORS);
            let style = st.style_mut(); style.frequency_scale = set.frequency_scale;
            style.reverse_frequency = set.reverse_frequency; style.smoothing_radius = set.smoothing_radius;
            style.smoothing_passes = set.smoothing_passes; style.highlight_threshold = set.highlight_threshold;
            style.display_mode = set.display_mode; style.weighting_mode = set.weighting_mode;
            style.show_secondary_line = set.show_secondary_line;
            style.bar_count = set.bar_count; style.bar_gap = set.bar_gap;
            st.update_show_grid(set.show_grid); st.update_show_peak_label(set.show_peak_label); };
        export(p, s) { let st = s.borrow(); let style = st.style();
            let mut out = settings_cfg::SpectrumSettings::from_config(&p.config());
            out.palette = visuals!(@export_palette &style.spectrum_palette, &palettes::spectrum::COLORS);
            out.smoothing_radius = style.smoothing_radius;
            out.smoothing_passes = style.smoothing_passes;
            out.highlight_threshold = style.highlight_threshold;
            out.display_mode = style.display_mode;
            out.weighting_mode = style.weighting_mode;
            out.show_secondary_line = style.show_secondary_line;
            out.bar_count = style.bar_count; out.bar_gap = style.bar_gap; out };

    Stereometer("Stereometer", 150.0, 220.0, 100.0) =>
        stereometer::StereometerProcessor, StereometerState;
        settings_cfg::StereometerSettings, &palettes::stereometer::COLORS;
        apply(p, s, set) { visuals!(@apply_config p, set); let mut st = s.borrow_mut();
            st.update_view_settings(&set); visuals!(@apply_palette st, set, &palettes::stereometer::COLORS); };
        export(p, s) { let st = s.borrow(); let cfg = p.config();
            let mut out = st.export_settings();
            out.segment_duration = cfg.segment_duration;
            out.target_sample_count = cfg.target_sample_count;
            out.correlation_window = cfg.correlation_window;
            out.palette = visuals!(@export_palette &st.palette, &palettes::stereometer::COLORS); out };
}

struct Visual<P, S> {
    processor: P,
    state: S,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VisualId(u32);

#[derive(Debug, Clone, Copy)]
pub struct VisualMetadata {
    pub display_name: &'static str,
    pub preferred_width: f32,
    pub preferred_height: f32,
    pub fill_horizontal: bool,
    pub fill_vertical: bool,
    pub min_width: f32,
    pub max_width: f32,
}

const DEFAULT_METADATA: VisualMetadata = VisualMetadata {
    display_name: "",
    preferred_width: 200.0,
    preferred_height: 200.0,
    fill_horizontal: true,
    fill_vertical: true,
    min_width: 100.0,
    max_width: f32::INFINITY,
};

impl VisualMetadata {
    pub(crate) fn wrap<'a, M: 'static>(&self, elem: Element<'a, M>) -> Element<'a, M> {
        let length = |fill, preferred| {
            if fill {
                Length::Fill
            } else {
                Length::Fixed(preferred)
            }
        };
        container(elem)
            .width(length(self.fill_horizontal, self.preferred_width))
            .height(length(self.fill_vertical, self.preferred_height))
            .center(Length::Fill)
            .into()
    }
}

pub trait VisualModule {
    fn ingest(&mut self, samples: &[f32], format: MeterFormat);
    fn content(&self) -> VisualContent;
    fn apply(&mut self, settings: &ModuleSettings);
    fn export(&self) -> ModuleSettings;
}

struct Descriptor {
    kind: VisualKind,
    meta: VisualMetadata,
    build: fn() -> Box<dyn VisualModule>,
}

struct Entry {
    id: VisualId,
    kind: VisualKind,
    enabled: bool,
    meta: VisualMetadata,
    module: Box<dyn VisualModule>,
}
impl Entry {
    fn new(id: VisualId, descriptor: &Descriptor) -> Self {
        let module = (descriptor.build)();
        Self {
            id,
            kind: descriptor.kind,
            enabled: false,
            meta: descriptor.meta,
            module,
        }
    }
    fn apply_settings(&mut self, settings: &ModuleSettings) {
        if let Some(enabled) = settings.enabled {
            self.enabled = enabled;
        }
        self.module.apply(settings);
    }

    fn snapshot(&self) -> VisualSlotSnapshot {
        VisualSlotSnapshot {
            id: self.id,
            kind: self.kind,
            enabled: self.enabled,
            metadata: self.meta,
            content: self.module.content(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct VisualSnapshot {
    pub slots: Vec<VisualSlotSnapshot>,
}

#[derive(Debug, Clone)]
pub(crate) struct VisualSlotSnapshot {
    pub id: VisualId,
    pub kind: VisualKind,
    pub enabled: bool,
    pub metadata: VisualMetadata,
    pub content: VisualContent,
}

pub(crate) struct VisualManager {
    entries: Vec<Entry>,
    next_id: u32,
}
impl VisualManager {
    pub fn new() -> Self {
        let mut m = Self {
            entries: Vec::with_capacity(DESCRIPTORS.len()),
            next_id: 1,
        };
        for descriptor in DESCRIPTORS {
            let id = VisualId(m.next_id);
            m.next_id = m.next_id.saturating_add(1);
            m.entries.push(Entry::new(id, descriptor));
        }
        m
    }
    fn by_kind(&self, kind: VisualKind) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.kind == kind)
    }
    fn by_kind_mut(&mut self, kind: VisualKind) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|entry| entry.kind == kind)
    }
    fn entry_index(&self, id: VisualId) -> Option<usize> {
        self.entries.iter().position(|entry| entry.id == id)
    }
    fn move_entry_to(&mut self, id: VisualId, target: usize) {
        let Some(current) = self.entry_index(id) else {
            return;
        };
        let target = target.min(self.entries.len().saturating_sub(1));
        if current != target {
            let entry = self.entries.remove(current);
            self.entries.insert(target, entry);
        }
    }
    fn swap_entries(&mut self, first: VisualId, second: VisualId) {
        let (Some(first_index), Some(second_index)) =
            (self.entry_index(first), self.entry_index(second))
        else {
            return;
        };
        if first_index != second_index {
            self.entries.swap(first_index, second_index);
        }
    }
    pub fn snapshot(&self) -> VisualSnapshot {
        VisualSnapshot {
            slots: self.entries.iter().map(Entry::snapshot).collect(),
        }
    }
    pub fn module_settings(&self, kind: VisualKind) -> Option<ModuleSettings> {
        self.by_kind(kind).map(|entry| {
            let mut settings = entry.module.export();
            settings.enabled.get_or_insert(entry.enabled);
            settings
        })
    }
    pub fn apply_module_settings(&mut self, kind: VisualKind, settings: &ModuleSettings) -> bool {
        self.by_kind_mut(kind).is_some_and(|entry| {
            entry.apply_settings(settings);
            true
        })
    }
    pub fn set_enabled_by_kind(&mut self, kind: VisualKind, enabled: bool) {
        if let Some(entry) = self.by_kind_mut(kind)
            && entry.enabled != enabled
        {
            entry.enabled = enabled;
        }
    }
    pub fn apply_visual_settings(&mut self, settings: &VisualSettings) {
        let default_settings = ModuleSettings::default();
        for entry in &mut self.entries {
            entry.apply_settings(
                settings
                    .modules
                    .get(&entry.kind)
                    .unwrap_or(&default_settings),
            );
        }
        if !settings.order.is_empty() {
            let ids: Vec<_> = settings
                .order
                .iter()
                .filter_map(|kind| self.by_kind(*kind).map(|entry| entry.id))
                .collect();
            if !ids.is_empty() {
                self.reorder(&ids);
            }
        }
    }
    pub fn reorder(&mut self, order: &[VisualId]) {
        if let [first, second] = order {
            self.swap_entries(*first, *second);
            return;
        }

        for (position, id) in order.iter().copied().take(self.entries.len()).enumerate() {
            self.move_entry_to(id, position);
        }
    }
    pub fn restore_position(&mut self, id: VisualId, target: usize) {
        self.move_entry_to(id, target);
    }
    pub fn apply_theme(&mut self, theme: &ThemeFile) {
        for entry in &mut self.entries {
            let mut settings = entry.module.export();
            settings.override_palette(theme.palettes.get(&entry.kind));
            entry.module.apply(&settings);
        }
    }
    pub fn ingest_samples(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }

        let format = meter_tap::current_format();
        for entry in &mut self.entries {
            if entry.enabled {
                entry.module.ingest(samples, format);
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct VisualManagerHandle(Rc<RefCell<VisualManager>>);
impl VisualManagerHandle {
    pub fn new(m: VisualManager) -> Self {
        Self(Rc::new(RefCell::new(m)))
    }
    pub fn borrow(&self) -> Ref<'_, VisualManager> {
        self.0.borrow()
    }
    pub fn borrow_mut(&self) -> RefMut<'_, VisualManager> {
        self.0.borrow_mut()
    }
    pub fn snapshot(&self) -> VisualSnapshot {
        self.0.borrow().snapshot()
    }
}
impl std::fmt::Debug for VisualManagerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VisualManagerHandle")
            .finish_non_exhaustive()
    }
}
