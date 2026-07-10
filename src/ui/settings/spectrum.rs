// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{
    FFT_OPTIONS, HOP_DIVISORS, get_closest_hop_divisor, set, set_f32, set_usize,
    update_fft_size, update_hop_divisor,
};
use crate::persistence::settings::SpectrumSettings;
use crate::ui::widgets::{SliderRange, pick, split, toggle};
use crate::util::audio::{Channel, FrequencyScale};
use crate::visuals::options::{SpectrumDisplayMode, SpectrumWeightingMode as WeightingMode};
use crate::visuals::spectrum::processor::{
    AveragingMode, MAX_SPECTRUM_DB_FLOOR, MAX_SPECTRUM_EXP_FACTOR, MAX_SPECTRUM_PEAK_DECAY,
    MIN_SPECTRUM_DB_FLOOR, MIN_SPECTRUM_EXP_FACTOR, MIN_SPECTRUM_PEAK_DECAY,
};

const EXP_R: SliderRange = SliderRange::new(MIN_SPECTRUM_EXP_FACTOR, MAX_SPECTRUM_EXP_FACTOR, 0.01);
const DECAY_R: SliderRange =
    SliderRange::new(MIN_SPECTRUM_PEAK_DECAY, MAX_SPECTRUM_PEAK_DECAY, 0.5);
const BARS_R: SliderRange = SliderRange::new(8.0, 128.0, 1.0);
const GAP_R: SliderRange = SliderRange::new(0.0, 0.8, 0.05);
const HIGH_R: SliderRange = SliderRange::new(0.0, 0.9, 0.01);
const FLOOR_R: SliderRange = SliderRange::new(MIN_SPECTRUM_DB_FLOOR, MAX_SPECTRUM_DB_FLOOR, 1.0);

crate::macros::choice_enum!(no_default all pub(in crate::ui) enum AvgMode {
    None => "None",
    Exponential => "Exponential",
    PeakHold => "Peak hold",
});

crate::macros::choice_enum!(no_default all pub(in crate::ui) enum FrequencyDirection {
    LowToHigh => "Low -> High",
    HighToLow => "High -> Low",
});

struct AveragingControls {
    mode: AvgMode,
    factor: f32,
    peak_decay: f32,
}

settings_pane!(
    SpectrumSettings,
    extra_from_settings(settings) {
        averaging: AveragingControls = split_averaging(settings.averaging),
    }
);

settings_messages!(pane, settings, value {
    FftSize(usize) => update_fft_size(&mut settings.fft_size, &mut settings.hop_size, value);
    HopDivisor(usize) => update_hop_divisor(settings.fft_size, &mut settings.hop_size, value);
    Source(Channel) => set(&mut settings.source, value);
    SecondarySource(Channel) => set(&mut settings.secondary_source, value);
    Scale(FrequencyScale) => set(&mut settings.frequency_scale, value);
    Direction(FrequencyDirection) => {
        set(&mut settings.reverse_frequency, value == FrequencyDirection::HighToLow)
    };
    Display(SpectrumDisplayMode) => set(&mut settings.display_mode, value);
    Weighting(WeightingMode) => set(&mut settings.weighting_mode, value);
    SecondaryWeighting(WeightingMode) => set(&mut settings.secondary_weighting_mode, value);
    Averaging(AvgMode) => pane.update_avg(|average| set(&mut average.mode, value));
    AvgFactor(f32) => pane.update_avg(|average| set_f32(&mut average.factor, value, EXP_R));
    PeakDecay(f32) => pane.update_avg(|average| {
        set_f32(&mut average.peak_decay, value, DECAY_R)
    });
    ShowGrid(bool) => set(&mut settings.show_grid, value);
    ShowPeakLabel(bool) => set(&mut settings.show_peak_label, value);
    FloorDb(f32) => set_f32(&mut settings.floor_db, value, FLOOR_R);
    BarCount(f32) => set_usize(&mut settings.bar_count, value, BARS_R);
    BarGap(f32) => set_f32(&mut settings.bar_gap, value, GAP_R);
    Highlight(f32) => set_f32(&mut settings.highlight_threshold, value, HIGH_R);
});

