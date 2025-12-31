use super::SettingsMessage;
use super::palette::PaletteEvent;
use super::widgets::{
    SliderRange, labeled_pick_list, labeled_slider, section_title, set_if_changed,
    update_f32_range, update_usize_from_f32,
};
use crate::dsp::spectrogram::{
    FrequencyScale, PLANCK_BESSEL_DEFAULT_BETA, PLANCK_BESSEL_DEFAULT_EPSILON, WindowKind,
};
use crate::ui::settings::{PianoRollSide, SettingsHandle, SpectrogramSettings};
use crate::ui::theme;
use crate::ui::visualization::visual_manager::{VisualKind, VisualManagerHandle};
use iced::widget::{column, row, toggler};
use iced::{Element, Length};

const FFT_OPTIONS: [usize; 5] = [1024, 2048, 4096, 8192, 16384];
const ZERO_PAD_OPTIONS: [usize; 6] = [1, 2, 4, 8, 16, 32];
const HOP_DIVISORS: [usize; 7] = [4, 6, 8, 16, 32, 64, 128];
const FREQ_SCALE_OPTIONS: [FrequencyScale; 3] = [
    FrequencyScale::Linear,
    FrequencyScale::Logarithmic,
    FrequencyScale::Mel,
];
const HISTORY_RANGE: SliderRange = SliderRange::new(120.0, 3840.0, 30.0);
const REASSIGN_FLOOR_RANGE: SliderRange = SliderRange::new(-120.0, -30.0, 1.0);
const DISPLAY_BINS_RANGE: SliderRange = SliderRange::new(64.0, 4096.0, 64.0);
const PB_EPSILON_RANGE: SliderRange = SliderRange::new(0.01, 0.5, 0.01);
const PB_BETA_RANGE: SliderRange = SliderRange::new(0.0, 20.0, 0.25);

fn get_closest_hop_divisor(fft_size: usize, hop_size: usize) -> usize {
    if fft_size == 0 || hop_size == 0 {
        return 8;
    }
    let ratio = fft_size as f32 / hop_size as f32;
    HOP_DIVISORS
        .iter()
        .copied()
        .min_by(|&a, &b| {
            (ratio - a as f32)
                .abs()
                .partial_cmp(&(ratio - b as f32).abs())
                .unwrap()
        })
        .unwrap_or(8)
}

fn extract_planck_bessel(window: WindowKind) -> (f32, f32) {
    match window {
        WindowKind::PlanckBessel { epsilon, beta } => (epsilon, beta),
        _ => (PLANCK_BESSEL_DEFAULT_EPSILON, PLANCK_BESSEL_DEFAULT_BETA),
    }
}

settings_pane!(
    SpectrogramSettingsPane, SpectrogramSettings, VisualKind::Spectrogram,
    theme::DEFAULT_SPECTROGRAM_PALETTE,
    extra_from_settings(settings) {
        planck_bessel: (f32, f32) = extract_planck_bessel(settings.window),
    }
);

#[derive(Debug, Clone, Copy)]
pub enum Message {
    FftSize(usize),
    HopDivisor(usize),
    HistoryLength(f32),
    Window(WindowPreset),
    PlanckBesselEpsilon(f32),
    PlanckBesselBeta(f32),
    FrequencyScale(FrequencyScale),
    UseReassignment(bool),
    ReassignmentFloor(f32),
    ZeroPadding(usize),
    DisplayBinCount(f32),
    ShowPianoRoll(bool),
    PianoRollSide(PianoRollSide),
    Palette(PaletteEvent),
}

impl SpectrogramSettingsPane {
    fn view(&self) -> Element<'_, SettingsMessage> {
        let s = &self.settings;
        let window = WindowPreset::from_kind(s.window);
        let hop_divisor = get_closest_hop_divisor(s.fft_size, s.hop_size);

        let left_col = column![
            labeled_pick_list("FFT size", &FFT_OPTIONS, Some(s.fft_size), |v| {
                SettingsMessage::Spectrogram(Message::FftSize(v))
            }),
            labeled_pick_list("Hop divisor", &HOP_DIVISORS, Some(hop_divisor), |v| {
                SettingsMessage::Spectrogram(Message::HopDivisor(v))
            }),
        ]
        .spacing(8);

