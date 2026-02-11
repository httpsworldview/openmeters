// UI wrapper around the oscilloscope DSP processor and renderer.

use crate::audio::meter_tap::MeterFormat;
use crate::dsp::oscilloscope::{
    OscilloscopeConfig, OscilloscopeProcessor as CoreOscilloscopeProcessor, OscilloscopeSnapshot,
};
use crate::dsp::{AudioBlock, AudioProcessor, Reconfigurable};
use crate::ui::render::oscilloscope::{OscilloscopeParams, OscilloscopePrimitive};
use crate::ui::settings::{ChannelMode, OscilloscopeSettings};
use crate::ui::theme;
use crate::util::audio::project_channel_data;
use crate::visualization_widget;
use iced::Color;

const MAX_PERSISTENCE: f32 = 0.98;
const FILL_ALPHA: f32 = 0.15;

#[derive(Debug, Clone)]
pub(crate) struct OscilloscopeProcessor {
    inner: CoreOscilloscopeProcessor,
}

impl OscilloscopeProcessor {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            inner: CoreOscilloscopeProcessor::new(OscilloscopeConfig {
                sample_rate,
                ..Default::default()
            }),
        }
    }

    pub fn ingest(&mut self, samples: &[f32], format: MeterFormat) -> Option<OscilloscopeSnapshot> {
        if samples.is_empty() {
            return None;
        }

        let sample_rate = format.sample_rate.max(1.0);
        let mut config = self.inner.config();
        if (config.sample_rate - sample_rate).abs() > f32::EPSILON {
            config.sample_rate = sample_rate;
            self.inner.update_config(config);
        }

        self.inner.process_block(&AudioBlock::now(
            samples,
            format.channels.max(1),
            sample_rate,
        ))
    }

    pub fn update_config(&mut self, config: OscilloscopeConfig) {
        self.inner.update_config(config);
    }

    pub fn config(&self) -> OscilloscopeConfig {
        self.inner.config()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OscilloscopeState {
    snapshot: OscilloscopeSnapshot,
    style: OscilloscopeStyle,
    persistence: f32,
    channel_mode: ChannelMode,
    key: u64,
}

impl OscilloscopeState {
    pub fn new() -> Self {
        let defaults = OscilloscopeSettings::default();
        Self {
            snapshot: OscilloscopeSnapshot::default(),
            style: OscilloscopeStyle::default(),
            persistence: defaults.persistence,
            channel_mode: defaults.channel_mode,
            key: super::next_key(),
        }
    }

    pub fn update_view_settings(&mut self, persistence: f32, channel_mode: ChannelMode) {
        self.persistence = persistence.clamp(0.0, 1.0);
        let mode_changed = self.channel_mode != channel_mode;
        self.channel_mode = channel_mode;
        if mode_changed {
            self.snapshot = Self::project_channels(&self.snapshot, channel_mode);
        }
    }

    pub fn set_palette(&mut self, palette: &[Color]) {
        self.style.colors.clear();
        self.style.colors.extend_from_slice(palette);
    }

    pub fn palette(&self) -> &[Color] {
        &self.style.colors
    }

    pub fn apply_snapshot(&mut self, snapshot: &OscilloscopeSnapshot) {
        let projected = Self::project_channels(snapshot, self.channel_mode);

        if !projected.samples.is_empty()
            && !self.snapshot.samples.is_empty()
            && projected.samples.len() == self.snapshot.samples.len()
        {
            let persistence = self.persistence.clamp(0.0, MAX_PERSISTENCE);
            if persistence > f32::EPSILON {
                let fresh = 1.0 - persistence;
                for (current, incoming) in self.snapshot.samples.iter_mut().zip(&projected.samples)
                {
                    *current = *current * persistence + incoming * fresh;
                }
                return;
            }
        }

        self.snapshot = projected;
    }

    pub fn channel_mode(&self) -> ChannelMode {
        self.channel_mode
    }

    pub fn persistence(&self) -> f32 {
        self.persistence
    }

    fn project_channels(source: &OscilloscopeSnapshot, mode: ChannelMode) -> OscilloscopeSnapshot {
        let (ch, spc) = (source.channels.max(1), source.samples_per_channel);
        if spc == 0 || source.samples.len() < ch * spc {
            return OscilloscopeSnapshot::default();
        }
        OscilloscopeSnapshot {
            channels: mode.output_channels(ch),
            samples_per_channel: spc,
            samples: project_channel_data(mode, &source.samples, spc, ch),
        }
    }

    pub fn visual_params(&self, bounds: iced::Rectangle) -> Option<OscilloscopeParams> {
        let channels = self.snapshot.channels.max(1);
        let samples_per_channel = self.snapshot.samples_per_channel;
        let required = channels.saturating_mul(samples_per_channel);

        if samples_per_channel < 2 || self.snapshot.samples.len() < required {
            return None;
        }

        let colors = self
            .style
            .colors
            .iter()
            .cycle()
            .take(channels)
            .map(|c| theme::color_to_rgba(*c))
            .collect();

        Some(OscilloscopeParams {
            key: self.key,
            bounds,
            channels,
            samples_per_channel,
            samples: self.snapshot.samples.clone(),
            colors,
            fill_alpha: FILL_ALPHA,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OscilloscopeStyle {
    pub colors: Vec<Color>,
}

impl Default for OscilloscopeStyle {
    fn default() -> Self {
        Self {
            colors: theme::oscilloscope::COLORS.to_vec(),
        }
    }
}

visualization_widget!(
    Oscilloscope,
    OscilloscopeState,
    OscilloscopePrimitive,
    |state, bounds| state.visual_params(bounds),
    |params| OscilloscopePrimitive::new(params)
);
