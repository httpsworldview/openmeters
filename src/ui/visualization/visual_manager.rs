//! Central owner of visual modules and their state.
use crate::{
    audio::meter_tap::{self, MeterFormat},
    dsp::ProcessorUpdate,
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

        #[derive(Debug, Clone)]
        pub enum VisualContent { $($variant(Shared<$module2::$state>)),* }
        impl VisualContent {
            pub fn render<M: 'static>(&self, meta: VisualMetadata) -> Element<'_, M> {
                let elem: Element<'_, M> = match self { $(Self::$variant(s) => $module::widget(s)),* };
                let (width, height) = (
                    if meta.fill_horizontal { Length::Fill } else { Length::Fixed(meta.preferred_width) },
                    if meta.fill_vertical { Length::Fill } else { Length::Fixed(meta.preferred_height) });
                container(elem).width(width).height(height).center(Length::Fill).into()
            }
        }
        const DESCRIPTORS: &[Descriptor] = &[$(Descriptor {
            kind: VisualKind::$variant,
            meta: VisualMetadata { display_name: $name, preferred_width: $width, preferred_height: $height,
                min_width: $min_w, $( max_width: $max_w, )? ..DEFAULT_METADATA },
            build: || { let $sr = DEFAULT_SAMPLE_RATE; Box::new(Visual { p: $proc_init, s: shared($state_init) }) },
        }),*];
        $(impl Module for Visual<$module::$processor, Shared<$module2::$state>> {
            fn ingest(&mut self, $samples: &[f32], $fmt: MeterFormat) {
                let ($p, $s) = (&mut self.p, &mut self.s); $ingest_body
            }
            fn content(&self) -> VisualContent { VisualContent::$variant(self.s.clone()) }
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
    Loudness("Loudness Meter", 140.0, 300.0, 80.0, max=140.0) =>
        loudness::LoudnessMeterProcessor, Shared<loudness::LoudnessMeterState>;
        init(sr) { loudness::LoudnessMeterProcessor::new(sr), loudness::LoudnessMeterState::new() }
        ingest(p, s, sa, fm) { s.borrow_mut().apply_snapshot(&p.ingest(sa, fm)); };
        settings_cfg::LoudnessSettings, &theme::DEFAULT_LOUDNESS_PALETTE;
        apply(_p, s, set) { let mut st = s.borrow_mut(); st.set_modes(set.left_mode, set.right_mode);
            visuals!(@apply_palette st, set, &theme::DEFAULT_LOUDNESS_PALETTE); };
        export(_p, s) { let st = s.borrow(); settings_cfg::LoudnessSettings { left_mode: st.left_mode(), right_mode: st.right_mode(),
            palette: visuals!(@export_palette st.palette(), &theme::DEFAULT_LOUDNESS_PALETTE) } };

    Oscilloscope("Oscilloscope", 150.0, 160.0, 100.0) =>
        oscilloscope::OscilloscopeProcessor, Shared<oscilloscope::OscilloscopeState>;
        init(sr) { oscilloscope::OscilloscopeProcessor::with_sample_rate(sr), oscilloscope::OscilloscopeState::new() }
        ingest(p, s, samples, fmt) if let Some(snap) = p.ingest(samples, fmt) { s.borrow_mut().apply_snapshot(&snap); };
        settings_cfg::OscilloscopeSettings, &theme::DEFAULT_OSCILLOSCOPE_PALETTE;
        apply(p, s, set) { visuals!(@apply_config p, set); let mut st = s.borrow_mut();
            st.update_view_settings(set.persistence, set.channel_mode);
            visuals!(@apply_palette st, set, &theme::DEFAULT_OSCILLOSCOPE_PALETTE); };
        export(p, s) { let st = s.borrow(); let mut out = settings_cfg::OscilloscopeSettings::from_config(&p.config());
            out.persistence = st.persistence(); out.channel_mode = st.channel_mode();
            out.palette = visuals!(@export_palette st.palette(), &theme::DEFAULT_OSCILLOSCOPE_PALETTE); out };

    Waveform("Waveform", 220.0, 180.0, 220.0) =>
        waveform::WaveformProcessor, Shared<waveform::WaveformState>;
        init(sr) { waveform::WaveformProcessor::new(sr), waveform::WaveformState::new() }
        ingest(p, s, samples, fmt) { waveform::WaveformProcessor::sync_capacity(s, p);
            if let Some(snap) = p.ingest(samples, fmt) { s.borrow_mut().apply_snapshot(snap); } };
        settings_cfg::WaveformSettings, &theme::DEFAULT_WAVEFORM_PALETTE;
        apply(p, s, set) { visuals!(@apply_config p, set); waveform::WaveformProcessor::sync_capacity(s, p);
            let mut st = s.borrow_mut(); st.set_channel_mode(set.channel_mode);
            visuals!(@apply_palette st, set, &theme::DEFAULT_WAVEFORM_PALETTE); };
        export(p, s) { let st = s.borrow(); let mut out = settings_cfg::WaveformSettings::from_config(&p.config());
            out.channel_mode = st.channel_mode();
            out.palette = visuals!(@export_palette st.palette(), &theme::DEFAULT_WAVEFORM_PALETTE); out };

    Spectrogram("Spectrogram", 320.0, 220.0, 300.0) =>
        spectrogram::SpectrogramProcessor, Shared<spectrogram::SpectrogramState>;
        init(sr) { spectrogram::SpectrogramProcessor::new(sr), spectrogram::SpectrogramState::new() }
        ingest(p, s, samples, fmt) { if let ProcessorUpdate::Snapshot(update) = p.ingest(samples, fmt) {
            s.borrow_mut().apply_update(&update); }};
        settings_cfg::SpectrogramSettings, &theme::DEFAULT_SPECTROGRAM_PALETTE;
        apply(p, s, set) { visuals!(@apply_config p, set); let mut st = s.borrow_mut();
            st.set_palette(resolve_palette(&set.palette, &theme::DEFAULT_SPECTROGRAM_PALETTE));
            st.set_piano_roll(set.show_piano_roll, set.piano_roll_side); };
        export(p, s) { let st = s.borrow(); let mut out = settings_cfg::SpectrogramSettings::from_config(&p.config());
            out.palette = visuals!(@export_palette &st.palette(), &theme::DEFAULT_SPECTROGRAM_PALETTE);
            out.show_piano_roll = st.piano_roll().is_some();
            out.piano_roll_side = st.piano_roll().unwrap_or_default(); out };

    Spectrum("Spectrum analyzer", 400.0, 180.0, 400.0) =>
        spectrum::SpectrumProcessor, Shared<spectrum::SpectrumState>;
        init(sr) { spectrum::SpectrumProcessor::new(sr), spectrum::SpectrumState::new() }
        ingest(p, s, samples, fmt) if let Some(snap) = p.ingest(samples, fmt) { s.borrow_mut().apply_snapshot(&snap); };
        settings_cfg::SpectrumSettings, &theme::DEFAULT_SPECTRUM_PALETTE;
        apply(p, s, set) { visuals!(@apply_config p, set); let mut st = s.borrow_mut();
            visuals!(@apply_palette st, set, &theme::DEFAULT_SPECTRUM_PALETTE);
            let style = st.style_mut(); style.frequency_scale = set.frequency_scale;
            style.reverse_frequency = set.reverse_frequency; style.smoothing_radius = set.smoothing_radius;
            style.smoothing_passes = set.smoothing_passes;
            st.update_show_grid(set.show_grid); st.update_show_peak_label(set.show_peak_label); };
        export(p, s) { let st = s.borrow(); let mut out = settings_cfg::SpectrumSettings::from_config(&p.config());
            out.palette = visuals!(@export_palette &st.palette(), &theme::DEFAULT_SPECTRUM_PALETTE);
            out.smoothing_radius = st.style().smoothing_radius;
            out.smoothing_passes = st.style().smoothing_passes; out };

    Stereometer("Stereometer", 150.0, 220.0, 100.0) =>
        stereometer::StereometerProcessor, Shared<stereometer::StereometerState>;
        init(sr) { stereometer::StereometerProcessor::new(sr), stereometer::StereometerState::new() }
        ingest(p, s, samples, fmt) { s.borrow_mut().apply_snapshot(&p.ingest(samples, fmt)); };
        settings_cfg::StereometerSettings, &theme::DEFAULT_STEREOMETER_PALETTE;
        apply(p, s, set) { visuals!(@apply_config p, set); let mut st = s.borrow_mut();
            st.update_view_settings(&set); visuals!(@apply_palette st, set, &theme::DEFAULT_STEREOMETER_PALETTE); };
        export(p, s) { let st = s.borrow();
            let (persistence, mode, scale, scale_range, rotation, flip) = st.view_settings();
            let mut out = settings_cfg::StereometerSettings::from_config(&p.config());
            out.persistence = persistence; out.mode = mode; out.scale = scale; out.scale_range = scale_range;
            out.rotation = rotation; out.flip = flip;
            out.palette = visuals!(@export_palette &st.palette(), &theme::DEFAULT_STEREOMETER_PALETTE); out };
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

trait Module {
    fn ingest(&mut self, samples: &[f32], format: MeterFormat);
    fn content(&self) -> VisualContent;
    fn apply(&mut self, settings: &ModuleSettings);
    fn export(&self) -> ModuleSettings;
}

struct Descriptor {
    kind: VisualKind,
    meta: VisualMetadata,
    build: fn() -> Box<dyn Module>,
}

struct Entry {
    id: VisualId,
    kind: VisualKind,
    enabled: bool,
    meta: VisualMetadata,
    module: Box<dyn Module>,
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
pub struct VisualSnapshot {
    pub slots: Vec<VisualSlotSnapshot>,
}

#[derive(Debug, Clone)]
pub struct VisualSlotSnapshot {
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

pub struct VisualManager {
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
pub struct VisualManagerHandle(Rc<RefCell<VisualManager>>);
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