        let right_col = column![
            labeled_pick_list("Window", &WindowPreset::ALL, Some(window), |v| {
                SettingsMessage::Spectrogram(Message::Window(v))
            }),
            labeled_pick_list(
                "Freq scale",
                &FREQ_SCALE_OPTIONS,
                Some(s.frequency_scale),
                |v| SettingsMessage::Spectrogram(Message::FrequencyScale(v))
            ),
            labeled_pick_list(
                "Zero pad",
                &ZERO_PAD_OPTIONS,
                Some(s.zero_padding_factor),
                |v| SettingsMessage::Spectrogram(Message::ZeroPadding(v))
            ),
        ]
        .spacing(8);

        let mut core =
            column![row![left_col, right_col].spacing(10).width(Length::Fill)].spacing(8);
        if let WindowKind::PlanckBessel { epsilon, beta } = s.window {
            core = core.push(labeled_slider(
                "PB epsilon",
                epsilon,
                format!("{epsilon:.3}"),
                PB_EPSILON_RANGE,
                |v| SettingsMessage::Spectrogram(Message::PlanckBesselEpsilon(v)),
            ));
            core = core.push(labeled_slider(
                "PB beta",
                beta,
                format!("{beta:.2}"),
                PB_BETA_RANGE,
                |v| SettingsMessage::Spectrogram(Message::PlanckBesselBeta(v)),
            ));
        }
        core = core.push(labeled_slider(
            "History length",
            s.history_length as f32,
            format!("{} cols", s.history_length),
            HISTORY_RANGE,
            |v| SettingsMessage::Spectrogram(Message::HistoryLength(v)),
        ));

        let mut adv = column![
            toggler(s.use_reassignment)
                .label("Time-frequency reassignment")
                .text_size(11)
                .spacing(4)
                .on_toggle(|v| SettingsMessage::Spectrogram(Message::UseReassignment(v)))
        ]
        .spacing(8);
        if s.use_reassignment {
            adv = adv.push(labeled_slider(
                "Reassign floor",
                s.reassignment_power_floor_db,
                format!("{:.0} dB", s.reassignment_power_floor_db),
                REASSIGN_FLOOR_RANGE,
                |v| SettingsMessage::Spectrogram(Message::ReassignmentFloor(v)),
            ));
            adv = adv.push(labeled_slider(
                "Display bins",
                s.display_bin_count as f32,
                format!("{} bins", s.display_bin_count),
                DISPLAY_BINS_RANGE,
                |v| SettingsMessage::Spectrogram(Message::DisplayBinCount(v)),
            ));
        }
        adv = adv.push(
            toggler(s.show_piano_roll)
                .label("Piano roll overlay")
                .text_size(11)
                .spacing(4)
                .on_toggle(|v| SettingsMessage::Spectrogram(Message::ShowPianoRoll(v))),
        );
        if s.show_piano_roll {
            adv = adv.push(labeled_pick_list(
                "Side",
                &[PianoRollSide::Left, PianoRollSide::Right],
                Some(s.piano_roll_side),
                |v| SettingsMessage::Spectrogram(Message::PianoRollSide(v)),
            ));
        }

