// Central owner of visual modules and their state.
use crate::{
    audio::meter_tap::{self, MeterFormat},
    ui::{
        settings::{self as settings_cfg, ModuleSettings, PaletteSettings, VisualSettings},
        theme,
        visualization::{loudness, oscilloscope, spectrogram, spectrum, stereometer, waveform},
    },
    util::audio::DEFAULT_SAMPLE_RATE,
};
use iced::{Color, Element, Length, widget::container};
use serde::{Deserialize, Serialize};
use std::{
    cell::{Ref, RefCell, RefMut},
    rc::Rc,
};

type Shared<T> = Rc<RefCell<T>>;
fn shared<T>(val: T) -> Shared<T> {
    Rc::new(RefCell::new(val))
}
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
    (@export_palette $state:expr, $default:expr) => { PaletteSettings::if_differs_from($state, $default) };
    (@apply_config $proc:ident, $settings:ident) => {{ let mut c = $proc.config(); $settings.apply_to(&mut c); $proc.update_config(c) }};
    (@apply_palette $st:expr, $settings:ident, $default:expr) => { $st.set_palette(&resolve_palette(&$settings.palette, $default)) };
    ($($variant:ident($name:expr, $width:expr, $height:expr, $min_w:expr $(, max=$max_w:expr)?) =>
       $module:ident::$processor:ident, Shared<$module2:ident::$state:ident>;
       init($sr:ident) { $proc_init:expr, $state_init:expr }
       ingest($p:ident, $s:ident, $samples:ident, $fmt:ident) $ingest_body:expr;
       $settings_ty:ty, $default_palette:expr;
       apply($ap:ident, $as:ident, $aset:ident) $apply_body:expr;
       export($ep:ident, $es:ident) $export_body:expr;
    )*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum VisualKind { $($variant),* }

        #[derive(Clone)]
        pub struct VisualContent(VisualContentInner);

        #[derive(Clone)]
        enum VisualContentInner { $($variant(Shared<$module2::$state>)),* }

        impl VisualContent {
            fn new(inner: VisualContentInner) -> Self {
                Self(inner)
            }

            pub fn render<M: 'static>(&self, meta: VisualMetadata) -> Element<'_, M> {
                let elem: Element<'_, M> = match &self.0 { $(VisualContentInner::$variant(s) => $module::widget(s)),* };
                let (width, height) = (
                    if meta.fill_horizontal { Length::Fill } else { Length::Fixed(meta.preferred_width) },
                    if meta.fill_vertical { Length::Fill } else { Length::Fixed(meta.preferred_height) });
                container(elem).width(width).height(height).center(Length::Fill).into()
            }
        }

        impl std::fmt::Debug for VisualContent {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct("VisualContent").finish_non_exhaustive()
            }
        }
        const DESCRIPTORS: &[Descriptor] = &[$(Descriptor {
            kind: VisualKind::$variant,
            meta: VisualMetadata { display_name: $name, preferred_width: $width, preferred_height: $height,
                min_width: $min_w, $( max_width: $max_w, )? ..DEFAULT_METADATA },
            build: || { let $sr = DEFAULT_SAMPLE_RATE; Box::new(Visual { p: $proc_init, s: shared($state_init) }) },
        }),*];
        $(impl VisualModule for Visual<$module::$processor, Shared<$module2::$state>> {
            fn ingest(&mut self, $samples: &[f32], $fmt: MeterFormat) {
                let ($p, $s) = (&mut self.p, &mut self.s); $ingest_body
            }
            fn content(&self) -> VisualContent { VisualContent::new(VisualContentInner::$variant(self.s.clone())) }
            fn apply(&mut self, module_cfg: &ModuleSettings) {
                let $aset: $settings_ty = module_cfg.config_or_default();
                let ($ap, $as) = (&mut self.p, &mut self.s); $apply_body
            }
            fn export(&self) -> ModuleSettings {
                let ($ep, $es) = (&self.p, &self.s);
                ModuleSettings::with_config(&{ let out: $settings_ty = $export_body; out })
            }
        })*
    };
}

