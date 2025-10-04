//! Spectrogram DSP scaffolding.

use super::{AudioBlock, AudioProcessor, ProcessorUpdate, Reconfigurable};

/// Configuration for spectrogram FFT analysis.
#[derive(Debug, Clone, Copy)]
pub struct SpectrogramConfig {
    pub sample_rate: f32,
    /// FFT size (must be a power of two for radix-2 implementations).
    pub fft_size: usize,
    /// Hop size between successive frames.
    pub hop_size: usize,
    /// Optional Hann/Hamming/Blackman window selection.
    pub window: WindowKind,
    /// Maximum retained history columns.
    pub history_length: usize,
}

impl Default for SpectrogramConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            fft_size: 2048,
            hop_size: 512,
            window: WindowKind::Hann,
            history_length: 120,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    Rectangular,
    Hann,
    Hamming,
    Blackman,
}

/// One column of log-power magnitudes.
#[derive(Debug, Clone)]
pub struct SpectrogramColumn {
    pub timestamp: std::time::Instant,
    pub magnitudes: Vec<f32>,
}

/// Spectrogram history buffer (ring of columns).
#[derive(Debug, Clone)]
pub struct SpectrogramSnapshot {
    pub fft_size: usize,
    pub columns: Vec<SpectrogramColumn>,
}

impl Default for SpectrogramSnapshot {
    fn default() -> Self {
        Self {
            fft_size: 2048,
            columns: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SpectrogramProcessor {
    config: SpectrogramConfig,
    snapshot: SpectrogramSnapshot,
}

impl SpectrogramProcessor {
    pub fn new(config: SpectrogramConfig) -> Self {
        Self {
            config,
            snapshot: SpectrogramSnapshot::default(),
        }
    }
}

impl AudioProcessor for SpectrogramProcessor {
    type Output = SpectrogramSnapshot;

    fn process_block(&mut self, _block: &AudioBlock<'_>) -> ProcessorUpdate<Self::Output> {
        // TODO: perform windowed FFT and update the history buffer.
        ProcessorUpdate::None
    }

    fn reset(&mut self) {
        self.snapshot = SpectrogramSnapshot::default();
    }
}

impl Reconfigurable<SpectrogramConfig> for SpectrogramProcessor {
    fn update_config(&mut self, config: SpectrogramConfig) {
        self.config = config;
        self.reset();
    }
}
