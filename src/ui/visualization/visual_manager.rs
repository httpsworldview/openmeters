//! Central owner of visual modules and their state.
use crate::audio::meter_tap::{self, MeterFormat};
use crate::dsp::ProcessorUpdate;
use crate::dsp::oscilloscope::OscilloscopeConfig;
use crate::ui::settings::{
    LoudnessSettings, ModuleSettings, OscilloscopeSettings, PaletteSettings, SpectrogramSettings,
    SpectrumSettings, StereometerSettings, VisualSettings, WaveformSettings,
};
use crate::ui::theme;
use crate::ui::visualization::loudness::{self, LoudnessMeterProcessor, LoudnessMeterState};
use crate::ui::visualization::oscilloscope::{self, OscilloscopeProcessor, OscilloscopeState};
use crate::ui::visualization::spectrogram::{self, SpectrogramProcessor, SpectrogramState};
use crate::ui::visualization::spectrum::{self, SpectrumProcessor, SpectrumState};
use crate::ui::visualization::stereometer::{self, StereometerProcessor, StereometerState};
use crate::ui::visualization::waveform::{
    self, WaveformProcessor as WaveformUiProcessor, WaveformState,
};
use crate::util::audio::DEFAULT_SAMPLE_RATE;
use iced::alignment::{Horizontal, Vertical};
use iced::widget::container;
use iced::{Element, Length};
use serde::{Deserialize, Serialize};
use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

fn resolve_palette<const N: usize>(
    stored: &Option<PaletteSettings>,
    default: &[iced::Color; N],
) -> [iced::Color; N] {
    stored
        .as_ref()
        .and_then(|p| p.to_array::<N>())
        .unwrap_or(*default)
}

fn rc_cell<T>(value: T) -> Rc<RefCell<T>> {
    Rc::new(RefCell::new(value))
}

fn centered<'a, M: 'static>(inner: Element<'a, M>) -> Element<'a, M> {
    container(inner)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .into()
}

struct Visual<P, S> {
    processor: P,
    state: S,
}

impl<P, S> Visual<P, S> {
    fn new(processor: P, state: S) -> Self {
        Self { processor, state }
    }
}

/// visual registration macro. figured i could try something fancy.
///
/// syntax: `Variant(StateType, ProcessorType) { ... }`
///
/// required:
/// - `name`: Display name
/// - `metadata`: `{ preferred_width, preferred_height, min_width?, max_width? }`
/// - `init`: Constructor returning `Visual<P, S>`
/// - `view`: `|state| -> Element` (use `centered(widget(state))` for common pattern)
/// - `ingest`: `|proc, state, samples, format| { ... }`
///
/// optional (require both if either present):
/// - `settings`: Settings type for persistence
/// - `apply`: `|proc, state, settings| { ... }`
/// - `export`: `|proc, state| -> Settings`
macro_rules! define_visuals {
    ($(
        $variant:ident($state_ty:ty, $processor_ty:ty) {
            name: $name:expr,
            metadata: { $($meta:tt)* },
            init: $init:expr,
            view: |$vs:ident| $view:expr,
            ingest: |$ip:ident, $is:ident, $sa:ident, $fm:ident| $ingest:expr
            $(, settings: $settings:ty, apply: |$ap:ident, $as:ident, $aset:ident| $apply:expr, export: |$ep:ident, $es:ident| $export:expr )?
        }
    ),* $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum VisualKind { $($variant),* }

        #[derive(Debug, Clone)]
        pub enum VisualContent { $($variant { state: $state_ty }),* }

        impl VisualContent {
            pub fn render<M: 'static>(&self, metadata: VisualMetadata) -> Element<'_, M> {
                let inner: Element<'_, M> = match self {
                    $(Self::$variant { state: $vs } => $view),*
                };
                let (w, h) = (
                    if metadata.fill_horizontal { Length::Fill } else { Length::Fixed(metadata.preferred_width) },
                    if metadata.fill_vertical { Length::Fill } else { Length::Fixed(metadata.preferred_height) },
                );
                container(inner)
                    .width(w)
                    .height(h)
                    .align_x(Horizontal::Center)
                    .align_y(Vertical::Center)
                    .into()
            }
        }

        const VISUAL_DESCRIPTORS: &[VisualDescriptor] = &[$(
            VisualDescriptor {
                kind: VisualKind::$variant,
                metadata: VisualMetadata { display_name: $name, $($meta)* ..VisualMetadata::DEFAULT },
                build: || Box::new($init),
            }
        ),*];

        $(
            impl VisualModule for Visual<$processor_ty, $state_ty> {
                fn ingest(&mut self, samples: &[f32], format: MeterFormat) {
                    let ($ip, $is, $sa, $fm) = (&mut self.processor, &mut self.state, samples, format);
                    $ingest
                }

                fn content(&self) -> VisualContent {
                    VisualContent::$variant { state: self.state.clone() }
                }

                fn apply_settings(&mut self, settings: &ModuleSettings) {
                    $(if let Some($aset) = settings.config::<$settings>() {
                        let ($ap, $as) = (&mut self.processor, &mut self.state);
                        $apply
                    })?
                }

                fn export_settings(&self) -> Option<ModuleSettings> {
                    $({
                        let ($ep, $es) = (&self.processor, &self.state);
                        let out: $settings = $export;
                        return Some(ModuleSettings::with_config(&out));
                    })?
                    #[allow(unreachable_code)]
                    None
                }
            }
        )*
    };
}