visuals! {
    Loudness("Loudness", 140.0, 300.0, 80.0, max=140.0) =>
        loudness::LoudnessProcessor, Shared<loudness::LoudnessState>;
        init(sr) { loudness::LoudnessProcessor::new(sr), loudness::LoudnessState::new() }
        ingest(p, s, sa, fm) if let Some(snap) = p.ingest(sa, fm) { s.borrow_mut().apply_snapshot(&snap); };
        settings_cfg::LoudnessSettings, &theme::loudness::COLORS;
        apply(_p, s, set) { let mut st = s.borrow_mut(); st.set_modes(set.left_mode, set.right_mode);
            visuals!(@apply_palette st, set, &theme::loudness::COLORS); };
        export(_p, s) { let st = s.borrow(); settings_cfg::LoudnessSettings { left_mode: st.left_mode(), right_mode: st.right_mode(),
            palette: visuals!(@export_palette st.palette(), &theme::loudness::COLORS) } };

    Oscilloscope("Oscilloscope", 150.0, 160.0, 100.0) =>
        oscilloscope::OscilloscopeProcessor, Shared<oscilloscope::OscilloscopeState>;
        init(sr) { oscilloscope::OscilloscopeProcessor::new(sr), oscilloscope::OscilloscopeState::new() }
        ingest(p, s, samples, fmt) if let Some(snap) = p.ingest(samples, fmt) { s.borrow_mut().apply_snapshot(&snap); };
        settings_cfg::OscilloscopeSettings, &theme::oscilloscope::COLORS;
        apply(p, s, set) { visuals!(@apply_config p, set); let mut st = s.borrow_mut();
            st.update_view_settings(set.persistence, set.channel_mode);
            visuals!(@apply_palette st, set, &theme::oscilloscope::COLORS); };
        export(p, s) { let st = s.borrow(); let mut out = settings_cfg::OscilloscopeSettings::from_config(&p.config());
            out.persistence = st.persistence(); out.channel_mode = st.channel_mode();
            out.palette = visuals!(@export_palette st.palette(), &theme::oscilloscope::COLORS); out };

    Waveform("Waveform", 220.0, 180.0, 220.0) =>
        waveform::WaveformProcessor, Shared<waveform::WaveformState>;
        init(sr) { waveform::WaveformProcessor::new(sr), waveform::WaveformState::new() }
        ingest(p, s, samples, fmt) { p.sync_capacity(s.borrow().desired_columns());
            if let Some(snap) = p.ingest(samples, fmt) { s.borrow_mut().apply_snapshot(&snap); } };
        settings_cfg::WaveformSettings, &theme::waveform::COLORS;
        apply(p, s, set) { visuals!(@apply_config p, set); p.sync_capacity(s.borrow().desired_columns());
            let mut st = s.borrow_mut(); st.set_channel_mode(set.channel_mode); st.set_color_mode(set.color_mode);
            visuals!(@apply_palette st, set, &theme::waveform::COLORS); };
        export(p, s) { let st = s.borrow(); let mut out = settings_cfg::WaveformSettings::from_config(&p.config());
            out.channel_mode = st.channel_mode(); out.color_mode = st.color_mode();
            out.palette = visuals!(@export_palette st.palette(), &theme::waveform::COLORS); out };

    Spectrogram("Spectrogram", 320.0, 220.0, 300.0) =>
        spectrogram::SpectrogramProcessor, Shared<spectrogram::SpectrogramState>;
        init(sr) { spectrogram::SpectrogramProcessor::new(sr), spectrogram::SpectrogramState::new() }
        ingest(p, s, samples, fmt) if let Some(snap) = p.ingest(samples, fmt) {
            s.borrow_mut().apply_snapshot(&snap); };
        settings_cfg::SpectrogramSettings, &theme::spectrogram::COLORS;
        apply(p, s, set) { visuals!(@apply_config p, set); let mut st = s.borrow_mut();
            visuals!(@apply_palette st, set, &theme::spectrogram::COLORS);
            st.set_piano_roll(set.show_piano_roll, set.piano_roll_side);
            st.set_floor_db(set.floor_db); };
        export(p, s) { let st = s.borrow(); let mut out = settings_cfg::SpectrogramSettings::from_config(&p.config());
            out.palette = visuals!(@export_palette &st.palette(), &theme::spectrogram::COLORS);
            out.show_piano_roll = st.piano_roll().is_some();
            out.piano_roll_side = st.piano_roll().unwrap_or_default();
            out.floor_db = st.floor_db(); out };

    Spectrum("Spectrum analyzer", 400.0, 180.0, 400.0) =>
        spectrum::SpectrumProcessor, Shared<spectrum::SpectrumState>;
        init(sr) { spectrum::SpectrumProcessor::new(sr), spectrum::SpectrumState::new() }
        ingest(p, s, samples, fmt) if let Some(snap) = p.ingest(samples, fmt) { s.borrow_mut().apply_snapshot(&snap); };
        settings_cfg::SpectrumSettings, &theme::spectrum::COLORS;
        apply(p, s, set) { visuals!(@apply_config p, set); let mut st = s.borrow_mut();
            visuals!(@apply_palette st, set, &theme::spectrum::COLORS);
            let style = st.style_mut(); style.frequency_scale = set.frequency_scale;
            style.reverse_frequency = set.reverse_frequency; style.smoothing_radius = set.smoothing_radius;
            style.smoothing_passes = set.smoothing_passes; style.highlight_threshold = set.highlight_threshold;
            style.display_mode = set.display_mode; style.weighting_mode = set.weighting_mode;
            style.show_secondary_line = set.show_secondary_line;
            style.bar_count = set.bar_count; style.bar_gap = set.bar_gap;
            st.update_show_grid(set.show_grid); st.update_show_peak_label(set.show_peak_label); };
        export(p, s) { let st = s.borrow(); let mut out = settings_cfg::SpectrumSettings::from_config(&p.config());
            out.palette = visuals!(@export_palette &st.palette(), &theme::spectrum::COLORS);
            out.smoothing_radius = st.style().smoothing_radius;
            out.smoothing_passes = st.style().smoothing_passes;
            out.highlight_threshold = st.style().highlight_threshold;
            out.display_mode = st.style().display_mode;
            out.weighting_mode = st.style().weighting_mode;
            out.show_secondary_line = st.style().show_secondary_line;
            out.bar_count = st.style().bar_count; out.bar_gap = st.style().bar_gap; out };

    Stereometer("Stereometer", 150.0, 220.0, 100.0) =>
        stereometer::StereometerProcessor, Shared<stereometer::StereometerState>;
        init(sr) { stereometer::StereometerProcessor::new(sr), stereometer::StereometerState::new() }
        ingest(p, s, samples, fmt) if let Some(snap) = p.ingest(samples, fmt) { s.borrow_mut().apply_snapshot(&snap); };
        settings_cfg::StereometerSettings, &theme::stereometer::COLORS;
        apply(p, s, set) { visuals!(@apply_config p, set); let mut st = s.borrow_mut();
            st.update_view_settings(&set); visuals!(@apply_palette st, set, &theme::stereometer::COLORS); };
        export(p, s) { let st = s.borrow(); let cfg = p.config();
            let mut out = st.export_settings();
            out.segment_duration = cfg.segment_duration;
            out.target_sample_count = cfg.target_sample_count;
            out.correlation_window = cfg.correlation_window;
            out.palette = visuals!(@export_palette &st.palette(), &theme::stereometer::COLORS); out };
}

