use super::palette::{PaletteEditor, PaletteEvent};
use super::widgets::{
    SliderRange, labeled_pick_list, labeled_slider, set_f32, set_if_changed, update_usize_from_f32,
};
use super::{ModuleSettingsPane, SettingsMessage};
use crate::dsp::spectrogram::FrequencyScale;
use crate::dsp::spectrum::AveragingMode;
use crate::ui::settings::{SettingsHandle, SpectrumSettings};
use crate::ui::theme;
use crate::ui::visualization::visual_manager::{VisualId, VisualKind, VisualManagerHandle};
use iced::Element;
use iced::widget::{column, toggler};

const FFT_OPTIONS: [usize; 4] = [1024, 2048, 4096, 8192];
const SCALE_OPTIONS: [FrequencyScale; 3] = [
    FrequencyScale::Linear,
    FrequencyScale::Logarithmic,
    FrequencyScale::Mel,
];
const AVERAGING_OPTIONS: [SpectrumAveragingMode; 3] = [
    SpectrumAveragingMode::None,
    SpectrumAveragingMode::Exponential,
    SpectrumAveragingMode::PeakHold,
];
const EXPONENTIAL_RANGE: SliderRange = SliderRange::new(0.0, 0.95, 0.01);
const PEAK_DECAY_RANGE: SliderRange = SliderRange::new(0.0, 60.0, 0.5);
const SMOOTHING_RADIUS_RANGE: SliderRange = SliderRange::new(0.0, 20.0, 1.0);
const SMOOTHING_PASSES_RANGE: SliderRange = SliderRange::new(0.0, 5.0, 1.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SpectrumAveragingMode {
    None,
    #[default]
    Exponential,
    PeakHold,
}

impl std::fmt::Display for SpectrumAveragingMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::None => "None",
            Self::Exponential => "Exponential",
            Self::PeakHold => "Peak hold",
        })
    }
}

#[derive(Debug)]
pub struct SpectrumSettingsPane {
    visual_id: VisualId,
    settings: SpectrumSettings,
    // Cached UI state derived from settings.config.averaging
    averaging_mode: SpectrumAveragingMode,
    averaging_factor: f32,
    peak_hold_decay: f32,
    palette: PaletteEditor,
}

#[derive(Debug, Clone, Copy)]
pub enum Message {
    FftSize(usize),
    AveragingMode(SpectrumAveragingMode),
    AveragingFactor(f32),
    PeakHoldDecay(f32),
    FrequencyScale(FrequencyScale),
    ReverseFrequency(bool),
    ShowGrid(bool),
    ShowPeakLabel(bool),
    Palette(PaletteEvent),
    SmoothingRadius(f32),
    SmoothingPasses(f32),
}

pub fn create(visual_id: VisualId, visual_manager: &VisualManagerHandle) -> SpectrumSettingsPane {
    let (settings, palette): (SpectrumSettings, _) = super::load_settings_and_palette(
        visual_manager,
        VisualKind::SPECTRUM,
        &theme::DEFAULT_SPECTRUM_PALETTE,
        &[],
    );
    let (averaging_mode, averaging_factor, peak_hold_decay) =
        split_averaging(settings.config.averaging);

    SpectrumSettingsPane {
        visual_id,
        settings,
        averaging_mode,
        averaging_factor,
        peak_hold_decay,
        palette,
    }
}

impl ModuleSettingsPane for SpectrumSettingsPane {
    fn visual_id(&self) -> VisualId {
        self.visual_id
    }

    fn view(&self) -> Element<'_, SettingsMessage> {
        let cfg = &self.settings.config;
        let dir_label = if cfg.reverse_frequency {
            "High <- Low"
        } else {
            "Low -> High"
        };
        let toggle = |checked, label, f: fn(bool) -> Message| {
            toggler(checked)
                .label(label)
                .spacing(8)
                .text_size(11)
                .on_toggle(move |v| SettingsMessage::Spectrum(f(v)))
        };

        let mut content = column![
            labeled_pick_list("FFT size", &FFT_OPTIONS, Some(cfg.fft_size), |s| {
                SettingsMessage::Spectrum(Message::FftSize(s))
            }),
            labeled_pick_list(
                "Frequency scale",
                &SCALE_OPTIONS,
                Some(cfg.frequency_scale),
                |s| SettingsMessage::Spectrum(Message::FrequencyScale(s))
            ),
            toggle(cfg.reverse_frequency, dir_label, Message::ReverseFrequency),
            toggle(cfg.show_grid, "Show frequency grid", Message::ShowGrid),
            toggle(
                cfg.show_peak_label,
                "Show peak frequency label",
                Message::ShowPeakLabel
            ),
            labeled_pick_list(
                "Averaging mode",
                &AVERAGING_OPTIONS,
                Some(self.averaging_mode),
                |m| SettingsMessage::Spectrum(Message::AveragingMode(m))
            ),
        ]
        .spacing(16);

