// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

pub mod loudness;
pub mod oscilloscope;
pub mod palettes;
pub mod registry;
pub mod render;
pub mod spectrogram;
pub mod spectrum;
pub mod stereometer;
pub mod waveform;

use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_VIS_KEY: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_key() -> u64 {
    NEXT_VIS_KEY.fetch_add(1, Ordering::Relaxed)
}

#[macro_export]
macro_rules! vis_processor {
    (@struct_new $name:ident, $core:ty, $config:ident) => {
        pub(crate) struct $name { inner: $core }
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
        impl $name {
            pub fn new(sample_rate: f32) -> Self {
                Self { inner: <$core>::new($config { sample_rate, ..Default::default() }) }
            }
        }
    };
    (@config $name:ident, $config:ident) => {
        impl $name {
            pub fn config(&self) -> $config { self.inner.config() }
            pub fn update_config(&mut self, c: $config) {
                use $crate::dsp::Reconfigurable as _;
                self.inner.update_config(c);
            }
        }
    };
    (@sync_rate $self_:ident, $sr:ident) => {
        use $crate::dsp::Reconfigurable as _;
        let mut cfg = $self_.inner.config();
        if !cfg.sample_rate.is_finite() || (cfg.sample_rate - $sr).abs() > f32::EPSILON {
            cfg.sample_rate = $sr;
            $self_.inner.update_config(cfg);
        }
    };
    (@ingest $name:ident, $output:ty $(, $sync:ident)?) => {
        impl $name {
            pub fn ingest(
                &mut self,
                samples: &[f32],
                format: $crate::infra::pipewire::meter_tap::MeterFormat,
            ) -> Option<$output> {
                use $crate::dsp::AudioProcessor as _;
                if samples.is_empty() { return None; }
                let sr = format.sample_rate.max(1.0);
                $($crate::vis_processor!(@$sync self, sr);)?
                self.inner.process_block(&$crate::dsp::AudioBlock::now(
                    samples, format.channels.max(1), sr,
                ))
            }
        }
    };
    ($name:ident, $core:ty, $config:ident, $output:ty, no_config) => {
        $crate::vis_processor!(@struct_new $name, $core, $config);
        $crate::vis_processor!(@ingest $name, $output);
    };
    ($name:ident, $core:ty, $config:ident, $output:ty $(, $sync:ident)?) => {
        $crate::vis_processor!(@struct_new $name, $core, $config);
        $crate::vis_processor!(@config $name, $config);
        $crate::vis_processor!(@ingest $name, $output $(, $sync)?);
    };
}

#[macro_export]
macro_rules! visualization_widget {
    (@base $widget:ident, $state:ty, |$this:ident, $renderer:ident, $theme:ident, $bounds:ident| $draw:block) => {
        #[derive(Debug)]
        pub(crate) struct $widget<'a> {
            state: &'a std::cell::RefCell<$state>,
        }

        impl<'a> $widget<'a> {
            pub(crate) fn new(state: &'a std::cell::RefCell<$state>) -> Self {
                Self { state }
            }
        }

        impl<M> iced::advanced::widget::Widget<M, iced::Theme, iced::Renderer> for $widget<'_> {
            fn tag(&self) -> iced::advanced::widget::tree::Tag {
                iced::advanced::widget::tree::Tag::stateless()
            }

            fn state(&self) -> iced::advanced::widget::tree::State {
                iced::advanced::widget::tree::State::None
            }

            fn size(&self) -> iced::Size<iced::Length> {
                iced::Size::new(iced::Length::Fill, iced::Length::Fill)
            }

            fn children(&self) -> Vec<iced::advanced::widget::Tree> {
                Vec::new()
            }

            fn diff(&self, _: &mut iced::advanced::widget::Tree) {}

            fn layout(
                &mut self,
                _: &mut iced::advanced::widget::Tree,
                _: &iced::Renderer,
                limits: &iced::advanced::layout::Limits,
            ) -> iced::advanced::layout::Node {
                iced::advanced::layout::Node::new(limits.resolve(
                    iced::Length::Fill,
                    iced::Length::Fill,
                    iced::Size::ZERO,
                ))
            }

            fn draw(
                &self,
                _: &iced::advanced::widget::Tree,
                renderer: &mut iced::Renderer,
                theme: &iced::Theme,
                _: &iced::advanced::renderer::Style,
                layout: iced::advanced::Layout<'_>,
                _: iced::advanced::mouse::Cursor,
                _: &iced::Rectangle,
            ) {
                use iced_wgpu::primitive::Renderer as _;
                let ($this, $renderer, $theme, $bounds) = (self, renderer, theme, layout.bounds());
                $draw
            }
        }

        pub(crate) fn widget<'a, M: 'a>(state: &'a std::cell::RefCell<$state>) -> iced::Element<'a, M> {
            iced::Element::new($widget::new(state))
        }
    };
    ($widget:ident, $state:ty, |$this:ident, $renderer:ident, $theme:ident, $bounds:ident| $draw:block) => {
        $crate::visualization_widget!(@base $widget, $state, |$this, $renderer, $theme, $bounds| $draw);
    };
    ($widget:ident, $state:ty, $primitive:ty) => {
        $crate::visualization_widget!(@base $widget, $state, |this, renderer, theme, bounds| {
            let state = this.state.borrow();
            match state.visual_params(bounds) {
                Some(params) => renderer.draw_primitive(bounds, <$primitive>::new(params)),
                None => $crate::visuals::render::common::fill_rect(
                    renderer,
                    bounds,
                    theme.extended_palette().background.base.color,
                ),
            }
        });
    };
}
