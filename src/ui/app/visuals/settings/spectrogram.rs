use super::palette::PaletteEvent;
use super::widgets::{
    SliderRange, labeled_pick_list, labeled_slider, labeled_toggler, section_title, set_if_changed,
    update_f32_range, update_usize_from_f32,
};
use crate::dsp::spectrogram::{
    FrequencyScale, PLANCK_BESSEL_DEFAULT_BETA, PLANCK_BESSEL_DEFAULT_EPSILON, WindowKind,
};
use crate::ui::settings::{PianoRollOverlay, SpectrogramSettings};
use crate::ui::theme;
use crate::ui::visualization::visual_manager::VisualKind;
use iced::widget::{column, row};
use iced::{Element, Length};

const FFT_OPTIONS: [usize; 5] = [1024, 2048, 4096, 8192, 16384];
const ZERO_PAD_OPTIONS: [usize; 6] = [1, 2, 4, 8, 16, 32];
const HOP_DIVISORS: [usize; 7] = [4, 6, 8, 16, 32, 64, 128];
const FREQ_SCALE_OPTIONS: [FrequencyScale; 3] = [
    FrequencyScale::Linear,
    FrequencyScale::Logarithmic,
    FrequencyScale::Mel,
];
const PIANO_ROLL_OVERLAY_OPTIONS: [PianoRollOverlay; 3] = [
    PianoRollOverlay::Off,
    PianoRollOverlay::Right,
    PianoRollOverlay::Left,
];
const HISTORY_RANGE: SliderRange = SliderRange::new(120.0, 3840.0, 30.0);
const FLOOR_DB_RANGE: SliderRange = SliderRange::new(-140.0, -1.0, 1.0);
const DISPLAY_BINS_RANGE: SliderRange = SliderRange::new(64.0, 4096.0, 64.0);
const MAX_CORR_HZ_RANGE: SliderRange = SliderRange::new(0.0, 200.0, 1.0);
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
    theme::spectrogram, Spectrogram,
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
    MaxCorrectionHz(f32),
    FloorDb(f32),
    ZeroPadding(usize),
    DisplayBinCount(f32),
    PianoRoll(PianoRollOverlay),
    Palette(PaletteEvent),
}

impl SpectrogramSettingsPane {
    fn view(&self) -> Element<'_, Message> {
        let s = &self.settings;
        let window = WindowPreset::from_kind(s.window);
        let hop_divisor = get_closest_hop_divisor(s.fft_size, s.hop_size);

        let left_col = column![
            labeled_pick_list("FFT size", &FFT_OPTIONS, Some(s.fft_size), Message::FftSize),
            labeled_pick_list(
                "Hop divisor",
                &HOP_DIVISORS,
                Some(hop_divisor),
                Message::HopDivisor
            ),
            labeled_pick_list(
                "Piano roll overlay",
                &PIANO_ROLL_OVERLAY_OPTIONS,
                Some(s.piano_roll_overlay),
                Message::PianoRoll
            ),
        ]
        .spacing(8)
        .width(Length::Fill);

        let right_col = column![
            labeled_pick_list("Window", &WindowPreset::ALL, Some(window), Message::Window),
            labeled_pick_list(
                "Freq scale",
                &FREQ_SCALE_OPTIONS,
                Some(s.frequency_scale),
                Message::FrequencyScale
            ),
            labeled_pick_list(
                "Zero pad",
                &ZERO_PAD_OPTIONS,
                Some(s.zero_padding_factor),
                Message::ZeroPadding
            ),
        ]
        .spacing(8)
        .width(Length::Fill);

        let mut core =
            column![row![left_col, right_col].spacing(10).width(Length::Fill)].spacing(8);
        if let WindowKind::PlanckBessel { epsilon, beta } = s.window {
            core = core
                .push(labeled_slider(
                    "PB epsilon",
                    epsilon,
                    format!("{epsilon:.3}"),
                    PB_EPSILON_RANGE,
                    Message::PlanckBesselEpsilon,
                ))
                .push(labeled_slider(
                    "PB beta",
                    beta,
                    format!("{beta:.2}"),
                    PB_BETA_RANGE,
                    Message::PlanckBesselBeta,
                ));
        }
        core = core
            .push(labeled_slider(
                "History length",
                s.history_length as f32,
                format!("{} cols", s.history_length),
                HISTORY_RANGE,
                Message::HistoryLength,
            ))
            .push(labeled_slider(
                "Floor",
                s.floor_db,
                format!("{:.0} dB", s.floor_db),
                FLOOR_DB_RANGE,
                Message::FloorDb,
            ));

