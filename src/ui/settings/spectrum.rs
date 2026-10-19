// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{
    FFT_OPTIONS, HOP_DIVISORS, QUARTER_TURNS, get_closest_hop_divisor, quarter_turn_label, set,
    set_f32, set_usize, settings_messages, settings_pane, settings_view, update_fft_size,
    update_hop_divisor,
};
use crate::persistence::settings::SpectrumSettings;
use crate::ui::widgets::{SliderRange, chip, pick, split, toggle};
use crate::util::audio::{Channel, FrequencyScale};
use crate::visuals::options::{SpectrumDisplayMode, SpectrumWeightingMode as WeightingMode};
use crate::visuals::spectrum::processor::{
    AveragingMode, DEFAULT_SPECTRUM_PEAK_DECAY, DEFAULT_SPECTRUM_TIME_CONSTANT,
    MAX_SPECTRUM_DB_FLOOR, MAX_SPECTRUM_PEAK_DECAY, MAX_SPECTRUM_TIME_CONSTANT,
    MIN_SPECTRUM_DB_FLOOR, MIN_SPECTRUM_PEAK_DECAY, MIN_SPECTRUM_TIME_CONSTANT,
};

const TIME_CONSTANT_R: SliderRange = SliderRange::new(
    MIN_SPECTRUM_TIME_CONSTANT,
    MAX_SPECTRUM_TIME_CONSTANT,
    0.005,
);
const DECAY_R: SliderRange =
    SliderRange::new(MIN_SPECTRUM_PEAK_DECAY, MAX_SPECTRUM_PEAK_DECAY, 0.5);
const BARS_R: SliderRange = SliderRange::new(8.0, 128.0, 1.0);
const GAP_R: SliderRange = SliderRange::new(0.0, 0.8, 0.05);
const FLOOR_R: SliderRange = SliderRange::new(MIN_SPECTRUM_DB_FLOOR, MAX_SPECTRUM_DB_FLOOR, 1.0);

crate::macros::choice_enum!(pub(in crate::ui) enum AvgMode {
    None => "None",
    Exponential => "Exponential",
    PeakHold => "Peak hold",
});

crate::macros::choice_enum!(pub(in crate::ui) enum FrequencyDirection {
    LowToHigh => "Low -> High",
    HighToLow => "High -> Low",
});

struct AveragingControls {
    mode: AvgMode,
    time_constant: f32,
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
    Rotation(f32) => set(&mut settings.rotation, value.round() as i8);
    Display(SpectrumDisplayMode) => set(&mut settings.display_mode, value);
    Weighting(WeightingMode) => set(&mut settings.weighting_mode, value);
    SecondaryWeighting(WeightingMode) => set(&mut settings.secondary_weighting_mode, value);
    Averaging(AvgMode) => pane.update_avg(|average| set(&mut average.mode, value));
    AvgTimeConstant(f32) => {
        pane.update_avg(|average| set_f32(&mut average.time_constant, value))
    };
    PeakDecay(f32) => pane.update_avg(|average| set_f32(&mut average.peak_decay, value));
    ShowGrid(bool) => set(&mut settings.show_grid, value);
    ShowPeakLabel(bool) => set(&mut settings.show_peak_label, value);
    FloorDb(f32) => set_f32(&mut settings.floor_db, value);
    BarCount(f32) => set_usize(&mut settings.bar_count, value);
    BarGap(f32) => set_f32(&mut settings.bar_gap, value);
});