settings_view! {
    pane as settings {
        use FrequencyDirection::{HighToLow, LowToHigh};
        let hop_divisor = get_closest_hop_divisor(settings.fft_size, settings.hop_size);
        let direction = if settings.reverse_frequency { HighToLow } else { LowToHigh };

        let sources = split(
            form!(
                pick("Primary source", Channel::ALL, settings.source, Source);
                pick("Primary weighting", WeightingMode::ALL, settings.weighting_mode, Weighting);
            ),
            form!(
                pick("Secondary source", Channel::ALL, settings.secondary_source, SecondarySource);
                pick(
                    "Secondary weighting", WeightingMode::ALL,
                    settings.secondary_weighting_mode, SecondaryWeighting
                );
            ),
        );
        let mut analysis = form!(
            split(
                form!(
                    pick("FFT size", &FFT_OPTIONS[..], settings.fft_size, FftSize);
                    pick("Hop divisor", &HOP_DIVISORS[..], hop_divisor, HopDivisor);
                ),
                form!(
                    pick("Frequency scale", FrequencyScale::ALL, settings.frequency_scale, Scale);
                    pick("Averaging", AvgMode::ALL, pane.averaging.mode, Averaging);
                ),
            );
        );
        match pane.averaging.mode {
            AvgMode::Exponential => {
                analysis = analysis.push(slider!(
                    "Exp factor", pane.averaging.factor, EXP_R, AvgFactor, "{:.2}"
                ));
            }
            AvgMode::PeakHold => {
                analysis = analysis.push(slider!(
                    "Peak decay", pane.averaging.peak_decay, DECAY_R, PeakDecay, "{:.1} dB/s"
                ));
            }
            AvgMode::None => {}
        }

        let mut display = form!(
            pick("Display", SpectrumDisplayMode::ALL, settings.display_mode, Display);
            split(
                form!(
                    pick("Direction", FrequencyDirection::ALL, direction, Direction);
                    toggle("Frequency grid", settings.show_grid, ShowGrid);
                ),
                form!(toggle("Peak label", settings.show_peak_label, ShowPeakLabel);),
            );
            slider!("Noise floor", settings.floor_db, FLOOR_R, FloorDb, "{:.0} dB");
        );
        if settings.display_mode == SpectrumDisplayMode::Bar {
            display = display
                .push(slider!(
                    "Bar count", settings.bar_count as f32, BARS_R, BarCount,
                    settings.bar_count.to_string()
                ))
                .push(slider!(
                    "Bar gap", settings.bar_gap, GAP_R, BarGap,
                    format!("{:.0}%", settings.bar_gap * 100.0)
                ));
        }
        display = display.push(slider!(
            "Color floor", settings.highlight_threshold, HIGH_R, Highlight,
            format!("{:.0}%", settings.highlight_threshold * 100.0)
        ));
    }
    "Sources" => sources;
    "Analysis" => analysis;
    "Display" => display;
}

impl Pane {
    fn update_avg(&mut self, update: impl FnOnce(&mut AveragingControls) -> bool) -> bool {
        if !update(&mut self.averaging) {
            return false;
        }
        self.settings.averaging = match self.averaging.mode {
            AvgMode::None => AveragingMode::None,
            AvgMode::Exponential => AveragingMode::Exponential {
                factor: self.averaging.factor,
            },
            AvgMode::PeakHold => AveragingMode::PeakHold {
                decay_per_second: self.averaging.peak_decay,
            },
        };
        true
    }
}

fn split_averaging(avg: AveragingMode) -> AveragingControls {
    let default_factor = AveragingMode::default_exponential_factor();
    let default_peak_decay = AveragingMode::default_peak_decay();
    let (mode, factor, peak_decay) = match avg {
        AveragingMode::None => (AvgMode::None, default_factor, default_peak_decay),
        AveragingMode::Exponential { factor } => {
            (AvgMode::Exponential, factor, default_peak_decay)
        }
        AveragingMode::PeakHold { decay_per_second } => {
            (AvgMode::PeakHold, default_factor, decay_per_second)
        }
    };
    AveragingControls {
        mode,
        factor,
        peak_decay,
    }
}
