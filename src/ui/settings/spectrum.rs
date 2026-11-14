// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{
    DB_FLOOR_RANGE, FFT_OPTIONS, HOP_DIVISORS, fmt_ms, get_closest_hop_divisor, quarter_turn_label,
    quarter_turn_slider, set, settings_messages, settings_pane, settings_view, update_fft_size,
    update_hop_divisor,
};
use crate::persistence::settings::SpectrumSettings;
use crate::ui::widgets::{SliderRange, chip, pick, split, toggle};
use crate::util::Finite;
use crate::util::audio::{Channel, FrequencyScale, Seconds};
use crate::visuals::options::{self, SpectrumDisplayMode, SpectrumWeightingMode as WeightingMode};
use crate::visuals::spectrum::processor::{
    AveragingMode, DEFAULT_SPECTRUM_PEAK_DECAY, DEFAULT_SPECTRUM_TIME_CONSTANT,
};

const TIME_CONSTANT_RANGE: SliderRange = SliderRange::new(0.0, 1.0, 0.005);
const DECAY_RANGE: SliderRange = SliderRange::new(0.0, 120.0, 0.5);
const BARS_RANGE: SliderRange = SliderRange::new(8.0, 128.0, 1.0);
const GAP_RANGE: SliderRange = SliderRange::new(options::BarGap::MIN, options::BarGap::MAX, 0.05);

crate::macros::choice_enum!(pub(in crate::ui) enum AvgMode {
    None => "None",
    Exponential => "Exponential",
    PeakHold => "Peak hold",
});

crate::macros::choice_enum!(pub(in crate::ui) enum FrequencyDirection {
    LowToHigh => "Low -> High",
    HighToLow => "High -> Low",
});

// The pane remembers each mode's parameter while another mode is active.
settings_pane!(
    SpectrumSettings,
    extra_from_settings(settings) {
        time_constant: Seconds = match settings.averaging {
            AveragingMode::Exponential { time_constant } => time_constant,
            _ => DEFAULT_SPECTRUM_TIME_CONSTANT,
        },
        peak_decay: Finite = match settings.averaging {
            AveragingMode::PeakHold { decay_per_second } => decay_per_second,
            _ => DEFAULT_SPECTRUM_PEAK_DECAY,
        },
    }
);

fn averaging(mode: AvgMode, time_constant: Seconds, decay_per_second: Finite) -> AveragingMode {
    match mode {
        AvgMode::None => AveragingMode::None,
        AvgMode::Exponential => AveragingMode::Exponential { time_constant },
        AvgMode::PeakHold => AveragingMode::PeakHold { decay_per_second },
    }
}

settings_messages!(pane, settings, value {
    FftSize(usize) => update_fft_size(&mut settings.fft_size, &mut settings.hop_size, value);
    HopDivisor(usize) => update_hop_divisor(settings.fft_size, &mut settings.hop_size, value);
    Source(Channel) => set(&mut settings.source, value);
    SecondarySource(Channel) => set(&mut settings.secondary_source, value);
    Scale(FrequencyScale) => set(&mut settings.frequency_scale, value);
    Direction(FrequencyDirection) => {
        set(&mut settings.reverse_frequency, value == FrequencyDirection::HighToLow)
    };
    Rotation(i8) => set(&mut settings.rotation, value);
    Display(SpectrumDisplayMode) => set(&mut settings.display_mode, value);
    Weighting(WeightingMode) => set(&mut settings.weighting_mode, value);
    SecondaryWeighting(WeightingMode) => set(&mut settings.secondary_weighting_mode, value);
    Averaging(AvgMode) => {
        set(&mut settings.averaging, averaging(value, pane.time_constant, pane.peak_decay))
    };
    AvgTimeConstant(f32) => {
        matches!(settings.averaging, AveragingMode::Exponential { .. })
            && pane.time_constant.set(value)
            && set(&mut settings.averaging, AveragingMode::Exponential {
                time_constant: pane.time_constant,
            })
    };
    PeakDecay(f32) => {
        matches!(settings.averaging, AveragingMode::PeakHold { .. })
            && pane.peak_decay.set(value)
            && set(&mut settings.averaging, AveragingMode::PeakHold {
                decay_per_second: pane.peak_decay,
            })
    };
    ShowGrid(bool) => set(&mut settings.show_grid, value);
    ShowPeakLabel(bool) => set(&mut settings.show_peak_label, value);
    FloorDb(f32) => settings.floor_db.set(value);
    BarCount(usize) => set(&mut settings.bar_count, value);
    BarGap(f32) => settings.bar_gap.set(value);
});

settings_view! {
    pane as settings {
        use FrequencyDirection::{HighToLow, LowToHigh};
        let primary = settings.source != Channel::None;
        let secondary = settings.secondary_source != Channel::None;
        let bars = settings.display_mode == SpectrumDisplayMode::Bar;
        let hop_divisor = get_closest_hop_divisor(settings.fft_size.get(), settings.hop_size.get());
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
        let (avg_mode, averaging) = match settings.averaging {
            AveragingMode::None => (AvgMode::None, None),
            AveragingMode::Exponential { time_constant } => (AvgMode::Exponential, Some(slider!(
                "Time constant", time_constant.get(), TIME_CONSTANT_RANGE, AvgTimeConstant,
                fmt_ms(time_constant.get(), 0)
            ))),
            AveragingMode::PeakHold { decay_per_second } => (AvgMode::PeakHold, Some(slider!(
                "Peak decay", decay_per_second.get(), DECAY_RANGE, PeakDecay, "{:.1} dB/s"
            ))),
        };
        let view = form!(
            split(
                pick("Display", SpectrumDisplayMode::ALL, settings.display_mode, Display),
                pick("Frequency scale", FrequencyScale::ALL, settings.frequency_scale, Scale),
            );
            pick("Averaging", AvgMode::ALL, avg_mode, Averaging);
            averaging;
            slider!("Noise floor", settings.floor_db.get(), DB_FLOOR_RANGE, FloorDb, "{:.0} dB");
        );
    }
    "Sources" => sources;
    "View" => view;
    "Analysis" [[
        chip(format!("FFT {}", settings.fft_size.get())),
        chip(format!("Hop 1/{hop_divisor}")),
    ]] => split(
        pick("FFT size", &FFT_OPTIONS[..], settings.fft_size.get(), FftSize),
        pick("Hop divisor", &HOP_DIVISORS[..], hop_divisor, HopDivisor),
    );
    "Details" [[
        bars.then(|| format!("{} bars", settings.bar_count)),
        settings.reverse_frequency.then(|| "Reversed".into()),
        (settings.rotation != 0).then(|| quarter_turn_label(settings.rotation)),
        settings.show_grid.then(|| "Grid".into()),
        (primary && settings.show_peak_label).then(|| "Peak label".into()),
    ].into_iter().flatten().map(chip)] => form!(
        pick("Direction", FrequencyDirection::ALL, direction, Direction);
        quarter_turn_slider(settings.rotation, Rotation);
        split(
            toggle("Frequency grid", settings.show_grid, ShowGrid),
            primary.then(|| toggle("Peak label", settings.show_peak_label, ShowPeakLabel)),
        );
        bars.then(|| slider!(
            "Bar count", settings.bar_count as f32, BARS_RANGE,
            |value| BarCount(value.round() as usize), settings.bar_count.to_string()
        ));
        bars.then(|| slider!(
            "Bar gap", settings.bar_gap.get(), GAP_RANGE, BarGap,
            format!("{:.0}%", settings.bar_gap.get() * 100.0)
        ));
    );
}
