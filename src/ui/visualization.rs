mod loudness;
mod oscilloscope;
mod spectrogram;
mod spectrum;
mod stereometer;
pub mod visual_manager;
mod waveform;

use crate::ui::settings::ChannelMode;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_VIS_KEY: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_key() -> u64 {
    NEXT_VIS_KEY.fetch_add(1, Ordering::Relaxed)
}

// Projects channel data according to a channel mode.
//
// Data layout: contiguous channels `[ch0_s0..ch0_sN, ch1_s0..ch1_sN, ...]`
// - `mode`: channel selection/mixing mode
// - `data`: samples with `stride` samples per channel
// - `stride`: number of samples per channel
// - `channels`: number of channels in the input data
#[inline]
pub(crate) fn project_channel_data(
    mode: ChannelMode,
    data: &[f32],
    stride: usize,
    channels: usize,
) -> Vec<f32> {
    match mode {
        ChannelMode::Both => data.to_vec(),
        ChannelMode::Left => data.get(..stride).map(|s| s.to_vec()).unwrap_or_default(),
        ChannelMode::Right => {
            let offset = if channels > 1 { stride } else { 0 };
            data.get(offset..offset + stride)
                .map(|s| s.to_vec())
                .unwrap_or_default()
        }
        ChannelMode::Mono => {
            let scale = 1.0 / channels.max(1) as f32;
            (0..stride)
                .map(|i| {
                    data.chunks(stride)
                        .take(channels)
                        .filter_map(|ch| ch.get(i))
                        .sum::<f32>()
                        * scale
                })
                .collect()
        }
    }
}

// creates a visualization. very simple macro to reduce boilerplate,
// it is used thrice. spectrum, spectrogram, loudness visualizations do
// *not* use this macro, as they have more complex requirements.
#[macro_export]
macro_rules! visualization_widget {
    ($widget:ident, $state:ty, $primitive:ty, |$st:ident, $bounds:ident| $params_expr:expr, |$p:ident| $prim_expr:expr) => {
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
                let $bounds = layout.bounds();
                let $st = self.state.borrow();
                match $params_expr {
                    Some($p) => renderer.draw_primitive($bounds, $prim_expr),
                    None => renderer.fill_quad(
                        iced::advanced::renderer::Quad {
                            bounds: $bounds,
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
