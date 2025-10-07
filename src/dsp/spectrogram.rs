//! Spectrogram DSP implementation built on a short-time Fourier transform.

use super::{AudioBlock, AudioProcessor, ProcessorUpdate, Reconfigurable};
use rustfft::{Fft, FftPlanner, num_complex::Complex32};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

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

impl WindowKind {
    fn coefficients(self, len: usize) -> Vec<f32> {
        match self {
            WindowKind::Rectangular => vec![1.0; len],
            WindowKind::Hann => (0..len)
                .map(|n| {
                    let phase = (n as f32) * core::f32::consts::TAU / (len as f32);
                    0.5 * (1.0 - phase.cos())
                })
                .collect(),
            WindowKind::Hamming => (0..len)
                .map(|n| {
                    let phase = (n as f32) * core::f32::consts::TAU / (len as f32);
                    0.54 - 0.46 * phase.cos()
                })
                .collect(),
            WindowKind::Blackman => {
                let a0 = 0.42;
                let a1 = 0.5;
                let a2 = 0.08;
                (0..len)
                    .map(|n| {
                        let phase = (n as f32) * core::f32::consts::TAU / (len as f32);
                        a0 - a1 * phase.cos() + a2 * (2.0 * phase).cos()
                    })
                    .collect()
            }
        }
    }
}

/// One column of log-power magnitudes.
#[derive(Debug, Clone)]
pub struct SpectrogramColumn {
    pub timestamp: Instant,
    pub magnitudes_db: Arc<[f32]>,
}

/// Spectrogram history buffer (ring of columns).
#[derive(Debug, Clone)]
pub struct SpectrogramSnapshot {
    pub fft_size: usize,
    pub hop_size: usize,
    pub sample_rate: f32,
    pub columns: Vec<SpectrogramColumn>,
}

impl Default for SpectrogramSnapshot {
    fn default() -> Self {
        Self {
            fft_size: 2048,
            hop_size: 512,
            sample_rate: SpectrogramConfig::default().sample_rate,
            columns: Vec::new(),
        }
    }
}

pub struct SpectrogramProcessor {
    config: SpectrogramConfig,
    snapshot: SpectrogramSnapshot,
    planner: FftPlanner<f32>,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    fft_buffer: Vec<Complex32>,
    magnitude_buffer: Vec<f32>,
    pcm_buffer: VecDeque<f32>,
    buffer_start_index: u64,
    start_instant: Option<Instant>,
}

impl SpectrogramProcessor {
    pub fn new(config: SpectrogramConfig) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(config.fft_size);
        let window = config.window.coefficients(config.fft_size);
        Self {
            snapshot: SpectrogramSnapshot {
                fft_size: config.fft_size,
                hop_size: config.hop_size,
                sample_rate: config.sample_rate,
                columns: Vec::new(),
            },
            fft_buffer: vec![Complex32::default(); config.fft_size],
            pcm_buffer: VecDeque::with_capacity(config.fft_size * 2),
            buffer_start_index: 0,
            start_instant: None,
            config,
            planner,
            fft,
            window,
            magnitude_buffer: vec![0.0; config.fft_size / 2 + 1],
        }
    }

    pub fn config(&self) -> SpectrogramConfig {
        self.config
    }

    fn rebuild_fft(&mut self) {
        self.fft = self.planner.plan_fft_forward(self.config.fft_size);
        self.window = self.config.window.coefficients(self.config.fft_size);
        self.fft_buffer
            .resize(self.config.fft_size, Complex32::default());
        self.magnitude_buffer
            .resize(self.config.fft_size / 2 + 1, 0.0);
        self.pcm_buffer.reserve(self.config.fft_size);
        self.snapshot.fft_size = self.config.fft_size;
        self.snapshot.hop_size = self.config.hop_size;
        self.snapshot.sample_rate = self.config.sample_rate;
        self.snapshot.columns.clear();
        self.pcm_buffer.clear();
        self.buffer_start_index = 0;
        self.start_instant = None;
    }

    fn ensure_fft_capacity(&mut self) {
        if self.fft_buffer.len() != self.config.fft_size {
            self.rebuild_fft();
        }
    }

    fn process_ready_windows(&mut self) -> bool {
        let mut updated = false;
        let fft_size = self.config.fft_size;
        if fft_size == 0 || self.config.hop_size == 0 {
            return false;
        }

        let hop = self.config.hop_size;
        let bins = fft_size / 2 + 1;

        while self.pcm_buffer.len() >= fft_size {
            for (idx, sample) in self.pcm_buffer.iter().take(fft_size).enumerate() {
                let windowed = sample * self.window[idx];
                self.fft_buffer[idx] = Complex32::new(windowed, 0.0);
            }

            self.fft.process(&mut self.fft_buffer);

            let scale = 1.0 / (fft_size as f32);
            for (bin, value) in self.magnitude_buffer.iter_mut().enumerate().take(bins) {
                let complex = self.fft_buffer[bin];
                let mut magnitude = complex.norm() * scale;
                if bin > 0 && bin + 1 < bins {
                    magnitude *= 2.0;
                }
                let magnitude_db = 20.0f32 * magnitude.max(1.0e-9).log10();
                *value = magnitude_db.max(-120.0f32);
            }

            let column_start_index = self.buffer_start_index;
            let center_index = column_start_index + (fft_size / 2) as u64;
            let timestamp = self
                .start_instant
                .map(|start| start + duration_from_samples(center_index, self.config.sample_rate))
                .unwrap_or_else(Instant::now);

            let magnitudes = Arc::from(self.magnitude_buffer[..bins].to_vec().into_boxed_slice());
            self.snapshot.columns.push(SpectrogramColumn {
                timestamp,
                magnitudes_db: magnitudes,
            });

            if self.snapshot.columns.len() > self.config.history_length {
                let overflow = self.snapshot.columns.len() - self.config.history_length;
                self.snapshot.columns.drain(0..overflow);
            }

            for _ in 0..hop {
                self.pcm_buffer.pop_front();
            }
            self.buffer_start_index += hop as u64;
            updated = true;
        }

        updated
    }
}

