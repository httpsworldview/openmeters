// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{loudness, oscilloscope, palettes, spectrogram, spectrum, stereometer, waveform};
pub use crate::domain::visuals::VisualKind;
use crate::{
    dsp::{AudioBlock, AudioFormat},
    persistence::settings::{PaletteSettings, ThemeFile, VisualConfig, VisualSettings},
};
use iced::Element;
use std::{cell::RefCell, rc::Rc};

type Shared<T> = Rc<RefCell<T>>;

macro_rules! visuals {
    ($($variant:ident($default_width_basis:expr, $min_w:expr) =>
       $module:ident :: $processor:ident, $state:ident;
       $(palette_ramp($positions:ident, $spreads:ident);)?
       $(prepare($prepare:ident);)?
       $(ignores_audio($ignores:ident);)?
       $(buffered_signal($buffered_signal:ident);)?
       $(pre_ingest($pip:ident, $pis:ident) $pre_ingest_body:block;)?
       $(config($cfg:ident) $($configure:block)?;)?
       apply($ap:ident, $as:ident, $aset:ident) $apply_body:block;
    )*) => {
        #[derive(Clone)]
        pub(crate) enum VisualContent {
            $($variant(Shared<$module::state::$state>)),*
        }

        impl VisualContent {
            pub(crate) fn render<M: 'static>(&self) -> Element<'_, M> {
                match self {
                    $(Self::$variant(s) => $module::state::widget(s)),*
                }
            }
        }

        const DEFAULT_ORDER: &[VisualKind] = &[$(VisualKind::$variant),*];

        fn default_width_basis(kind: VisualKind) -> f32 {
            match kind {
                $(VisualKind::$variant => $default_width_basis,)*
            }
        }

        fn entries() -> Vec<Entry> {
            vec![$({
                let state = Rc::new(RefCell::new($module::state::$state::default()));
                Entry {
                    slot: VisualSlotSnapshot {
                        kind: VisualKind::$variant,
                        enabled: false,
                        width_basis: $default_width_basis,
                        min_width: $min_w,
                        content: VisualContent::$variant(state.clone()),
                    },
                    module: Box::new(Visual {
                        processor: $module::processor::$processor::new(Default::default()),
                        state,
                        pending_audio: false,
                    }),
                }
            }),*]
        }

        $(impl VisualModule for Visual<$module::processor::$processor, Shared<$module::state::$state>> {
            fn ingest(&mut self, block: &AudioBlock<'_>, signal: bool) {
                $({
                    let ($pip, $pis) = (&mut self.processor, &self.state);
                    $pre_ingest_body
                })?
                self.pending_audio |= signal;
                if let Some(snap) = self.processor.process_block(block).into() {
                    self.state.borrow_mut().apply_snapshot(snap);
                    if !signal $(&& !self.processor.$buffered_signal())? {
                        self.pending_audio = false;
                    }
                }
            }

            fn reset_audio(&mut self) {
                self.processor.reset_audio();
                self.state.borrow_mut().reset_audio();
                self.pending_audio = true;
            }

            fn is_quiescent(&self) -> bool {
                let state = self.state.borrow();
                (!self.pending_audio $(|| state.$ignores())?) && state.is_quiescent()
            }

            fn prepare(&mut self) {
                $(self.processor.$prepare();)?
            }

            fn apply(&mut self, config: &VisualConfig) {
                let VisualConfig::$variant($aset) = config else {
                    unreachable!("config routed to the wrong visual");
                };
                let ($ap, $as) = (&mut self.processor, &self.state);
                $({
                    let mut $cfg = $ap.config();
                    $aset.apply_to(&mut $cfg);
                    $($configure)?
                    $ap.update_config($cfg);
                })?
                $apply_body
                self.pending_audio = true;
            }

            fn apply_palette(&mut self, palette: Option<&PaletteSettings>) {
                let mut state = self.state.borrow_mut();
                state.set_palette(&PaletteSettings::colors(palette, &palettes::$module::COLORS));
                $(
                    let ramp = &palettes::$module::DEFAULT_POSITIONS;
                    state.$positions.copy_from_slice(&PaletteSettings::positions(palette, ramp));
                    state.$spreads.copy_from_slice(&PaletteSettings::spreads(palette, ramp.len()));
                )?
            }
        })*
    };
}