settings_view! {
    pane as settings {
        use FrequencyDirection::{HighToLow, LowToHigh};
        let primary = settings.source != Channel::None;
        let secondary = settings.secondary_source != Channel::None;
        let bars = settings.display_mode == SpectrumDisplayMode::Bar;
        let hop_divisor = get_closest_hop_divisor(settings.fft_size, settings.hop_size);
        let direction = if settings.reverse_frequency { HighToLow } else { LowToHigh };

        let sources = split(
            form!(
                pick("Primary source", Channel::ALL, settings.source, Source);
                primary.then(|| {
                    pick("Primary weighting", WeightingMode::ALL, settings.weighting_mode, Weighting)
                });
            ),
            form!(
                pick("Secondary source", Channel::ALL, settings.secondary_source, SecondarySource);
                secondary.then(|| pick(
                    "Secondary weighting", WeightingMode::ALL,
                    settings.secondary_weighting_mode, SecondaryWeighting
                ));
            ),
        );
        let averaging = match pane.averaging.mode {
            AvgMode::Exponential => Some(slider!(
                "Time constant", pane.averaging.time_constant, TIME_CONSTANT_R, AvgTimeConstant,
                format!("{:.0} ms", pane.averaging.time_constant * 1000.0)
            )),
            AvgMode::PeakHold => Some(slider!(
                "Peak decay", pane.averaging.peak_decay, DECAY_R, PeakDecay, "{:.1} dB/s"
            )),
            AvgMode::None => None,
        };
        let view = form!(
            split(
                pick("Display", SpectrumDisplayMode::ALL, settings.display_mode, Display),
                pick("Frequency scale", FrequencyScale::ALL, settings.frequency_scale, Scale),
            );
            pick("Averaging", AvgMode::ALL, pane.averaging.mode, Averaging);
            averaging;
            slider!("Noise floor", settings.floor_db, FLOOR_R, FloorDb, "{:.0} dB");
        );

        let analysis = split(
            pick("FFT size", &FFT_OPTIONS[..], settings.fft_size, FftSize),
            pick("Hop divisor", &HOP_DIVISORS[..], hop_divisor, HopDivisor),
        );
        let details = form!(
            pick("Direction", FrequencyDirection::ALL, direction, Direction);
            slider!(
                "Rotation", settings.rotation as f32, QUARTER_TURNS, Rotation,
                quarter_turn_label(settings.rotation)
            );
            split(
                toggle("Frequency grid", settings.show_grid, ShowGrid),
                primary.then(|| toggle("Peak label", settings.show_peak_label, ShowPeakLabel)),
            );
            bars.then(|| slider!(
                "Bar count", settings.bar_count as f32, BARS_R, BarCount,
                settings.bar_count.to_string()
            ));
            bars.then(|| slider!(
                "Bar gap", settings.bar_gap, GAP_R, BarGap,
                format!("{:.0}%", settings.bar_gap * 100.0)
            ));
        );
        let details_summary = [
            bars.then(|| format!("{} bars", settings.bar_count)),
            settings.reverse_frequency.then(|| "Reversed".into()),
            (settings.rotation != 0).then(|| quarter_turn_label(settings.rotation)),
            settings.show_grid.then(|| "Grid".into()),
            (primary && settings.show_peak_label).then(|| "Peak label".into()),
        ];
    }
    "Sources" => sources;
    "View" => view;
    "Analysis" [[
        chip(format!("FFT {}", settings.fft_size)),
        chip(format!("Hop 1/{hop_divisor}")),
    ]] => analysis;
    "Details" [details_summary.into_iter().flatten().map(chip)] => details;
}

impl Pane {
    fn update_avg(&mut self, update: impl FnOnce(&mut AveragingControls) -> bool) -> bool {
        if !update(&mut self.averaging) {
            return false;
        }
        self.settings.averaging = match self.averaging.mode {
            AvgMode::None => AveragingMode::None,
            AvgMode::Exponential => AveragingMode::Exponential {
                time_constant: self.averaging.time_constant,
            },
            AvgMode::PeakHold => AveragingMode::PeakHold {
                decay_per_second: self.averaging.peak_decay,
            },
        };
        true
    }
}

fn split_averaging(avg: AveragingMode) -> AveragingControls {
    let default_time_constant = DEFAULT_SPECTRUM_TIME_CONSTANT;
    let default_peak_decay = DEFAULT_SPECTRUM_PEAK_DECAY;
    let (mode, time_constant, peak_decay) = match avg {
        AveragingMode::None => (AvgMode::None, default_time_constant, default_peak_decay),
        AveragingMode::Exponential { time_constant } => {
            (AvgMode::Exponential, time_constant, default_peak_decay)
        }
        AveragingMode::PeakHold { decay_per_second } => {
            (AvgMode::PeakHold, default_time_constant, decay_per_second)
        }
    };
    AveragingControls {
        mode,
        time_constant,
        peak_decay,
    }
}
