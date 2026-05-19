// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::palette::PaletteEvent;
use super::widgets::{
    FFT_OPTIONS, HOP_DIVISORS, SliderRange, get_closest_hop_divisor, pick, section, set_if_changed,
    slide, toggle, update_f32_range, update_fft_size, update_hop_divisor, update_usize_from_f32,
};
use crate::persistence::settings::{SpectrumDisplayMode, SpectrumSettings, SpectrumWeightingMode};
use crate::util::audio::FrequencyScale;
use crate::visuals::registry::VisualKind;
use crate::visuals::spectrum::processor::{
    AveragingMode, MAX_SPECTRUM_DB_FLOOR, MAX_SPECTRUM_EXP_FACTOR, MAX_SPECTRUM_PEAK_DECAY,
    MIN_SPECTRUM_DB_FLOOR, MIN_SPECTRUM_EXP_FACTOR, MIN_SPECTRUM_PEAK_DECAY,
};
use iced::widget::{column, row};
use iced::{Element, Length};

const EXP_R: SliderRange = SliderRange::new(MIN_SPECTRUM_EXP_FACTOR, MAX_SPECTRUM_EXP_FACTOR, 0.01);
const DECAY_R: SliderRange =
    SliderRange::new(MIN_SPECTRUM_PEAK_DECAY, MAX_SPECTRUM_PEAK_DECAY, 0.5);
const SRAD_R: SliderRange = SliderRange::new(0.0, 20.0, 1.0);
const SPAS_R: SliderRange = SliderRange::new(0.0, 5.0, 1.0);
const BARS_R: SliderRange = SliderRange::new(8.0, 128.0, 1.0);
const GAP_R: SliderRange = SliderRange::new(0.0, 0.8, 0.05);
const HIGH_R: SliderRange = SliderRange::new(0.0, 0.9, 0.01);
const FLOOR_R: SliderRange = SliderRange::new(MIN_SPECTRUM_DB_FLOOR, MAX_SPECTRUM_DB_FLOOR, 1.0);

crate::settings_enum!(all pub(crate) enum AvgMode {
    None => "None",
    #[default] Exponential => "Exponential",
    PeakHold => "Peak hold",
});

settings_pane!(
    SpectrumSettingsPane, SpectrumSettings, VisualKind::Spectrum, Spectrum,
    extra_from_settings(settings) {
        avg_mode: AvgMode = split_averaging(settings.averaging).0,
        avg_factor: f32 = split_averaging(settings.averaging).1,
        peak_decay: f32 = split_averaging(settings.averaging).2,
    }
);

#[derive(Debug, Clone, Copy)]
pub enum Message {
    FftSize(usize),
    HopDivisor(usize),
    FreqScale(FrequencyScale),
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
    FloorDb(f32),
    BarCount(f32),
    BarGap(f32),
    Highlight(f32),
    Palette(PaletteEvent),
}

impl SpectrumSettingsPane {
    fn view(&self) -> Element<'_, Message> {
        use Message::*;
        let s = &self.settings;
        let hop_divisor = get_closest_hop_divisor(s.fft_size, s.hop_size);
        let dir = if s.reverse_frequency {
            "High <- Low"
        } else {
            "Low -> High"
        };
        let left = controls!(8.0;
            pick("Display", SpectrumDisplayMode::ALL, s.display_mode, DisplayMode);
            pick("Weighting", SpectrumWeightingMode::ALL, s.weighting_mode, WeightingMode);
            pick("FFT size", &FFT_OPTIONS, s.fft_size, FftSize);
        )
        .width(Length::Fill);
        let right = controls!(8.0;
            pick("Freq scale", FrequencyScale::ALL, s.frequency_scale, FreqScale);
            pick("Averaging", AvgMode::ALL, self.avg_mode, Averaging);
            pick("Hop divisor", &HOP_DIVISORS, hop_divisor, HopDivisor);
        )
        .width(Length::Fill);
        let avg_ctrl = match self.avg_mode {
            AvgMode::Exponential => controls!(8.0;
                slider!("Exp factor", self.avg_factor, EXP_R, AvgFactor, "{:.2}");
            ),
            AvgMode::PeakHold => controls!(8.0;
                slider!("Peak decay", self.peak_decay, DECAY_R, PeakDecay, "{:.1} dB/s");
            ),
            AvgMode::None => column![].spacing(8),
        };