        let mut adv = column![labeled_toggler(
            "Time-frequency reassignment",
            s.use_reassignment,
            Message::UseReassignment
        )]
        .spacing(8);
        if s.use_reassignment {
            let corr_is_auto = !s.reassignment_max_correction_hz.is_finite()
                || s.reassignment_max_correction_hz <= 0.0;
            adv = adv
                .push(labeled_slider(
                    "Display bins",
                    s.display_bin_count as f32,
                    format!("{} bins", s.display_bin_count),
                    DISPLAY_BINS_RANGE,
                    Message::DisplayBinCount,
                ))
                .push(labeled_slider(
                    "Max correction",
                    s.reassignment_max_correction_hz,
                    if corr_is_auto {
                        "Auto".to_string()
                    } else {
                        format!("{:.0} Hz", s.reassignment_max_correction_hz)
                    },
                    MAX_CORR_HZ_RANGE,
                    Message::MaxCorrectionHz,
                ));
        }

        column![
            section_title("Core controls"),
            core,
            section_title("Advanced"),
            adv,
            super::palette_section(&self.palette, Message::Palette)
        ]
        .spacing(16)
        .into()
    }

    fn handle(&mut self, msg: &Message) -> bool {
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
                changed |= set_if_changed(&mut s.hop_size, (s.fft_size / div).max(1))
            }
            Message::HistoryLength(v) => {
                changed |= update_usize_from_f32(&mut s.history_length, v, HISTORY_RANGE)
            }
            Message::Window(preset) => {
                if WindowPreset::from_kind(s.window) != preset {
                    if let WindowKind::PlanckBessel { epsilon, beta } = s.window {
                        self.planck_bessel = (epsilon, beta);
                    }
                    s.window = if preset == WindowPreset::PlanckBessel {
                        WindowKind::PlanckBessel {
                            epsilon: self.planck_bessel.0,
                            beta: self.planck_bessel.1,
                        }
                    } else {
                        preset.to_window_kind()
                    };
                    changed = true;
                }
            }
            Message::PlanckBesselEpsilon(v) => {
                if let WindowKind::PlanckBessel { mut epsilon, beta } = s.window
                    && update_f32_range(&mut epsilon, v, PB_EPSILON_RANGE)
                {
                    self.planck_bessel.0 = epsilon;
                    s.window = WindowKind::PlanckBessel { epsilon, beta };
                    changed = true;
                }
            }
            Message::PlanckBesselBeta(v) => {
                if let WindowKind::PlanckBessel { epsilon, mut beta } = s.window
                    && update_f32_range(&mut beta, v, PB_BETA_RANGE)
                {
                    self.planck_bessel.1 = beta;
                    s.window = WindowKind::PlanckBessel { epsilon, beta };
                    changed = true;
                }
            }
            Message::FrequencyScale(sc) => changed |= set_if_changed(&mut s.frequency_scale, sc),
            Message::UseReassignment(v) => changed |= set_if_changed(&mut s.use_reassignment, v),
            Message::MaxCorrectionHz(v) => {
                changed |=
                    update_f32_range(&mut s.reassignment_max_correction_hz, v, MAX_CORR_HZ_RANGE)
            }
            Message::FloorDb(v) => changed |= update_f32_range(&mut s.floor_db, v, FLOOR_DB_RANGE),
            Message::ZeroPadding(v) => changed |= set_if_changed(&mut s.zero_padding_factor, v),
            Message::DisplayBinCount(v) => {
                changed |= s.use_reassignment
                    && update_usize_from_f32(&mut s.display_bin_count, v, DISPLAY_BINS_RANGE)
            }
            Message::PianoRoll(opt) => changed |= set_if_changed(&mut s.piano_roll_overlay, opt),
            Message::Palette(e) => changed |= self.palette.update(e),
        }
        changed
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
