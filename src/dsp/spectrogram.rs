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
            fft_size: 8192,
            hop_size: 256,
            window: WindowKind::Hann,
            history_length: 960,
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
    pub magnitudes_db: Arc<Vec<f32>>,
}

/// Incremental update emitted by the spectrogram processor.
#[derive(Debug, Clone)]
pub struct SpectrogramUpdate {
    pub fft_size: usize,
    pub hop_size: usize,
    pub sample_rate: f32,
    pub history_length: usize,
    pub reset: bool,
    pub new_columns: Vec<SpectrogramColumn>,
}

#[derive(Debug, Clone)]
struct SpectrogramHistory {
    columns: VecDeque<SpectrogramColumn>,
    capacity: usize,
}

impl SpectrogramHistory {
    fn new(capacity: usize) -> Self {
        Self {
            columns: VecDeque::with_capacity(capacity.max(1)),
            capacity,
        }
    }

    fn set_capacity(&mut self, capacity: usize) -> Vec<SpectrogramColumn> {
        self.capacity = capacity;
        let mut evicted = Vec::new();
        if capacity == 0 {
            evicted.extend(self.columns.drain(..));
            return evicted;
        }

        while self.columns.len() > capacity {
            if let Some(column) = self.columns.pop_front() {
                evicted.push(column);
            }
        }
        evicted
    }

    fn clear(&mut self) -> Vec<SpectrogramColumn> {
        self.columns.drain(..).collect()
    }

    fn push(&mut self, column: SpectrogramColumn) -> Option<SpectrogramColumn> {
        if self.capacity == 0 {
            return Some(column);
        }

        let evicted = if self.columns.len() == self.capacity {
            self.columns.pop_front()
        } else {
            None
        };
        self.columns.push_back(column);
        evicted
    }

    fn len(&self) -> usize {
        self.columns.len()
    }
}

#[derive(Debug, Clone)]
struct SampleBuffer {
    data: Vec<f32>,
    read: usize,
    len: usize,
}

impl SampleBuffer {
    fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            data: vec![0.0; capacity],
            read: 0,
            len: 0,
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn push(&mut self, sample: f32) {
        if self.len == self.data.len() {
            let new_cap = (self.data.len() * 2).max(1);
            self.grow_to(new_cap);
        }

        let write = (self.read + self.len) % self.data.len();
        self.data[write] = sample;
        self.len += 1;
    }

    fn for_each_front<F>(&self, count: usize, mut f: F)
    where
        F: FnMut(usize, f32),
    {
        assert!(count <= self.len);
        let mut idx = self.read;
        let capacity = self.data.len();
        for pos in 0..count {
            f(pos, self.data[idx]);
            idx += 1;
            if idx == capacity {
                idx = 0;
            }
        }
    }

    fn consume(&mut self, count: usize) {
        assert!(count <= self.len);
        if count == 0 {
            return;
        }
        self.read = (self.read + count) % self.data.len();
        self.len -= count;
    }

    fn clear(&mut self) {
        self.read = 0;
        self.len = 0;
    }

    fn resize_capacity(&mut self, capacity: usize) {
        if capacity == 0 {
            self.data.clear();
            self.read = 0;
            self.len = 0;
            return;
        }

        if capacity == self.data.len() {
            return;
        }

        if capacity < self.len {
            self.consume(self.len - capacity);
        }
        self.grow_to(capacity);
    }

    fn grow_to(&mut self, new_capacity: usize) {
        let mut new_data = vec![0.0; new_capacity];
        let mut idx = self.read;
        let capacity = self.data.len().max(1);
        for i in 0..self.len {
            new_data[i] = self.data[idx];
            idx += 1;
            if idx == capacity {
                idx = 0;
            }
        }
        self.data = new_data;
        self.read = 0;
    }
}

