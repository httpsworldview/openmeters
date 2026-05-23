// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::palette::PaletteEvent;
use super::widgets::{
    FFT_OPTIONS, HOP_DIVISORS, SliderRange, get_closest_hop_divisor, pick, section, set_if_changed,
    slide, toggle, update_f32_range, update_fft_size, update_hop_divisor, update_usize_from_f32,
};
use crate::persistence::settings::SpectrumSettings;
use crate::util::audio::FrequencyScale;
use crate::visuals::options::{SpectrumDisplayMode, SpectrumWeightingMode};
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

crate::macros::choice_enum!(all pub(crate) enum AvgMode {
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

settings_messages!(SpectrumSettingsPane as pane, value {
    FftSize(usize) => update_fft_size(&mut pane.settings.fft_size, &mut pane.settings.hop_size, value);
    HopDivisor(usize) => update_hop_divisor(pane.settings.fft_size, &mut pane.settings.hop_size, value);
    FreqScale(FrequencyScale) => set_if_changed(&mut pane.settings.frequency_scale, value);
    ReverseFrequency(bool) => set_if_changed(&mut pane.settings.reverse_frequency, value);
    DisplayMode(SpectrumDisplayMode) => set_if_changed(&mut pane.settings.display_mode, value);
    WeightingMode(SpectrumWeightingMode) => set_if_changed(&mut pane.settings.weighting_mode, value);
    ShowSecondary(bool) => set_if_changed(&mut pane.settings.show_secondary_line, value);
    Averaging(AvgMode) => set_if_changed(&mut pane.avg_mode, value).then(|| pane.sync_avg()).is_some();
    AvgFactor(f32) => update_f32_range(&mut pane.avg_factor, value, EXP_R).then(|| pane.sync_avg()).is_some();
    PeakDecay(f32) => update_f32_range(&mut pane.peak_decay, value, DECAY_R).then(|| pane.sync_avg()).is_some();
    SmoothRadius(f32) => update_usize_from_f32(&mut pane.settings.smoothing_radius, value, SRAD_R);
    SmoothPasses(f32) => update_usize_from_f32(&mut pane.settings.smoothing_passes, value, SPAS_R);
    ShowGrid(bool) => set_if_changed(&mut pane.settings.show_grid, value);
    ShowPeakLabel(bool) => set_if_changed(&mut pane.settings.show_peak_label, value);
    FloorDb(f32) => update_f32_range(&mut pane.settings.floor_db, value, FLOOR_R);
    BarCount(f32) => update_usize_from_f32(&mut pane.settings.bar_count, value, BARS_R);
    BarGap(f32) => update_f32_range(&mut pane.settings.bar_gap, value, GAP_R);
    Highlight(f32) => update_f32_range(&mut pane.settings.highlight_threshold, value, HIGH_R);
    Palette(PaletteEvent) => pane.palette.update(value);
});

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