define_visuals! {
    Loudness(LoudnessMeterState, LoudnessMeterProcessor) {
        name: "Loudness Meter",
        metadata: { preferred_width: 140.0, preferred_height: 300.0, min_width: 80.0, max_width: 140.0, },
        init: Visual::new(LoudnessMeterProcessor::new(DEFAULT_SAMPLE_RATE), LoudnessMeterState::new()),
        view: |state| {
            container(loudness::widget_with_layout(state, 140.0, 300.0))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Horizontal::Center)
                .align_y(Vertical::Bottom)
                .into()
        },
        ingest: |p, s, samples, format| { s.apply_snapshot(&p.ingest(samples, format)); },
        settings: LoudnessSettings,
        apply: |_p, s, set| {
            s.set_modes(set.left_mode, set.right_mode);
            s.set_palette(&resolve_palette(&set.palette, &theme::DEFAULT_LOUDNESS_PALETTE));
        },
        export: |_p, s| {
            let mut out = LoudnessSettings::new(s.left_mode(), s.right_mode());
            out.palette = PaletteSettings::maybe_from_colors(s.palette(), &theme::DEFAULT_LOUDNESS_PALETTE);
            out
        }
    },

    Oscilloscope(Rc<RefCell<OscilloscopeState>>, OscilloscopeProcessor) {
        name: "Oscilloscope",
        metadata: { preferred_width: 150.0, preferred_height: 160.0, min_width: 100.0, },
        init: Visual::new(
            OscilloscopeProcessor::new(OscilloscopeConfig { sample_rate: DEFAULT_SAMPLE_RATE, ..Default::default() }),
            rc_cell(OscilloscopeState::new()),
        ),
        view: |s| centered(oscilloscope::widget(s)),
        ingest: |p, s, samples, fmt| {
            if let Some(snap) = p.ingest(samples, fmt) { s.borrow_mut().apply_snapshot(&snap); }
        },
        settings: OscilloscopeSettings,
        apply: |p, s, set| {
            let mut cfg = p.config();
            cfg.segment_duration = set.segment_duration;
            cfg.trigger_mode = set.trigger_mode;
            p.update_config(cfg);
            let mut st = s.borrow_mut();
            st.update_view_settings(set.persistence, set.channel_mode);
            st.set_palette(&resolve_palette(&set.palette, &theme::DEFAULT_OSCILLOSCOPE_PALETTE));
        },
        export: |p, s| {
            let cfg = p.config();
            let st = s.borrow();
            OscilloscopeSettings {
                segment_duration: cfg.segment_duration,
                trigger_mode: cfg.trigger_mode,
                persistence: st.persistence(),
                channel_mode: st.channel_mode(),
                palette: PaletteSettings::maybe_from_colors(st.palette(), &theme::DEFAULT_OSCILLOSCOPE_PALETTE),
            }
        }
    },

    Waveform(Rc<RefCell<WaveformState>>, WaveformUiProcessor) {
        name: "Waveform",
        metadata: { preferred_width: 220.0, preferred_height: 180.0, min_width: 220.0, },
        init: Visual::new(WaveformUiProcessor::new(DEFAULT_SAMPLE_RATE), rc_cell(WaveformState::new())),
        view: |s| centered(waveform::widget(s)),
        ingest: |p, s, samples, fmt| {
            WaveformUiProcessor::sync_capacity(s, p);
            if let Some(snap) = p.ingest(samples, fmt) { s.borrow_mut().apply_snapshot(snap); }
        },
        settings: WaveformSettings,
        apply: |p, s, set| {
            let mut cfg = p.config();
            set.apply_to(&mut cfg);
            p.update_config(cfg);
            WaveformUiProcessor::sync_capacity(s, p);
            let mut st = s.borrow_mut();
            st.set_palette(&resolve_palette(&set.palette, &theme::DEFAULT_WAVEFORM_PALETTE));
            st.set_channel_mode(set.channel_mode);
        },
        export: |p, s| {
            let st = s.borrow();
            let mut out = WaveformSettings::from_config(&p.config());
            out.channel_mode = st.channel_mode();
            out.palette = PaletteSettings::maybe_from_colors(st.palette(), &theme::DEFAULT_WAVEFORM_PALETTE);
            out
        }
    },

    Spectrogram(Rc<RefCell<SpectrogramState>>, SpectrogramProcessor) {
        name: "Spectrogram",
        metadata: { preferred_width: 320.0, preferred_height: 220.0, min_width: 300.0, },
        init: Visual::new(SpectrogramProcessor::new(DEFAULT_SAMPLE_RATE), rc_cell(SpectrogramState::new())),
        view: |s| centered(spectrogram::widget(s)),
        ingest: |p, s, samples, fmt| {
            if let ProcessorUpdate::Snapshot(upd) = p.ingest(samples, fmt) { s.borrow_mut().apply_update(&upd); }
        },
        settings: SpectrogramSettings,
        apply: |p, s, set| {
            let mut cfg = p.config();
            set.apply_to(&mut cfg);
            p.update_config(cfg);
            s.borrow_mut().set_palette(resolve_palette(&set.palette, &theme::DEFAULT_SPECTROGRAM_PALETTE));
        },
        export: |p, s| {
            let mut out = SpectrogramSettings::from_config(&p.config());
            out.palette = PaletteSettings::maybe_from_colors(&s.borrow().palette(), &theme::DEFAULT_SPECTROGRAM_PALETTE);
            out
        }
    },

    Spectrum(Rc<RefCell<SpectrumState>>, SpectrumProcessor) {
        name: "Spectrum analyzer",
        metadata: { preferred_width: 400.0, preferred_height: 180.0, min_width: 400.0, },
        init: Visual::new(SpectrumProcessor::new(DEFAULT_SAMPLE_RATE), rc_cell(SpectrumState::new())),
        view: |s| centered(spectrum::widget(s)),
        ingest: |p, s, samples, fmt| {
            if let Some(snap) = p.ingest(samples, fmt) { s.borrow_mut().apply_snapshot(&snap); }
        },
        settings: SpectrumSettings,
        apply: |p, s, set| {
            let mut cfg = p.config();
            set.apply_to(&mut cfg);
            p.update_config(cfg);
            let mut st = s.borrow_mut();
            st.set_palette(&resolve_palette(&set.palette, &theme::DEFAULT_SPECTRUM_PALETTE));
            let upd = p.config();
            let sty = st.style_mut();
            sty.frequency_scale = upd.frequency_scale;
            sty.reverse_frequency = upd.reverse_frequency;
            sty.smoothing_radius = set.smoothing_radius;
            sty.smoothing_passes = set.smoothing_passes;
            st.update_show_grid(upd.show_grid);
            st.update_show_peak_label(upd.show_peak_label);
        },
        export: |p, s| {
            let mut out = SpectrumSettings::from_config(&p.config());
            let st = s.borrow();
            out.palette = PaletteSettings::maybe_from_colors(&st.palette(), &theme::DEFAULT_SPECTRUM_PALETTE);
            let sty = st.style();
            out.smoothing_radius = sty.smoothing_radius;
            out.smoothing_passes = sty.smoothing_passes;
            out
        }
    },

    Stereometer(Rc<RefCell<StereometerState>>, StereometerProcessor) {
        name: "Stereometer",
        metadata: { preferred_width: 150.0, preferred_height: 220.0, min_width: 100.0, },
        init: Visual::new(StereometerProcessor::new(DEFAULT_SAMPLE_RATE), rc_cell(StereometerState::new())),
        view: |s| centered(stereometer::widget(s)),
        ingest: |p, s, samples, fmt| { s.borrow_mut().apply_snapshot(&p.ingest(samples, fmt)); },
        settings: StereometerSettings,
        apply: |p, s, set| {
            let mut cfg = p.config();
            set.apply_to(&mut cfg);
            p.update_config(cfg);
            let mut st = s.borrow_mut();
            st.update_view_settings(&set);
            st.set_palette(&resolve_palette(&set.palette, &theme::DEFAULT_STEREOMETER_PALETTE));
        },
        export: |p, s| {
            let st = s.borrow();
            let (persistence, mode, scale, scale_range, rotation, flip) = st.view_settings();
            let mut out = StereometerSettings::from_config(&p.config());
            out.persistence = persistence;
            out.mode = mode;
            out.scale = scale;
            out.scale_range = scale_range;
            out.rotation = rotation;
            out.flip = flip;
            out.palette = PaletteSettings::maybe_from_colors(&st.palette(), &theme::DEFAULT_STEREOMETER_PALETTE);
            out
        }
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VisualId(u32);

impl VisualId {
    fn next(counter: &mut u32) -> Self {
        let current = *counter;
        *counter = counter.saturating_add(1);
        VisualId(current)
    }
}

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

impl VisualMetadata {
    const DEFAULT: Self = Self {
        display_name: "",
        preferred_width: 200.0,
        preferred_height: 200.0,
        fill_horizontal: true,
        fill_vertical: true,
        min_width: 100.0,
        max_width: f32::INFINITY,
    };
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

impl From<&VisualEntry> for VisualSlotSnapshot {
    fn from(entry: &VisualEntry) -> Self {
        Self {
            id: entry.id,
            kind: entry.kind,
            enabled: entry.enabled,
            metadata: entry.metadata,
            content: entry.cached_content.clone(),
        }
    }
}

trait VisualModule {
    fn ingest(&mut self, samples: &[f32], format: MeterFormat);
    fn content(&self) -> VisualContent;
    fn apply_settings(&mut self, settings: &ModuleSettings);
    fn export_settings(&self) -> Option<ModuleSettings> {
        None
    }
}

struct VisualDescriptor {
    kind: VisualKind,
    metadata: VisualMetadata,
    build: fn() -> Box<dyn VisualModule>,
}

struct VisualEntry {
    id: VisualId,
    kind: VisualKind,
    enabled: bool,
    metadata: VisualMetadata,
    module: Box<dyn VisualModule>,
    cached_content: VisualContent,
}

impl VisualEntry {
    fn new(id: VisualId, descriptor: &VisualDescriptor) -> Self {
        let module = (descriptor.build)();
        let cached_content = module.content();
        Self {
            id,
            kind: descriptor.kind,
            enabled: false,
            metadata: descriptor.metadata,
            module,
            cached_content,
        }
    }

    fn apply_settings(&mut self, settings: &ModuleSettings) {
        if let Some(enabled) = settings.enabled {
            self.enabled = enabled;
        }
        self.module.apply_settings(settings);
        self.cached_content = self.module.content();
    }
}

pub struct VisualManager {
    entries: Vec<VisualEntry>,
    next_id: u32,
}

impl VisualManager {
    pub fn new() -> Self {
        let mut manager = Self {
            entries: Vec::with_capacity(VISUAL_DESCRIPTORS.len()),
            next_id: 1,
        };
        for descriptor in VISUAL_DESCRIPTORS {
            let id = VisualId::next(&mut manager.next_id);
            manager.entries.push(VisualEntry::new(id, descriptor));
        }
        manager
    }

    fn entry_mut_by_kind(&mut self, kind: VisualKind) -> Option<&mut VisualEntry> {
        self.entries.iter_mut().find(|entry| entry.kind == kind)
    }

    fn entry_by_kind(&self, kind: VisualKind) -> Option<&VisualEntry> {
        self.entries.iter().find(|entry| entry.kind == kind)
    }

    pub fn snapshot(&self) -> VisualSnapshot {
        VisualSnapshot {
            slots: self.entries.iter().map(VisualSlotSnapshot::from).collect(),
        }
    }

    pub fn module_settings(&self, kind: VisualKind) -> Option<ModuleSettings> {
        self.entry_by_kind(kind).map(|entry| {
            let mut snapshot = entry.module.export_settings().unwrap_or_default();
            if snapshot.enabled.is_none() {
                snapshot.enabled = Some(entry.enabled);
            }
            snapshot
        })
    }

    pub fn apply_module_settings(&mut self, kind: VisualKind, settings: &ModuleSettings) -> bool {
        if let Some(entry) = self.entry_mut_by_kind(kind) {
            entry.apply_settings(settings);
            true
        } else {
            false
        }
    }

    pub fn set_enabled_by_kind(&mut self, kind: VisualKind, enabled: bool) {
        if let Some(entry) = self.entry_mut_by_kind(kind)
            && entry.enabled != enabled
        {
            entry.enabled = enabled;
        }
    }

    pub fn apply_visual_settings(&mut self, settings: &VisualSettings) {
        for entry in &mut self.entries {
            if let Some(module_settings) = settings.modules.get(&entry.kind) {
                entry.apply_settings(module_settings);
            }
        }

        if !settings.order.is_empty() {
            let ordered_ids: Vec<_> = settings
                .order
                .iter()
                .filter_map(|kind| self.entry_by_kind(*kind).map(|e| e.id))
                .collect();
            if !ordered_ids.is_empty() {
                self.reorder(&ordered_ids);
            }
        }
    }

    pub fn reorder(&mut self, new_order: &[VisualId]) {
        if let [a, b] = new_order {
            let i = self.entries.iter().position(|e| e.id == *a);
            let j = self.entries.iter().position(|e| e.id == *b);
            if let (Some(i), Some(j)) = (i, j) {
                self.entries.swap(i, j);
            }
            return;
        }
        for (position, id) in new_order.iter().enumerate() {
            if position >= self.entries.len() {
                break;
            }
            let Some(current_index) = self.entries.iter().position(|e| e.id == *id) else {
                continue;
            };
            if current_index != position {
                self.entries.swap(position, current_index);
            }
        }
    }

    pub fn restore_position(&mut self, visual_id: VisualId, target_index: usize) {
        let Some(current_index) = self.entries.iter().position(|e| e.id == visual_id) else {
            return;
        };

        let target = target_index.min(self.entries.len().saturating_sub(1));

        if current_index == target {
            return;
        }

        let entry = self.entries.remove(current_index);
        self.entries.insert(target, entry);
    }

    pub fn ingest_samples(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }

        let format = meter_tap::current_format();
        for entry in &mut self.entries {
            if entry.enabled {
                entry.module.ingest(samples, format);
                entry.cached_content = entry.module.content();
            }
        }
    }
}

#[derive(Clone)]
pub struct VisualManagerHandle {
    inner: Rc<RefCell<VisualManager>>,
}

impl VisualManagerHandle {
    pub fn new(manager: VisualManager) -> Self {
        Self {
            inner: rc_cell(manager),
        }
    }

    pub fn borrow(&self) -> Ref<'_, VisualManager> {
        self.inner.borrow()
    }

    pub fn borrow_mut(&self) -> RefMut<'_, VisualManager> {
        self.inner.borrow_mut()
    }

    pub fn snapshot(&self) -> VisualSnapshot {
        self.inner.borrow().snapshot()
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
        let manager = VisualManager::new();
        let snapshot = manager.snapshot();

        assert_eq!(snapshot.slots.len(), VISUAL_DESCRIPTORS.len());

        for descriptor in VISUAL_DESCRIPTORS {
            let slot = snapshot
                .slots
                .iter()
                .find(|slot| slot.kind == descriptor.kind)
                .unwrap_or_else(|| {
                    panic!(
                        "{} slot missing from snapshot",
                        descriptor.metadata.display_name
                    )
                });

            assert!(
                !slot.enabled,
                "{} should be disabled by default",
                descriptor.metadata.display_name
            );
        }
    }
}