        column![
            section_title("Core controls"),
            core,
            section_title("Advanced"),
            adv,
            super::palette_section(
                &self.palette,
                Message::Palette,
                SettingsMessage::Spectrogram
            )
        ]
        .spacing(16)
        .into()
    }

    fn handle(
        &mut self,
        message: &SettingsMessage,
        visual_manager: &VisualManagerHandle,
        settings_handle: &SettingsHandle,
    ) {
        let SettingsMessage::Spectrogram(msg) = message else {
            return;
        };
        let s = &mut self.settings;
        let mut changed = false;
        match *msg {
            Message::FftSize(size) => {
                let hop_div = get_closest_hop_divisor(s.fft_size, s.hop_size);
                if set_if_changed(&mut s.fft_size, size) {
                    s.hop_size = (size / hop_div).max(1);
                    changed = true;
                }
            }
            Message::HopDivisor(div) => {
                changed |= set_if_changed(&mut s.hop_size, (s.fft_size / div).max(1));
            }
            Message::HistoryLength(v) => {
                changed |= update_usize_from_f32(&mut s.history_length, v, HISTORY_RANGE);
            }
            Message::Window(preset) => {
                let cur = WindowPreset::from_kind(s.window);
                if cur != preset {
                    if let WindowKind::PlanckBessel { epsilon, beta } = s.window {
                        self.planck_bessel = (epsilon, beta);
                    }
                    s.window = match preset {
                        WindowPreset::PlanckBessel => WindowKind::PlanckBessel {
                            epsilon: self.planck_bessel.0,
                            beta: self.planck_bessel.1,
                        },
                        _ => preset.to_window_kind(),
                    };
                    changed = true;
                }
            }
            Message::PlanckBesselEpsilon(v) => {
                if let WindowKind::PlanckBessel { epsilon, beta } = s.window {
                    let mut e = epsilon;
                    if update_f32_range(&mut e, v, PB_EPSILON_RANGE) {
                        self.planck_bessel.0 = e;
                        s.window = WindowKind::PlanckBessel { epsilon: e, beta };
                        changed = true;
                    }
                }
            }
            Message::PlanckBesselBeta(v) => {
                if let WindowKind::PlanckBessel { epsilon, beta } = s.window {
                    let mut b = beta;
                    if update_f32_range(&mut b, v, PB_BETA_RANGE) {
                        self.planck_bessel.1 = b;
                        s.window = WindowKind::PlanckBessel { epsilon, beta: b };
                        changed = true;
                    }
                }
            }
            Message::FrequencyScale(sc) => {
                changed |= set_if_changed(&mut s.frequency_scale, sc);
            }
            Message::UseReassignment(v) => {
                changed |= set_if_changed(&mut s.use_reassignment, v);
            }
            Message::ReassignmentFloor(v) => {
                changed |=
                    update_f32_range(&mut s.reassignment_power_floor_db, v, REASSIGN_FLOOR_RANGE);
            }
            Message::ZeroPadding(v) => {
                changed |= set_if_changed(&mut s.zero_padding_factor, v);
            }
            Message::DisplayBinCount(v) => {
                changed |= s.use_reassignment
                    && update_usize_from_f32(&mut s.display_bin_count, v, DISPLAY_BINS_RANGE);
            }
            Message::ShowPianoRoll(v) => {
                changed |= set_if_changed(&mut s.show_piano_roll, v);
            }
            Message::PianoRollSide(side) => {
                changed |= set_if_changed(&mut s.piano_roll_side, side);
            }
            Message::Palette(e) => {
                changed |= self.palette.update(e);
            }
        }
        if changed {
            persist_palette!(
                visual_manager,
                settings_handle,
                VisualKind::Spectrogram,
                self,
                theme::DEFAULT_SPECTROGRAM_PALETTE
            );
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowPreset {
    Rectangular,
    Hann,
    Hamming,
    Blackman,
    BlackmanHarris,
    PlanckBessel,
}

impl WindowPreset {
    const ALL: [Self; 6] = [
        Self::Rectangular,
        Self::Hann,
        Self::Hamming,
        Self::Blackman,
        Self::BlackmanHarris,
        Self::PlanckBessel,
    ];
    fn from_kind(kind: WindowKind) -> Self {
        match kind {
            WindowKind::Rectangular => Self::Rectangular,
            WindowKind::Hann => Self::Hann,
            WindowKind::Hamming => Self::Hamming,
            WindowKind::Blackman => Self::Blackman,
            WindowKind::BlackmanHarris => Self::BlackmanHarris,
            WindowKind::PlanckBessel { .. } => Self::PlanckBessel,
        }
    }
    fn to_window_kind(self) -> WindowKind {
        match self {
            Self::Rectangular => WindowKind::Rectangular,
            Self::Hann => WindowKind::Hann,
            Self::Hamming => WindowKind::Hamming,
            Self::Blackman => WindowKind::Blackman,
            Self::BlackmanHarris => WindowKind::BlackmanHarris,
            Self::PlanckBessel => WindowKind::PlanckBessel {
                epsilon: PLANCK_BESSEL_DEFAULT_EPSILON,
                beta: PLANCK_BESSEL_DEFAULT_BETA,
            },
        }
    }
}

impl std::fmt::Display for WindowPreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Rectangular => "Rectangular",
            Self::Hann => "Hann",
            Self::Hamming => "Hamming",
            Self::Blackman => "Blackman",
            Self::BlackmanHarris => "Blackman-Harris",
            Self::PlanckBessel => "Planck-Bessel",
        })
    }
}
