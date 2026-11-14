// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

macro_rules! visualization_widget {
    (
        $widget:ident, $state:ty, $($update:ident,)?
        |$this:ident, $renderer:ident, $theme:ident, $bounds:ident| $draw:block
    ) => {
        struct $widget<'a> {
            state: &'a std::cell::RefCell<$state>,
        }

        impl<M> iced::advanced::widget::Widget<M, iced::Theme, iced::Renderer> for $widget<'_> {
            $crate::macros::widget_method!(layout
                iced::Size::new(iced::Length::Fill, iced::Length::Fill),
                |limits| limits.resolve(iced::Length::Fill, iced::Length::Fill, iced::Size::ZERO)
            );

            $($crate::macros::widget_method!(update M; this; _, event, layout, cursor, _, _, shell, _ => {
                if this.state.borrow_mut().$update(event, cursor, layout.bounds()) {
                    shell.request_redraw();
                    shell.capture_event();
                }
            });)?

            $crate::macros::widget_method!(draw $this; _, $renderer, $theme, _, layout, _, _ => {
                use iced_wgpu::primitive::Renderer as _;
                let $bounds = layout.bounds();
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
        $crate::visuals::visualization_widget!($widget, $state, |this, renderer, _theme, bounds| {
            for params in this.state.borrow_mut().visual_params(bounds) {
                renderer.draw_primitive(params.bounds, params);
            }
        });
    };
}

pub(in crate::visuals) use visualization_widget;

// `set_palette` from the stored palette, or the defaults in `palettes::$module`. `ramp` also
// sets the stops of the ramp its leading colors form.
macro_rules! palette_setter {
    (@ramp ramp, $state:ident, $palette:ident) => {
        PaletteSettings::write_positions(
            $palette,
            &defaults::DEFAULT_POSITIONS,
            &mut $state.stop_positions,
        );
        PaletteSettings::write_spreads($palette, &mut $state.stop_spreads);
    };
    ($module:ident $(, $ramp:ident)? $(=> $($geometry:ident),+)?) => {
        pub(in crate::visuals) fn set_palette(
            &mut self,
            palette: Option<&$crate::persistence::settings::PaletteSettings>,
        ) {
            use $crate::persistence::settings::PaletteSettings;
            use $crate::visuals::palettes::$module as defaults;
            self.palette = PaletteSettings::colors(palette, &defaults::COLORS);
            $($crate::visuals::palette_setter!(@ramp $ramp, self, palette);)?
            $($(self.$geometry.invalidate();)+)?
        }
    };
}

pub(in crate::visuals) use palette_setter;

pub mod loudness {
    pub mod processor;
    pub mod render;
    pub mod state;
}

pub mod oscilloscope {
    pub mod processor;
    pub mod render;
    pub mod state;
}

pub mod spectrogram {
    pub mod processor;
    pub mod render;
    pub mod state;
}

pub mod spectrum {
    pub mod processor;
    pub mod render;
    pub mod state;
}

pub mod stereometer {
    pub mod processor;
    pub mod render;
    pub mod state;
}

pub mod waveform {
    pub mod processor;
    pub mod render;
    pub mod state;
}

pub mod options {
    crate::macros::choice_enum!(#[derive(Default)] pub enum StereometerMode {
        Lissajous => "Lissajous",
        DotCloud => "Dot Cloud",
        #[default] DotCloudBands => "Dot Cloud (Bands)",
    });
    crate::macros::choice_enum!(#[derive(Default)] pub enum StereometerScale { Linear => "Linear", #[default] #[serde(alias = "exponential")] Scaled => "Scaled" });
    crate::macros::choice_enum!(#[derive(Default)] pub enum CorrelationMeterMode { Off => "Off", SingleBand => "Single Band", #[default] MultiBand => "Multi Band" });
    crate::macros::choice_enum!(#[derive(Default)] pub enum CorrelationMeterSide { Left => "Left", #[default] Right => "Right" });
    crate::macros::choice_enum!(#[derive(Default)] pub enum PianoRollOverlay { #[default] Off => "Off", Right => "Right", Left => "Left" });

    crate::macros::choice_enum!(pub enum MeterMode {
        LufsShortTerm => "LUFS Short-term",
        LufsMomentary => "LUFS Momentary",
        LufsIntegrated => "LUFS Integrated",
        RmsFast => "RMS Fast",
        RmsSlow => "RMS Slow",
        TruePeak => "True Peak",
    });

    crate::macros::choice_enum!(#[derive(Default)] pub enum SpectrumDisplayMode { #[default] Line => "Line", Bar => "Bar" });
    crate::macros::choice_enum!(#[derive(Default)] pub enum SpectrumWeightingMode { #[default] AWeighted => "A-Weighted", Raw => "Raw" });
    crate::macros::choice_enum!(#[derive(Default)] pub enum WaveformColorMode { #[default] Frequency => "Frequency Bands", Loudness => "Loudness", Static => "Static" });
    crate::macros::choice_enum!(#[derive(Default)] pub enum WaveformHistoryMode { Off => "Off", #[default] RmsFast => "RMS Fast", RmsSlow => "RMS Slow" });

    crate::macros::bounded! {
        // The share of each spectrum bar's slot left empty.
        pub struct BarGap(f32) in 0.0..=0.8;
        // Stereometer dots, in pixels.
        pub struct DotRadius(f32) in 0.5..=8.0;
        pub struct EighthTurns(i8) in -4..=4;
    }
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
