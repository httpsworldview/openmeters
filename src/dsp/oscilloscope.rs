//! Oscilloscope/triggered waveform DSP scaffolding.

use super::{AudioBlock, AudioProcessor, ProcessorUpdate, Reconfigurable};

/// Options controlling oscilloscope behaviour.
#[derive(Debug, Clone, Copy)]
pub struct OscilloscopeConfig {
    pub sample_rate: f32,
    /// Duration of the captured segment in seconds.
    pub segment_duration: f32,
    /// Trigger level expressed in linear amplitude.
    pub trigger_level: f32,
    /// Whether to use rising-edge or falling-edge detection.
    pub trigger_rising: bool,
}

impl Default for OscilloscopeConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            segment_duration: 0.02,
            trigger_level: 0.05,
            trigger_rising: true,
        }
    }
}

/// Snapshot handed to the renderer containing oscilloscope samples.
#[derive(Debug, Clone)]
pub struct OscilloscopeSnapshot {
    pub channels: usize,
    pub samples: Vec<f32>,
}

impl Default for OscilloscopeSnapshot {
    fn default() -> Self {
        Self {
            channels: 2,
            samples: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct OscilloscopeProcessor {
    config: OscilloscopeConfig,
    snapshot: OscilloscopeSnapshot,
}

impl OscilloscopeProcessor {
    pub fn new(config: OscilloscopeConfig) -> Self {
        Self {
            config,
            snapshot: OscilloscopeSnapshot::default(),
        }
    }
}

impl AudioProcessor for OscilloscopeProcessor {
    type Output = OscilloscopeSnapshot;

    fn process_block(&mut self, _block: &AudioBlock<'_>) -> ProcessorUpdate<Self::Output> {
        // TODO: implement trigger detection & segment extraction.
        ProcessorUpdate::None
    }

    fn reset(&mut self) {
        self.snapshot = OscilloscopeSnapshot::default();
    }
}

impl Reconfigurable<OscilloscopeConfig> for OscilloscopeProcessor {
    fn update_config(&mut self, config: OscilloscopeConfig) {
        self.config = config;
        self.reset();
    }
}