pub struct SpectrogramProcessor {
    config: SpectrogramConfig,
    planner: FftPlanner<f32>,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    fft_buffer: Vec<Complex32>,
    magnitude_buffer: Vec<f32>,
    bin_gain_sq: Vec<f32>,
    pcm_buffer: SampleBuffer,
    buffer_start_index: u64,
    start_instant: Option<Instant>,
    history: SpectrogramHistory,
    magnitude_pool: Vec<Vec<f32>>,
    pending_reset: bool,
}

impl SpectrogramProcessor {
    pub fn new(config: SpectrogramConfig) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(config.fft_size);
        let window = config.window.coefficients(config.fft_size);
        Self {
            fft_buffer: vec![Complex32::default(); config.fft_size],
            pcm_buffer: SampleBuffer::with_capacity(config.fft_size * 2),
            buffer_start_index: 0,
            start_instant: None,
            config,
            planner,
            fft,
            window,
            magnitude_buffer: vec![0.0; config.fft_size / 2 + 1],
            bin_gain_sq: Self::compute_bin_gain_sq(config.fft_size),
            history: SpectrogramHistory::new(config.history_length),
            magnitude_pool: Vec::new(),
            pending_reset: true,
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
        self.bin_gain_sq = Self::compute_bin_gain_sq(self.config.fft_size);
        let target_capacity = self.config.fft_size.saturating_mul(2).max(1);
        self.pcm_buffer.resize_capacity(target_capacity);
        let evicted = self.history.clear();
        self.recycle_columns(evicted);
        let evicted = self.history.set_capacity(self.config.history_length);
        self.recycle_columns(evicted);
        self.pcm_buffer.clear();
        self.buffer_start_index = 0;
        self.start_instant = None;
        self.pending_reset = true;
    }

    fn ensure_fft_capacity(&mut self) {
        if self.fft_buffer.len() != self.config.fft_size {
            self.rebuild_fft();
        }
    }

    fn process_ready_windows(&mut self) -> Vec<SpectrogramColumn> {
        let mut new_columns = Vec::new();
        let fft_size = self.config.fft_size;
        if fft_size == 0 || self.config.hop_size == 0 {
            return new_columns;
        }

        let hop = self.config.hop_size;
        let bins = fft_size / 2 + 1;
        let scale_sq = {
            let scale = 1.0 / (fft_size as f32);
            scale * scale
        };

        let Some(start_instant) = self.start_instant else {
            return new_columns;
        };

        while self.pcm_buffer.len() >= fft_size {
            self.pcm_buffer.for_each_front(fft_size, |idx, sample| {
                let windowed = sample * self.window[idx];
                self.fft_buffer[idx] = Complex32::new(windowed, 0.0);
            });

            self.fft.process(&mut self.fft_buffer);

            for (bin, value) in self.magnitude_buffer.iter_mut().enumerate().take(bins) {
                let complex = self.fft_buffer[bin];
                let gain = self.bin_gain_sq[bin];
                let power = complex.norm_sqr() * scale_sq * gain;
                let magnitude_db = 10.0f32 * power.max(1.0e-18).log10();
                *value = magnitude_db.max(-120.0f32);
            }

            let column_start_index = self.buffer_start_index;
            let center_index = column_start_index + (fft_size / 2) as u64;
            let timestamp =
                start_instant + duration_from_samples(center_index, self.config.sample_rate);

            let mut storage = self.acquire_magnitude_storage(bins);
            storage.copy_from_slice(&self.magnitude_buffer[..bins]);
            let magnitudes = Arc::new(storage);
            let column = SpectrogramColumn {
                timestamp,
                magnitudes_db: magnitudes,
            };
            if let Some(evicted) = self.history.push(column.clone()) {
                self.recycle_column(evicted);
            }
            new_columns.push(column);

            self.pcm_buffer.consume(hop);
            self.buffer_start_index += hop as u64;
        }

        new_columns
    }
}

impl AudioProcessor for SpectrogramProcessor {
    type Output = SpectrogramUpdate;