struct Visual<P, S> {
    p: P,
    s: S,
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
    content: VisualContent,
}
impl Entry {
    fn new(id: VisualId, d: &Descriptor) -> Self {
        let m = (d.build)();
        Self {
            id,
            kind: d.kind,
            enabled: false,
            meta: d.meta,
            content: m.content(),
            module: m,
        }
    }
    fn apply(&mut self, s: &ModuleSettings) {
        if let Some(e) = s.enabled {
            self.enabled = e;
        }
        self.module.apply(s);
        self.content = self.module.content();
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
impl From<&Entry> for VisualSlotSnapshot {
    fn from(e: &Entry) -> Self {
        Self {
            id: e.id,
            kind: e.kind,
            enabled: e.enabled,
            metadata: e.meta,
            content: e.content.clone(),
        }
    }
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
        for d in DESCRIPTORS {
            let id = VisualId(m.next_id);
            m.next_id = m.next_id.saturating_add(1);
            m.entries.push(Entry::new(id, d));
        }
        m
    }
    fn by_kind(&self, k: VisualKind) -> Option<&Entry> {
        self.entries.iter().find(|e| e.kind == k)
    }
    fn by_kind_mut(&mut self, k: VisualKind) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.kind == k)
    }
    pub fn snapshot(&self) -> VisualSnapshot {
        VisualSnapshot {
            slots: self.entries.iter().map(Into::into).collect(),
        }
    }
    pub fn module_settings(&self, k: VisualKind) -> Option<ModuleSettings> {
        self.by_kind(k).map(|e| {
            let mut s = e.module.export();
            s.enabled.get_or_insert(e.enabled);
            s
        })
    }
    pub fn apply_module_settings(&mut self, k: VisualKind, s: &ModuleSettings) -> bool {
        self.by_kind_mut(k).is_some_and(|e| {
            e.apply(s);
            true
        })
    }
    pub fn set_enabled_by_kind(&mut self, k: VisualKind, enabled: bool) {
        if let Some(e) = self.by_kind_mut(k)
            && e.enabled != enabled
        {
            e.enabled = enabled;
        }
    }
    pub fn apply_visual_settings(&mut self, s: &VisualSettings) {
        for e in &mut self.entries {
            e.apply(s.modules.get(&e.kind).unwrap_or(&ModuleSettings::default()));
        }
        if !s.order.is_empty() {
            let ids: Vec<_> = s
                .order
                .iter()
                .filter_map(|k| self.by_kind(*k).map(|e| e.id))
                .collect();
            if !ids.is_empty() {
                self.reorder(&ids);
            }
        }
    }
    pub fn reorder(&mut self, order: &[VisualId]) {
        if let [a, b] = order {
            let (i, j) = (
                self.entries.iter().position(|e| e.id == *a),
                self.entries.iter().position(|e| e.id == *b),
            );
            if let (Some(i), Some(j)) = (i, j) {
                self.entries.swap(i, j);
            }
            return;
        }
        for (pos, id) in order.iter().enumerate() {
            if pos >= self.entries.len() {
                break;
            }
            if let Some(cur) = self.entries.iter().position(|e| e.id == *id)
                && cur != pos
            {
                self.entries.swap(pos, cur);
            }
        }
    }
    pub fn restore_position(&mut self, id: VisualId, target: usize) {
        let Some(cur) = self.entries.iter().position(|e| e.id == id) else {
            return;
        };
        let t = target.min(self.entries.len().saturating_sub(1));
        if cur != t {
            let e = self.entries.remove(cur);
            self.entries.insert(t, e);
        }
    }
    pub fn ingest_samples(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        let fmt = meter_tap::current_format();
        for e in &mut self.entries {
            if e.enabled {
                e.module.ingest(samples, fmt);
                e.content = e.module.content();
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_reflects_descriptor_defaults() {
        let m = VisualManager::new();
        let s = m.snapshot();
        assert_eq!(s.slots.len(), DESCRIPTORS.len());
        for d in DESCRIPTORS {
            let slot = s
                .slots
                .iter()
                .find(|s| s.kind == d.kind)
                .unwrap_or_else(|| panic!("{} missing", d.meta.display_name));
            assert!(!slot.enabled, "{} should be disabled", d.meta.display_name);
        }
    }
}
