// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

macro_rules! visual_modules {
    ($($module:ident { $processor:ident, $config:ident, $state:ident }),+ $(,)?) => {
        $(pub mod $module {
            pub mod processor;
            pub mod render;
            pub mod state;
            pub(in crate::visuals) use processor::{$config, $processor};
            pub(in crate::visuals) use state::{widget, $state};
        })+
    };
}

macro_rules! visualization_widget {
    ($widget:ident, $state:ty, |$this:ident, $renderer:ident, $theme:ident, $bounds:ident| $draw:block) => {
        struct $widget<'a> {
            state: &'a std::cell::RefCell<$state>,
        }

        impl<M> iced::advanced::widget::Widget<M, iced::Theme, iced::Renderer> for $widget<'_> {
            crate::macros::widget_method!(layout
                iced::Size::new(iced::Length::Fill, iced::Length::Fill),
                |limits| limits.resolve(iced::Length::Fill, iced::Length::Fill, iced::Size::ZERO)
            );

            crate::macros::widget_method!(draw widget; _, renderer, theme, _, layout, _, _ => {
                use iced_wgpu::primitive::Renderer as _;
                let ($this, $renderer, $theme, $bounds) = (widget, renderer, theme, layout.bounds());
                $draw
            });
        }

        pub(in crate::visuals) fn widget<'a, M: 'a>(
            state: &'a std::cell::RefCell<$state>,
        ) -> iced::Element<'a, M> {
            iced::Element::new($widget { state })
        }
    };
    ($widget:ident, $state:ty) => {
        $crate::visuals::visualization_widget!($widget, $state, |this, renderer, theme, bounds| {
            let state = this.state.borrow();
            match state.visual_params(bounds) {
                Some(params) => renderer.draw_primitive(bounds, params),
                None => $crate::visuals::render::common::fill_rect(
                    renderer,
                    bounds,
                    theme.extended_palette().background.base.color,
                ),
            }
        });
    };
}

pub(in crate::visuals) use visualization_widget;

macro_rules! palette_setter {
    ($size:expr $(=> $geometry:ident)*) => {
        pub fn set_palette(&mut self, palette: &[iced::Color; $size]) {
            self.palette = *palette;
            $(self.$geometry.invalidate();)*
        }
    };
}

pub(in crate::visuals) use palette_setter;

visual_modules! {
    loudness { LoudnessProcessor, LoudnessConfig, LoudnessState },
    oscilloscope { OscilloscopeProcessor, OscilloscopeConfig, OscilloscopeState },
    spectrogram { SpectrogramProcessor, SpectrogramConfig, SpectrogramState },
    spectrum { SpectrumProcessor, SpectrumConfig, SpectrumState },
    stereometer { StereometerProcessor, StereometerConfig, StereometerState },
    waveform { WaveformProcessor, WaveformConfig, WaveformState },
}

pub mod options {
    crate::macros::choice_enum!(pub enum StereometerMode {
        Lissajous => "Lissajous",
        #[default] DotCloud => "Dot Cloud",
        DotCloudBands => "Dot Cloud (Bands)",
    });
    crate::macros::choice_enum!(pub enum StereometerScale { Linear => "Linear", #[default] #[serde(alias = "exponential")] Scaled => "Scaled" });
    crate::macros::choice_enum!(pub enum CorrelationMeterMode { Off => "Off", SingleBand => "Single Band", #[default] MultiBand => "Multi Band" });
    crate::macros::choice_enum!(pub enum CorrelationMeterSide { Left => "Left", #[default] Right => "Right" });
    crate::macros::choice_enum!(pub enum PianoRollOverlay { #[default] Off => "Off", Right => "Right", Left => "Left" });

    crate::macros::choice_enum!(no_default pub enum MeterMode {
        LufsShortTerm => "LUFS Short-term",
        LufsMomentary => "LUFS Momentary",
        RmsFast => "RMS Fast",
        RmsSlow => "RMS Slow",
        TruePeak => "True Peak",
    });

    crate::macros::choice_enum!(pub enum SpectrumDisplayMode { #[default] Line => "Line", Bar => "Bar" });
    crate::macros::choice_enum!(pub enum SpectrumWeightingMode { #[default] AWeighted => "A-Weighted", Raw => "Raw" });
    crate::macros::choice_enum!(pub enum WaveformColorMode { #[default] Frequency => "Frequency Bands", Loudness => "Loudness", Static => "Static" });
    crate::macros::choice_enum!(pub enum WaveformHistoryMode { #[default] Off => "Off", RmsFast => "RMS Fast", RmsSlow => "RMS Slow" });
}

pub mod palettes;
pub mod registry;
pub mod render {
    pub mod common;
}

use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_VIS_KEY: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy)]
pub(crate) struct GeometryKey {
    id: u64,
    revision: u64,
}

impl GeometryKey {
    fn new() -> Self {
        Self {
            id: next_key(),
            revision: 0,
        }
    }

    fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

pub(in crate::visuals) fn next_key() -> u64 {
    NEXT_VIS_KEY.fetch_add(1, Ordering::Relaxed)
}