visuals! {
    Spectrogram(320.0, 300.0) =>
        spectrogram::SpectrogramProcessor, SpectrogramState;
        palette_ramp(stop_positions, stop_spreads);
        prepare(prepare);
        buffered_signal(has_buffered_signal);
        pre_ingest(p, s) {
            let view_width = s.borrow().view_width;
            let history_length = view_width as usize;
            let mut cfg = p.config();
            if view_width > 0 && cfg.history_length != history_length {
                cfg.history_length = history_length;
                p.update_config(cfg);
            }
        };
        config(cfg);
        apply(p, s, set) {
            s.borrow_mut().update_view_settings(set);
        };

    Spectrum(400.0, 100.0) =>
        spectrum::SpectrumProcessor, SpectrumState;
        palette_ramp(stop_positions, stop_spreads);
        prepare(prepare);
        ignores_audio(ignores_audio);
        buffered_signal(has_buffered_signal);
        config(cfg);
        apply(p, s, set) {
            s.borrow_mut().update_view_settings(set, p.snapshot());
        };

    Waveform(220.0, 220.0) =>
        waveform::WaveformProcessor, WaveformState;
        prepare(prepare);
        pre_ingest(p, s) {
            let max_columns = s.borrow().view_columns();
            let mut cfg = p.config();
            if cfg.max_columns != max_columns {
                cfg.max_columns = max_columns;
                p.update_config(cfg);
            }
        };
        config(cfg) {
            cfg.bands = set.bands();
        };
        apply(p, s, set) {
            s.borrow_mut().update_view_settings(set);
        };

    Oscilloscope(150.0, 100.0) =>
        oscilloscope::OscilloscopeProcessor, OscilloscopeState;
        ignores_audio(ignores_audio);
        config(cfg);
        apply(p, s, set) {
            s.borrow_mut().update_view_settings(set);
        };

    Stereometer(150.0, 100.0) =>
        stereometer::StereometerProcessor, StereometerState;
        config(cfg) {
            cfg.bands = set.bands();
        };
        apply(p, s, set) {
            s.borrow_mut().update_view_settings(set);
        };

    Loudness(140.0, 140.0) =>
        loudness::LoudnessProcessor, LoudnessState;
        apply(_p, s, set) {
            s.borrow_mut().set_modes(set.left_mode, set.right_mode);
        };
}

struct Visual<P, S> {
    processor: P,
    state: S,
    pending_audio: bool,
}

trait VisualModule {
    fn ingest(&mut self, block: &AudioBlock<'_>, signal: bool);
    fn reset_audio(&mut self);
    fn is_quiescent(&self) -> bool;
    fn prepare(&mut self);
    fn apply(&mut self, config: &VisualConfig);
    fn apply_palette(&mut self, palette: Option<&PaletteSettings>);
}

struct Entry {
    slot: VisualSlotSnapshot,
    module: Box<dyn VisualModule>,
}
impl Entry {
    fn apply_config(&mut self, config: VisualConfig) {
        self.module.apply(&config);
        if self.slot.enabled {
            self.module.prepare();
        }
    }