impl AudioProcessor for SpectrogramProcessor {
    type Output = SpectrogramSnapshot;

    fn process_block(&mut self, block: &AudioBlock<'_>) -> ProcessorUpdate<Self::Output> {
        if block.frame_count() == 0 || block.channels == 0 {
            return ProcessorUpdate::None;
        }

        if self.config.sample_rate <= 0.0 {
            self.config.sample_rate = block.sample_rate;
            self.snapshot.sample_rate = block.sample_rate;
        } else if (self.config.sample_rate - block.sample_rate).abs() > f32::EPSILON {
            self.config.sample_rate = block.sample_rate;
            self.snapshot.sample_rate = block.sample_rate;
        }

        if self.start_instant.is_none() {
            self.start_instant = Some(block.timestamp);
        }

        self.ensure_fft_capacity();

        let channels = block.channels;
        for frame in block.samples.chunks_exact(channels) {
            let mono = frame.iter().copied().sum::<f32>() / channels as f32;
            self.pcm_buffer.push_back(mono);
        }

        let updated = self.process_ready_windows();
        if updated {
            ProcessorUpdate::Snapshot(self.snapshot.clone())
        } else {
            ProcessorUpdate::None
        }
    }

    fn reset(&mut self) {
        self.snapshot.columns.clear();
        self.snapshot.fft_size = self.config.fft_size;
        self.snapshot.hop_size = self.config.hop_size;
        self.snapshot.sample_rate = self.config.sample_rate;
        self.pcm_buffer.clear();
        self.buffer_start_index = 0;
        self.start_instant = None;
    }
}

impl Reconfigurable<SpectrogramConfig> for SpectrogramProcessor {
    fn update_config(&mut self, config: SpectrogramConfig) {
        self.config = config;
        self.rebuild_fft();
    }
}

fn duration_from_samples(sample_index: u64, sample_rate: f32) -> Duration {
    if sample_rate <= 0.0 {
        return Duration::default();
    }
    let seconds = sample_index as f64 / sample_rate as f64;
    Duration::from_secs_f64(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::{AudioBlock, ProcessorUpdate};
    use std::time::Instant;

    fn make_block(samples: Vec<f32>, channels: usize, sample_rate: f32) -> AudioBlock<'static> {
        AudioBlock::new(
            Box::leak(samples.into_boxed_slice()),
            channels,
            sample_rate,
            Instant::now(),
        )
    }

    #[test]
    fn detects_sine_frequency_peak() {
        let config = SpectrogramConfig {
            fft_size: 1024,
            hop_size: 512,
            history_length: 8,
            sample_rate: 48_000.0,
            window: WindowKind::Hann,
        };
        let mut processor = SpectrogramProcessor::new(config);

        let freq = 1_000.0;
        let frames = config.fft_size * 2;
        let mut samples = Vec::with_capacity(frames);
        for n in 0..frames {
            let t = n as f32 / config.sample_rate;
            samples.push((2.0 * core::f32::consts::PI * freq * t).sin());
        }

        let block_samples = samples.clone();
        let block = make_block(block_samples, 1, config.sample_rate);

        let result = processor.process_block(&block);
        let snapshot = match result {
            ProcessorUpdate::Snapshot(snapshot) => snapshot,
            ProcessorUpdate::None => panic!("expected snapshot"),
        };

        assert!(!snapshot.columns.is_empty());
        let last = snapshot.columns.last().unwrap();
        let bin_hz = config.sample_rate / config.fft_size as f32;
        let max_index = last
            .magnitudes_db
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(idx, _)| idx)
            .unwrap();
        let peak_freq = max_index as f32 * bin_hz;
        assert!((peak_freq - freq).abs() < bin_hz * 1.5);
    }

    #[test]
    fn history_respects_limit() {
        let config = SpectrogramConfig {
            history_length: 4,
            ..SpectrogramConfig::default()
        };
        let mut processor = SpectrogramProcessor::new(config);
        let frames = config.fft_size * 4;
        let samples = vec![0.0f32; frames];
        let block = make_block(samples, 1, config.sample_rate);
        let _ = processor.process_block(&block);
        assert!(processor.snapshot.columns.len() <= config.history_length);
    }
}
