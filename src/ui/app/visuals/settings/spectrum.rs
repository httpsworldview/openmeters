use super::palette::PaletteEvent;
use super::widgets::{
    SliderRange, labeled_pick_list, labeled_slider, labeled_toggler, section_title, set_f32,
    set_if_changed, update_usize_from_f32,
};
use crate::dsp::spectrogram::FrequencyScale;
use crate::dsp::spectrum::{
    AveragingMode, MAX_SPECTRUM_EXP_FACTOR, MAX_SPECTRUM_PEAK_DECAY, MIN_SPECTRUM_EXP_FACTOR,
    MIN_SPECTRUM_PEAK_DECAY,
};
use crate::ui::settings::{SpectrumDisplayMode, SpectrumSettings, SpectrumWeightingMode};
use crate::ui::theme;
use crate::ui::visualization::visual_manager::VisualKind;
use iced::widget::{column, row};
use iced::{Element, Length};

const FFT_OPTIONS: [usize; 5] = [1024, 2048, 4096, 8192, 16384];
const FREQ_SCALE: [FrequencyScale; 3] = [
    FrequencyScale::Linear,
    FrequencyScale::Logarithmic,
    FrequencyScale::Mel,
];
const DISPLAY_MODE: [SpectrumDisplayMode; 2] =
    [SpectrumDisplayMode::Line, SpectrumDisplayMode::Bar];
const WEIGHTING: [SpectrumWeightingMode; 2] =
    [SpectrumWeightingMode::AWeighted, SpectrumWeightingMode::Raw];
const AVG_MODE: [AvgMode; 3] = [AvgMode::None, AvgMode::Exponential, AvgMode::PeakHold];

const EXP_R: SliderRange = SliderRange::new(MIN_SPECTRUM_EXP_FACTOR, MAX_SPECTRUM_EXP_FACTOR, 0.01);
const DECAY_R: SliderRange =
    SliderRange::new(MIN_SPECTRUM_PEAK_DECAY, MAX_SPECTRUM_PEAK_DECAY, 0.5);
const SRAD_R: SliderRange = SliderRange::new(0.0, 20.0, 1.0);
const SPAS_R: SliderRange = SliderRange::new(0.0, 5.0, 1.0);
const BARS_R: SliderRange = SliderRange::new(8.0, 128.0, 1.0);
const GAP_R: SliderRange = SliderRange::new(0.0, 0.8, 0.05);
const HIGH_R: SliderRange = SliderRange::new(0.0, 0.9, 0.01);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AvgMode {
    None,
    #[default]
    Exponential,
    PeakHold,
}

impl std::fmt::Display for AvgMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::None => "None",
            Self::Exponential => "Exponential",
            Self::PeakHold => "Peak hold",
        })
    }
}

settings_pane!(
    SpectrumSettingsPane, SpectrumSettings, VisualKind::Spectrum,
    theme::spectrum, Spectrum,
    extra_from_settings(settings) {
        avg_mode: AvgMode = split_averaging(settings.averaging).0,
        avg_factor: f32 = split_averaging(settings.averaging).1,
        peak_decay: f32 = split_averaging(settings.averaging).2,
    }
);

#[derive(Debug, Clone, Copy)]
pub enum Message {
    FftSize(usize),
    FrequencyScale(FrequencyScale),
    ReverseFrequency(bool),
    DisplayMode(SpectrumDisplayMode),
    WeightingMode(SpectrumWeightingMode),
    ShowSecondary(bool),
    Averaging(AvgMode),
    AvgFactor(f32),
    PeakDecay(f32),
    SmoothRadius(f32),
    SmoothPasses(f32),
    ShowGrid(bool),
    ShowPeakLabel(bool),
    BarCount(f32),
    BarGap(f32),
    Highlight(f32),
    Palette(PaletteEvent),
}

impl SpectrumSettingsPane {
    fn view(&self) -> Element<'_, Message> {
        use Message::*;
        let s = &self.settings;
        let dir = if s.reverse_frequency {
            "High <- Low"
        } else {
            "Low -> High"
        };

        let left = column![
            labeled_pick_list("Display", &DISPLAY_MODE, Some(s.display_mode), DisplayMode),
            labeled_pick_list(
                "Weighting",
                &WEIGHTING,
                Some(s.weighting_mode),
                WeightingMode
            ),
            labeled_pick_list("FFT size", &FFT_OPTIONS, Some(s.fft_size), FftSize),
        ]
        .spacing(8)
        .width(Length::Fill);

        let right = column![
            labeled_pick_list(
                "Freq scale",
                &FREQ_SCALE,
                Some(s.frequency_scale),
                FrequencyScale
            ),
            labeled_pick_list("Averaging", &AVG_MODE, Some(self.avg_mode), Averaging),
        ]
        .spacing(8)
        .width(Length::Fill);

