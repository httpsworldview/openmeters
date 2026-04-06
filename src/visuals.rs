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

use crate::persistence::settings::Channel;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_VIS_KEY: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_key() -> u64 {
    NEXT_VIS_KEY.fetch_add(1, Ordering::Relaxed)
}

// Data layout: contiguous channels `[ch0_s0..ch0_sN, ch1_s0..ch1_sN, ...]`.
// Returns `None` for `Channel::None`; callers must validate `stride > 0`.
#[inline]
pub(crate) fn project_channel_data(
    channel: Channel,
    data: &[f32],
    stride: usize,
    channels: usize,
) -> Option<Vec<f32>> {
    match channel {
        Channel::Left => data.get(..stride).map(<[f32]>::to_vec),
        Channel::Right => {
            let offset = if channels > 1 { stride } else { 0 };
            data.get(offset..offset + stride).map(<[f32]>::to_vec)
        }
        Channel::Mid => {
            let scale = 1.0 / channels.max(1) as f32;
            let mut out = vec![0.0f32; stride];
            for ch in data.chunks(stride).take(channels) {
                for (o, &s) in out.iter_mut().zip(ch) {
                    *o += s;
                }
            }
            out.iter_mut().for_each(|v| *v *= scale);
            Some(out)
        }
        Channel::Side => {
            let left = data.get(..stride)?;
            let right_offset = if channels > 1 { stride } else { 0 };
            let right = data
                .get(right_offset..right_offset + stride)
                .unwrap_or(left);
            Some(left.iter().zip(right).map(|(l, r)| (l - r) * 0.5).collect())
        }
        Channel::None => None,
    }
}

pub(crate) fn measure_text(s: &str, px: f32) -> iced::Size {
    use iced::advanced::graphics::text::Paragraph;
    use iced::advanced::text::{self, Paragraph as _};
    Paragraph::with_text(text::Text {
        content: s,
        bounds: iced::Size::INFINITE,
        size: iced::Pixels(px),
        font: iced::Font::default(),
        align_x: iced::alignment::Horizontal::Left.into(),
        align_y: iced::alignment::Vertical::Top,
        line_height: text::LineHeight::default(),
        shaping: text::Shaping::Basic,
        wrapping: text::Wrapping::None,
    })
    .min_bounds()
}

pub(crate) fn make_text(
    s: &str,
    px: f32,
    bounds: iced::Size,
) -> iced::advanced::text::Text<String> {
    use iced::advanced::text;
    text::Text {
        content: s.to_string(),
        bounds,
        size: iced::Pixels(px),
        font: iced::Font::default(),
        align_x: iced::alignment::Horizontal::Left.into(),
        align_y: iced::alignment::Vertical::Top,
        line_height: text::LineHeight::default(),
        shaping: text::Shaping::Basic,
        wrapping: text::Wrapping::None,
    }
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
        if (cfg.sample_rate - $sr).abs() > f32::EPSILON {
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

// Used by simple visualizations; spectrum/spectrogram/loudness roll their own widgets.
#[macro_export]
macro_rules! visualization_widget {
    ($widget:ident, $state:ty, $primitive:ty) => {
        #[derive(Debug)]
        pub struct $widget<'a> {
            state: &'a std::cell::RefCell<$state>,
        }

        impl<'a> $widget<'a> {
            pub fn new(state: &'a std::cell::RefCell<$state>) -> Self {
                Self { state }
            }
        }

        impl<M> iced::advanced::widget::Widget<M, iced::Theme, iced::Renderer> for $widget<'_> {
            fn tag(&self) -> iced::advanced::widget::tree::Tag {
                iced::advanced::widget::tree::Tag::stateless()
            }
            fn state(&self) -> iced::advanced::widget::tree::State {
                iced::advanced::widget::tree::State::new(())
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
                use iced::advanced::Renderer as _;
                use iced_wgpu::primitive::Renderer as _;
                let bounds = layout.bounds();
                let state = self.state.borrow();
                match state.visual_params(bounds) {
                    Some(params) => renderer.draw_primitive(bounds, <$primitive>::new(params)),
                    None => renderer.fill_quad(
                        iced::advanced::renderer::Quad {
                            bounds,
                            border: Default::default(),
                            shadow: Default::default(),
                            snap: true,
                        },
                        iced::Background::Color(theme.extended_palette().background.base.color),
                    ),
                }
            }
        }

        pub fn widget<'a, M: 'a>(state: &'a std::cell::RefCell<$state>) -> iced::Element<'a, M> {
            iced::Element::new($widget::new(state))
        }
    };
}