        if let SpectrumAveragingMode::Exponential = self.averaging_mode {
            content = content.push(labeled_slider(
                "Exponential factor",
                self.averaging_factor,
                format!("{:.2}", self.averaging_factor),
                EXPONENTIAL_RANGE,
                |v| SettingsMessage::Spectrum(Message::AveragingFactor(v)),
            ));
        } else if let SpectrumAveragingMode::PeakHold = self.averaging_mode {
            content = content.push(labeled_slider(
                "Peak decay (dB/s)",
                self.peak_hold_decay,
                format!("{:.1} dB/s", self.peak_hold_decay),
                PEAK_DECAY_RANGE,
                |v| SettingsMessage::Spectrum(Message::PeakHoldDecay(v)),
            ));
        }

        content
            .push(labeled_slider(
                "Smoothing radius",
                self.settings.smoothing_radius as f32,
                format!("{} bins", self.settings.smoothing_radius),
                SMOOTHING_RADIUS_RANGE,
                |v| SettingsMessage::Spectrum(Message::SmoothingRadius(v)),
            ))
            .push(labeled_slider(
                "Smoothing passes",
                self.settings.smoothing_passes as f32,
                self.settings.smoothing_passes.to_string(),
                SMOOTHING_PASSES_RANGE,
                |v| SettingsMessage::Spectrum(Message::SmoothingPasses(v)),
            ))
            .push(super::palette_section(
                &self.palette,
                Message::Palette,
                SettingsMessage::Spectrum,
            ))
            .into()
    }

    fn handle(
        &mut self,
        message: &SettingsMessage,
        visual_manager: &VisualManagerHandle,
        settings: &SettingsHandle,
    ) {
        let SettingsMessage::Spectrum(msg) = message else {
            return;
        };

        let cfg = &mut self.settings.config;
        let changed = match *msg {
            Message::FftSize(size) => {
                if set_if_changed(&mut cfg.fft_size, size) {
                    cfg.hop_size = (size / 4).max(1);
                    true
                } else {
                    false
                }
            }
            Message::AveragingMode(mode) => {
                if set_if_changed(&mut self.averaging_mode, mode) {
                    self.sync_averaging();
                    true
                } else {
                    false
                }
            }
            Message::AveragingFactor(v) => {
                if set_f32(&mut self.averaging_factor, EXPONENTIAL_RANGE.snap(v)) {
                    self.sync_averaging();
                    true
                } else {
                    false
                }
            }
            Message::PeakHoldDecay(v) => {
                if set_f32(&mut self.peak_hold_decay, PEAK_DECAY_RANGE.snap(v)) {
                    self.sync_averaging();
                    true
                } else {
                    false
                }
            }
            Message::FrequencyScale(s) => set_if_changed(&mut cfg.frequency_scale, s),
            Message::ReverseFrequency(v) => set_if_changed(&mut cfg.reverse_frequency, v),
            Message::ShowGrid(v) => set_if_changed(&mut cfg.show_grid, v),
            Message::ShowPeakLabel(v) => set_if_changed(&mut cfg.show_peak_label, v),
            Message::SmoothingRadius(v) => update_usize_from_f32(
                &mut self.settings.smoothing_radius,
                v,
                SMOOTHING_RADIUS_RANGE,
            ),
            Message::SmoothingPasses(v) => update_usize_from_f32(
                &mut self.settings.smoothing_passes,
                v,
                SMOOTHING_PASSES_RANGE,
            ),
            Message::Palette(e) => self.palette.update(e),
        };

        if changed {
            persist_palette!(
                visual_manager,
                settings,
                VisualKind::SPECTRUM,
                self,
                theme::DEFAULT_SPECTRUM_PALETTE
            );
        }
    }
}

impl SpectrumSettingsPane {
    fn sync_averaging(&mut self) {
        self.settings.config.averaging = match self.averaging_mode {
            SpectrumAveragingMode::None => AveragingMode::None,
            SpectrumAveragingMode::Exponential => AveragingMode::Exponential {
                factor: self.averaging_factor,
            },
            SpectrumAveragingMode::PeakHold => AveragingMode::PeakHold {
                decay_per_second: self.peak_hold_decay,
            },
        }
        .normalized();
    }
}

fn split_averaging(avg: AveragingMode) -> (SpectrumAveragingMode, f32, f32) {
    let default_factor = AveragingMode::default_exponential_factor();
    let default_decay = AveragingMode::default_peak_decay();
    match avg.normalized() {
        AveragingMode::None => (SpectrumAveragingMode::None, default_factor, default_decay),
        AveragingMode::Exponential { factor } => {
            (SpectrumAveragingMode::Exponential, factor, default_decay)
        }
        AveragingMode::PeakHold { decay_per_second } => (
            SpectrumAveragingMode::PeakHold,
            default_factor,
            decay_per_second,
        ),
    }
}