    // Disabled visuals hold no audio, nor the GPU history tied to it.
    fn set_enabled(&mut self, enabled: bool) {
        if std::mem::replace(&mut self.slot.enabled, enabled) != enabled {
            self.module.reset_audio();
            if enabled {
                self.module.prepare();
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct VisualSlotSnapshot {
    pub kind: VisualKind,
    pub enabled: bool,
    pub width_basis: f32,
    pub min_width: f32,
    pub content: VisualContent,
}

crate::macros::default_struct! {
    pub(crate) struct VisualManager {
        entries: Vec<Entry> = entries(),
        format_generation: Option<u64> = None,
    }
}
impl VisualManager {
    fn position(&self, kind: VisualKind) -> usize {
        self.entries
            .iter()
            .position(|entry| entry.slot.kind == kind)
            .expect("visual kind missing from registry")
    }
    pub fn snapshot(&self) -> Vec<VisualSlotSnapshot> {
        self.entries
            .iter()
            .map(|entry| entry.slot.clone())
            .collect()
    }
    pub fn order(&self) -> Vec<VisualKind> {
        self.entries.iter().map(|entry| entry.slot.kind).collect()
    }
    // Outside tests, settings drive visuals through `apply_visual_settings`.
    #[cfg(test)]
    pub fn apply_config(&mut self, config: VisualConfig) {
        let index = self.position(config.kind());
        self.entries[index].apply_config(config);
    }
    #[cfg(test)]
    pub fn set_enabled(&mut self, kind: VisualKind, enabled: bool) {
        let index = self.position(kind);
        self.entries[index].set_enabled(enabled);
    }
    pub fn apply_palette(&mut self, kind: VisualKind, palette: Option<&PaletteSettings>) {
        let index = self.position(kind);
        self.entries[index].module.apply_palette(palette);
    }
    pub fn has_enabled(&self) -> bool {
        self.entries.iter().any(|entry| entry.slot.enabled)
    }
    pub fn is_quiescent(&self) -> bool {
        self.entries
            .iter()
            .all(|entry| !entry.slot.enabled || entry.module.is_quiescent())
    }
    pub fn reset_audio(&mut self) {
        self.format_generation = None;
        for entry in &mut self.entries {
            entry.module.reset_audio();
        }
    }
    /// Configs equal in `previous` are kept, since applying one restarts its processing.
    pub fn apply_visual_settings(
        &mut self,
        settings: &VisualSettings,
        previous: Option<&VisualSettings>,
    ) {
        for entry in &mut self.entries {
            let kind = entry.slot.kind;
            let config = settings.config(kind);
            if previous.is_none_or(|previous| previous.config(kind) != config) {
                entry.apply_config(config);
            }
            entry.set_enabled(settings.is_enabled(kind));
            entry.slot.width_basis = (settings.width_basis.get(&kind).copied())
                .unwrap_or_else(|| default_width_basis(kind));
        }
        self.reorder(&settings.order);
    }
    fn reorder(&mut self, order: &[VisualKind]) {
        let rank = |kind| {
            let position = |list: &[VisualKind]| list.iter().position(|&listed| listed == kind);
            position(order).unwrap_or_else(|| {
                order.len() + position(DEFAULT_ORDER).expect("visual kind missing from registry")
            })
        };
        self.entries.sort_by_key(|entry| rank(entry.slot.kind));
    }
    /// Palettes equal in `previous` are kept, so a drag previewed elsewhere stays shown.
    pub fn apply_palettes(&mut self, theme: &ThemeFile, previous: Option<&ThemeFile>) {
        for entry in &mut self.entries {
            let palette = theme.palettes.get(&entry.slot.kind);
            if previous.is_none_or(|previous| previous.palettes.get(&entry.slot.kind) != palette) {
                entry.module.apply_palette(palette);
            }
        }
    }
    pub fn ingest_samples(&mut self, samples: &[f32], format: AudioFormat) {
        if self
            .format_generation
            .is_some_and(|generation| generation != format.generation)
        {
            self.reset_audio();
        }
        self.format_generation = Some(format.generation);
        let signal = samples.iter().any(|&sample| sample != 0.0);
        let block = AudioBlock::new(samples, format);
        for entry in &mut self.entries {
            if entry.slot.enabled {
                entry.module.ingest(&block, signal);
            }
        }
    }
}

pub(crate) type VisualManagerHandle = Shared<VisualManager>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::settings as settings_cfg;
    use crate::visuals::options::WaveformHistoryMode;

    #[test]
    fn typed_updates_follow_the_variant_after_reordering_and_keep_enablement() {
        use settings_cfg::*;
        let mut manager = VisualManager::default();
        let mut order = manager.order();
        order.reverse();
        manager.reorder(&order);
        manager.set_enabled(VisualKind::Spectrum, true);
        let configs = [
            VisualConfig::Loudness(LoudnessSettings {
                left_mode: crate::visuals::options::MeterMode::RmsFast,
                ..Default::default()
            }),
            VisualConfig::Oscilloscope(OscilloscopeSettings {
                stacked: true,
                persistence_time_constant: crate::util::audio::Seconds::must(0.25),
                ..Default::default()
            }),
            VisualConfig::Waveform(WaveformSettings {
                scroll_speed: waveform::processor::ScrollSpeed::must(72.0),
                history_mode: WaveformHistoryMode::RmsFast,
                ..Default::default()
            }),
            VisualConfig::Spectrogram(SpectrogramSettings {
                fft_size: crate::util::audio::FftSize::must(2048),
                source: crate::util::audio::Channel::Right,
                rotation: -1,
                ..Default::default()
            }),
            VisualConfig::Spectrum(SpectrumSettings {
                fft_size: crate::util::audio::FftSize::must(4096),
                show_grid: false,
                ..Default::default()
            }),
            VisualConfig::Stereometer(StereometerSettings {
                target_sample_count: 800,
                flip: false,
                ..Default::default()
            }),
        ];
        for config in configs {
            manager.apply_config(config);
        }
        assert_eq!(manager.order(), order);
        assert!(
            manager
                .entries
                .iter()
                .all(|entry| entry.slot.enabled == (entry.slot.kind == VisualKind::Spectrum))
        );
    }

    #[test]
    fn visuals_go_quiet_after_signal_and_wake_on_buffered_signal() {
        let format = AudioFormat::fallback(2, crate::dsp::SampleRate::DEFAULT);
        let tone = crate::util::audio::sine_wave(440.0, 48_000.0, 96_000, 0.5);
        for &kind in VisualKind::ALL {
            let mut manager = VisualManager::default();
            manager.apply_visual_settings(&Default::default(), None);
            for &other in VisualKind::ALL {
                manager.set_enabled(other, other == kind);
            }
            for entry in manager.snapshot() {
                if let VisualContent::Spectrogram(state) = entry.content {
                    state.borrow_mut().view_width = 400;
                }
            }
            for chunk in tone
                .chunks(512)
                .chain(std::iter::repeat_n(&[0.0; 512][..], 2_000))
            {
                manager.ingest_samples(chunk, format);
            }
            assert!(manager.is_quiescent(), "{kind:?}");
            manager.ingest_samples(&[0.25; 2], format);
            assert!(!manager.is_quiescent(), "{kind:?}");
        }
    }
}