        let mut visual = controls!(8.0;
            slider!("Smooth radius", s.smoothing_radius as f32, SRAD_R, SmoothRadius, format!("{} bins", s.smoothing_radius));
            slider!("Smooth passes", s.smoothing_passes as f32, SPAS_R, SmoothPasses, s.smoothing_passes.to_string());
            slider!("Noise floor", s.floor_db, FLOOR_R, FloorDb, "{:.0} dB");
        );
        if s.display_mode == SpectrumDisplayMode::Bar {
            visual = controls!(visual;
                slider!("Bar count", s.bar_count as f32, BARS_R, BarCount, s.bar_count.to_string());
                slider!("Bar gap", s.bar_gap, GAP_R, BarGap, format!("{:.0}%", s.bar_gap * 100.0));
            );
        }
        visual = controls!(visual;
            slider!(
                "Color floor", s.highlight_threshold, HIGH_R, Highlight,
                format!("{:.0}%", s.highlight_threshold * 100.0)
            );
        );

        let toggles = row![
            controls!(8.0;
                toggle(dir, s.reverse_frequency, ReverseFrequency);
                toggle("Freq grid", s.show_grid, ShowGrid);
            )
            .width(Length::Fill),
            controls!(8.0;
                toggle("Peak label", s.show_peak_label, ShowPeakLabel);
                toggle("Secondary", s.show_secondary_line, ShowSecondary);
            )
            .width(Length::Fill),
        ]
        .spacing(16);

        column![
            section("Core"),
            row![left, right].spacing(16),
            avg_ctrl,
            section("Display"),
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
            FftSize(v) => update_fft_size(&mut s.fft_size, &mut s.hop_size, v),
            HopDivisor(v) => update_hop_divisor(s.fft_size, &mut s.hop_size, v),
            FreqScale(v) => set_if_changed(&mut s.frequency_scale, v),
            ReverseFrequency(v) => set_if_changed(&mut s.reverse_frequency, v),
            DisplayMode(v) => set_if_changed(&mut s.display_mode, v),
            WeightingMode(v) => set_if_changed(&mut s.weighting_mode, v),
            ShowSecondary(v) => set_if_changed(&mut s.show_secondary_line, v),
            ShowGrid(v) => set_if_changed(&mut s.show_grid, v),
            ShowPeakLabel(v) => set_if_changed(&mut s.show_peak_label, v),
            FloorDb(v) => update_f32_range(&mut s.floor_db, v, FLOOR_R),
            Averaging(m) => set_if_changed(&mut self.avg_mode, m)
                .then(|| self.sync_avg())
                .is_some(),
            AvgFactor(v) => update_f32_range(&mut self.avg_factor, v, EXP_R)
                .then(|| self.sync_avg())
                .is_some(),
            PeakDecay(v) => update_f32_range(&mut self.peak_decay, v, DECAY_R)
                .then(|| self.sync_avg())
                .is_some(),
            SmoothRadius(v) => update_usize_from_f32(&mut s.smoothing_radius, v, SRAD_R),
            SmoothPasses(v) => update_usize_from_f32(&mut s.smoothing_passes, v, SPAS_R),
            BarCount(v) => update_usize_from_f32(&mut s.bar_count, v, BARS_R),
            BarGap(v) => update_f32_range(&mut s.bar_gap, v, GAP_R),
            Highlight(v) => update_f32_range(&mut s.highlight_threshold, v, HIGH_R),
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