    fn process_block(&mut self, block: &AudioBlock<'_>) -> ProcessorUpdate<Self::Output> {
        if block.frame_count() == 0 || block.channels == 0 {
            return ProcessorUpdate::None;
        }

        if self.config.sample_rate <= 0.0 {
            self.config.sample_rate = block.sample_rate;
        } else if (self.config.sample_rate - block.sample_rate).abs() > f32::EPSILON {
            self.config.sample_rate = block.sample_rate;
            self.rebuild_fft();
        }

        if self.start_instant.is_none() {
            self.start_instant = Some(block.timestamp);
        }

        self.ensure_fft_capacity();

        let channels = block.channels;
        match channels {
            1 => {
                for sample in block.samples.iter().copied() {
                    self.pcm_buffer.push(sample);
                }
            }
            2 => {
                let mut chunks = block.samples.chunks_exact(2);
                for chunk in chunks.by_ref() {
                    let mono = 0.5 * (chunk[0] + chunk[1]);
                    self.pcm_buffer.push(mono);
                }
                if !chunks.remainder().is_empty() {
                    let remainder = chunks.remainder();
                    let sum = remainder.iter().copied().sum::<f32>();
                    let mono = sum * 0.5;
                    self.pcm_buffer.push(mono);
                }
            }
            _ => {
                let inv_channels = 1.0 / channels as f32;
                let mut chunks = block.samples.chunks_exact(channels);
                for frame in chunks.by_ref() {
                    let sum = frame.iter().copied().sum::<f32>();
                    self.pcm_buffer.push(sum * inv_channels);
                }
                let remainder = chunks.remainder();
                if !remainder.is_empty() {
                    let sum = remainder.iter().copied().sum::<f32>();
                    self.pcm_buffer.push(sum * inv_channels);
                }
            }
        }

        let new_columns = self.process_ready_windows();
        if new_columns.is_empty() && !self.pending_reset {
            ProcessorUpdate::None
        } else {
            let reset = self.pending_reset;
            self.pending_reset = false;
            ProcessorUpdate::Snapshot(SpectrogramUpdate {
                fft_size: self.config.fft_size,
                hop_size: self.config.hop_size,
                sample_rate: self.config.sample_rate,
                history_length: self.config.history_length,
                reset,
                new_columns,
            })
        }
    }

    fn reset(&mut self) {
        let evicted = self.history.clear();
        self.recycle_columns(evicted);
        let target_capacity = self.config.fft_size.saturating_mul(2).max(1);
        self.pcm_buffer.resize_capacity(target_capacity);
        self.pcm_buffer.clear();
        self.buffer_start_index = 0;
        self.start_instant = None;
        self.pending_reset = true;
    }
}

impl SpectrogramProcessor {
    fn acquire_magnitude_storage(&mut self, bins: usize) -> Vec<f32> {
        if let Some(mut buffer) = self.magnitude_pool.pop() {
            buffer.resize(bins, 0.0);
            buffer
        } else {
            vec![0.0; bins]
        }
    }

    fn compute_bin_gain_sq(fft_size: usize) -> Vec<f32> {
        let bins = fft_size / 2 + 1;
        if bins == 0 {
            return Vec::new();
        }

        let mut gains = vec![4.0; bins];
        gains[0] = 1.0;
        if bins > 1 {
            gains[bins - 1] = 1.0;
        }
        gains
    }

    fn recycle_column(&mut self, column: SpectrogramColumn) {
        if let Ok(buffer) = Arc::try_unwrap(column.magnitudes_db) {
            self.magnitude_pool.push(buffer);
        }
    }

    fn recycle_columns<I>(&mut self, columns: I)
    where
        I: IntoIterator<Item = SpectrogramColumn>,
    {
        for column in columns {
            self.recycle_column(column);
        }
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
        let update = match result {
            ProcessorUpdate::Snapshot(update) => update,
            ProcessorUpdate::None => panic!("expected snapshot"),
        };

        assert!(!update.new_columns.is_empty());
        let last = update.new_columns.last().unwrap();
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
        assert!(processor.history.len() <= config.history_length);
    }
}