        let mut avg_ctrl = column![].spacing(8);
        match self.avg_mode {
            AvgMode::Exponential => {
                avg_ctrl = avg_ctrl.push(labeled_slider(
                    "Exp factor",
                    self.avg_factor,
                    format!("{:.2}", self.avg_factor),
                    EXP_R,
                    AvgFactor,
                ))
            }
            AvgMode::PeakHold => {
                avg_ctrl = avg_ctrl.push(labeled_slider(
                    "Peak decay",
                    self.peak_decay,
                    format!("{:.1} dB/s", self.peak_decay),
                    DECAY_R,
                    PeakDecay,
                ))
            }
            AvgMode::None => {}
        }

        let mut visual = column![
            labeled_slider(
                "Smooth radius",
                s.smoothing_radius as f32,
                format!("{} bins", s.smoothing_radius),
                SRAD_R,
                SmoothRadius
            ),
            labeled_slider(
                "Smooth passes",
                s.smoothing_passes as f32,
                s.smoothing_passes.to_string(),
                SPAS_R,
                SmoothPasses
            ),
        ]
        .spacing(8);
        if s.display_mode == SpectrumDisplayMode::Bar {
            visual = visual
                .push(labeled_slider(
                    "Bar count",
                    s.bar_count as f32,
                    s.bar_count.to_string(),
                    BARS_R,
                    BarCount,
                ))
                .push(labeled_slider(
                    "Bar gap",
                    s.bar_gap,
                    format!("{:.0}%", s.bar_gap * 100.0),
                    GAP_R,
                    BarGap,
                ));
        }
        visual = visual.push(labeled_slider(
            "Color floor",
            s.highlight_threshold,
            format!("{:.0}%", s.highlight_threshold * 100.0),
            HIGH_R,
            Highlight,
        ));

        let toggles = row![
            column![
                labeled_toggler(dir, s.reverse_frequency, ReverseFrequency),
                labeled_toggler("Freq grid", s.show_grid, ShowGrid),
            ]
            .spacing(8)
            .width(Length::Fill),
            column![
                labeled_toggler("Peak label", s.show_peak_label, ShowPeakLabel),
                labeled_toggler("Secondary", s.show_secondary_line, ShowSecondary),
            ]
            .spacing(8)
            .width(Length::Fill),
        ]
        .spacing(16);

        column![
            section_title("Core"),
            row![left, right].spacing(16),
            avg_ctrl,
            section_title("Display"),
            toggles,
            visual,
            super::palette_section(&self.palette, Palette)
        ]
        .spacing(12)
        .into()
    }

    fn handle(&mut self, msg: &Message) -> bool {
        use Message::*;
        let s = &mut self.settings;

        match *msg {
            FftSize(v) => set_if_changed(&mut s.fft_size, v)
                .then(|| s.hop_size = (v / 4).max(1))
                .is_some(),
            FrequencyScale(v) => set_if_changed(&mut s.frequency_scale, v),
            ReverseFrequency(v) => set_if_changed(&mut s.reverse_frequency, v),
            DisplayMode(v) => set_if_changed(&mut s.display_mode, v),
            WeightingMode(v) => set_if_changed(&mut s.weighting_mode, v),
            ShowSecondary(v) => set_if_changed(&mut s.show_secondary_line, v),
            ShowGrid(v) => set_if_changed(&mut s.show_grid, v),
            ShowPeakLabel(v) => set_if_changed(&mut s.show_peak_label, v),
            Averaging(m) => set_if_changed(&mut self.avg_mode, m)
                .then(|| self.sync_avg())
                .is_some(),
            AvgFactor(v) => set_f32(&mut self.avg_factor, EXP_R.snap(v))
                .then(|| self.sync_avg())
                .is_some(),
            PeakDecay(v) => set_f32(&mut self.peak_decay, DECAY_R.snap(v))
                .then(|| self.sync_avg())
                .is_some(),
            SmoothRadius(v) => update_usize_from_f32(&mut s.smoothing_radius, v, SRAD_R),
            SmoothPasses(v) => update_usize_from_f32(&mut s.smoothing_passes, v, SPAS_R),
            BarCount(v) => update_usize_from_f32(&mut s.bar_count, v, BARS_R),
            BarGap(v) => set_f32(&mut s.bar_gap, GAP_R.snap(v)),
            Highlight(v) => set_f32(&mut s.highlight_threshold, HIGH_R.snap(v)),
            Palette(e) => self.palette.update(e),
        }
    }

    fn sync_avg(&mut self) {
        self.settings.averaging = match self.avg_mode {
            AvgMode::None => AveragingMode::None,
            AvgMode::Exponential => AveragingMode::Exponential {
                factor: self.avg_factor,
            },
            AvgMode::PeakHold => AveragingMode::PeakHold {
                decay_per_second: self.peak_decay,
            },
        }
        .normalized();
    }
}

fn split_averaging(avg: AveragingMode) -> (AvgMode, f32, f32) {
    let (df, dd) = (
        AveragingMode::default_exponential_factor(),
        AveragingMode::default_peak_decay(),
    );
    match avg.normalized() {
        AveragingMode::None => (AvgMode::None, df, dd),
        AveragingMode::Exponential { factor } => (AvgMode::Exponential, factor, dd),
        AveragingMode::PeakHold { decay_per_second } => (AvgMode::PeakHold, df, decay_per_second),
    }
}
