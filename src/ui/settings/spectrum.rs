// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::widgets::{
    FFT_OPTIONS, HOP_DIVISORS, SliderRange, card, get_closest_hop_divisor, palette_card, pick,
    set_if_changed, slide, split, toggle, update_f32_range, update_fft_size, update_hop_divisor,
    update_usize_from_f32,
};
use crate::persistence::settings::SpectrumSettings;
use crate::util::audio::{Channel, FrequencyScale};
use crate::visuals::options::{SpectrumDisplayMode, SpectrumWeightingMode};
use crate::visuals::spectrum::processor::{
    AveragingMode, MAX_SPECTRUM_DB_FLOOR, MAX_SPECTRUM_EXP_FACTOR, MAX_SPECTRUM_PEAK_DECAY,
    MIN_SPECTRUM_DB_FLOOR, MIN_SPECTRUM_EXP_FACTOR, MIN_SPECTRUM_PEAK_DECAY,
};
use iced::Element;

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

settings_messages!(pane, value {
    FftSize(usize) => update_fft_size(&mut pane.settings.fft_size, &mut pane.settings.hop_size, value);
    HopDivisor(usize) => update_hop_divisor(pane.settings.fft_size, &mut pane.settings.hop_size, value);
    Source(Channel) => set_if_changed(&mut pane.settings.source, value);
    SecondarySource(Channel) => set_if_changed(&mut pane.settings.secondary_source, value);
    FrequencyScale(FrequencyScale) => set_if_changed(&mut pane.settings.frequency_scale, value);
    ReverseFrequency(bool) => set_if_changed(&mut pane.settings.reverse_frequency, value);
    DisplayMode(SpectrumDisplayMode) => set_if_changed(&mut pane.settings.display_mode, value);
    WeightingMode(SpectrumWeightingMode) => set_if_changed(&mut pane.settings.weighting_mode, value);
    SecondaryWeightingMode(SpectrumWeightingMode) => set_if_changed(&mut pane.settings.secondary_weighting_mode, value);
    Averaging(AvgMode) => pane.update_avg(|avg| set_if_changed(&mut avg.mode, value));
    AvgFactor(f32) => pane.update_avg(|avg| update_f32_range(&mut avg.factor, value, EXP_R));
    PeakDecay(f32) => pane.update_avg(|avg| update_f32_range(&mut avg.peak_decay, value, DECAY_R));
    ShowGrid(bool) => set_if_changed(&mut pane.settings.show_grid, value);
    ShowPeakLabel(bool) => set_if_changed(&mut pane.settings.show_peak_label, value);
    FloorDb(f32) => update_f32_range(&mut pane.settings.floor_db, value, FLOOR_R);
    BarCount(f32) => update_usize_from_f32(&mut pane.settings.bar_count, value, BARS_R);
    BarGap(f32) => update_f32_range(&mut pane.settings.bar_gap, value, GAP_R);
    Highlight(f32) => update_f32_range(&mut pane.settings.highlight_threshold, value, HIGH_R);
});

impl Pane {
    pub(super) fn view(&self) -> Element<'_, Message> {
        let s = &self.settings;
        let hop_divisor = get_closest_hop_divisor(s.fft_size, s.hop_size);
        let dir = if s.reverse_frequency {
            "High <- Low"
        } else {
            "Low -> High"
        };

        let sources = split(
            controls!(8.0;
                pick("Primary source", Channel::ALL, s.source, Message::Source);
                pick("Primary weighting", SpectrumWeightingMode::ALL, s.weighting_mode, Message::WeightingMode);
            ),
            controls!(8.0;
                pick("Secondary source", Channel::ALL, s.secondary_source, Message::SecondarySource);
                pick("Secondary weighting", SpectrumWeightingMode::ALL, s.secondary_weighting_mode, Message::SecondaryWeightingMode);
            ),
        );
        let mut analysis = controls!(8.0;
            split(
                controls!(8.0;
                    pick("FFT size", &FFT_OPTIONS, s.fft_size, Message::FftSize);
                    pick("Hop divisor", &HOP_DIVISORS, hop_divisor, Message::HopDivisor);
                ),
                controls!(8.0;
                    pick("Frequency scale", FrequencyScale::ALL, s.frequency_scale, Message::FrequencyScale);
                    pick("Averaging", AvgMode::ALL, self.averaging.mode, Message::Averaging);
                ),
            );
        );
        analysis = match self.averaging.mode {
            AvgMode::Exponential => controls!(analysis;
                slider!("Exp factor", self.averaging.factor, EXP_R, Message::AvgFactor, "{:.2}");
            ),
            AvgMode::PeakHold => controls!(analysis;
                slider!("Peak decay", self.averaging.peak_decay, DECAY_R, Message::PeakDecay, "{:.1} dB/s");
            ),
            AvgMode::None => analysis,
        };

        let mut display = controls!(8.0;
            pick("Display", SpectrumDisplayMode::ALL, s.display_mode, Message::DisplayMode);
            split(
                controls!(8.0;
                    toggle(dir, s.reverse_frequency, Message::ReverseFrequency);
                    toggle("Frequency grid", s.show_grid, Message::ShowGrid);
                ),
                controls!(8.0;
                    toggle("Peak label", s.show_peak_label, Message::ShowPeakLabel);
                ),
            );
            slider!("Noise floor", s.floor_db, FLOOR_R, Message::FloorDb, "{:.0} dB");
        );
        if s.display_mode == SpectrumDisplayMode::Bar {
            display = controls!(display;
                slider!("Bar count", s.bar_count as f32, BARS_R, Message::BarCount, s.bar_count.to_string());
                slider!("Bar gap", s.bar_gap, GAP_R, Message::BarGap, format!("{:.0}%", s.bar_gap * 100.0));
            );
        }
        display = controls!(display;
            slider!(
                "Color floor", s.highlight_threshold, HIGH_R, Message::Highlight,
                format!("{:.0}%", s.highlight_threshold * 100.0)
            );
        );

        controls!(12.0;
            card("Sources", sources);
            card("Analysis", analysis);
            card("Display", display);
            palette_card(&self.palette, Message::Palette);
        )
        .into()
    }

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
