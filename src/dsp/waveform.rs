//! Scrolling waveform with 3-band frequency coloring (low/mid/high at 200Hz/2kHz crossovers).

use super::{AudioBlock, AudioProcessor, ProcessorUpdate, Reconfigurable};
use crate::util::audio::DEFAULT_SAMPLE_RATE;
use realfft::{RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex32;
use std::sync::Arc;

pub const MIN_SCROLL_SPEED: f32 = 10.0;
pub const MAX_SCROLL_SPEED: f32 = 1000.0;
pub const MIN_COLUMN_CAPACITY: usize = 512;
pub const MAX_COLUMN_CAPACITY: usize = 16_384;
pub const DEFAULT_COLUMN_CAPACITY: usize = 4_096;

const LOW_CROSSOVER: f32 = 200.0;
const HIGH_CROSSOVER: f32 = 2000.0;
const FFT_SIZE_RANGE: std::ops::RangeInclusive<usize> = 512..=4096;

#[derive(Debug, Clone, Copy)]
pub struct WaveformConfig {
    pub sample_rate: f32,
    pub scroll_speed: f32,
    pub max_columns: usize,
}

impl Default for WaveformConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            scroll_speed: 80.0,
            max_columns: DEFAULT_COLUMN_CAPACITY,
        }
    }
}

impl WaveformConfig {
    fn normalized(mut self) -> Self {
        self.sample_rate = self.sample_rate.max(1.0);
        self.scroll_speed = self.scroll_speed.clamp(MIN_SCROLL_SPEED, MAX_SCROLL_SPEED);
        self.max_columns = self
            .max_columns
            .clamp(MIN_COLUMN_CAPACITY, MAX_COLUMN_CAPACITY);
        self
    }
    fn samples_per_column(&self) -> usize {
        (self.sample_rate / self.scroll_speed).round() as usize
    }
    fn fft_size(&self) -> usize {
        self.samples_per_column()
            .next_power_of_two()
            .clamp(*FFT_SIZE_RANGE.start(), *FFT_SIZE_RANGE.end())
    }
}

#[derive(Debug, Clone, Default)]
pub struct WaveformPreview {
    pub progress: f32,
    pub min_values: Vec<f32>,
    pub max_values: Vec<f32>,
}

#[derive(Debug, Clone, Default)]
pub struct WaveformSnapshot {
    pub channels: usize,
    pub columns: usize,
    pub min_values: Vec<f32>,
    pub max_values: Vec<f32>,
    pub frequency_normalized: Vec<f32>,
    pub column_spacing_seconds: f32,
    pub scroll_position: f32,
    pub preview: WaveformPreview,
}

/// Converts sentinel extrema values to zero for display.
#[inline]
fn clamp_extrema(min: f32, max: f32) -> (f32, f32) {
    (
        if min == f32::MAX { 0.0 } else { min },
        if max == f32::MIN { 0.0 } else { max },
    )
}

#[derive(Clone)]
struct BandAnalyzer {
    fft: Arc<dyn RealToComplex<f32>>,
    size: usize,
    buf: Vec<f32>,
    out: Vec<Complex32>,
    scratch: Vec<Complex32>,
    low: usize,
    high: usize,
}

impl std::fmt::Debug for BandAnalyzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BandAnalyzer")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl BandAnalyzer {
    fn new(size: usize, sr: f32) -> Self {
        let fft = RealFftPlanner::new().plan_fft_forward(size);
        let (low, high) = Self::crossover_bins(size, sr);
        Self {
            scratch: vec![Complex32::default(); fft.get_scratch_len()],
            buf: vec![0.0; size],
            out: vec![Complex32::default(); size / 2 + 1],
            low,
            high,
            size,
            fft,
        }
    }

    fn reconfigure(&mut self, size: usize, sr: f32) {
        if size != self.size {
            self.fft = RealFftPlanner::new().plan_fft_forward(size);
            self.size = size;
            self.scratch
                .resize(self.fft.get_scratch_len(), Complex32::default());
            self.buf.resize(size, 0.0);
            self.out.resize(size / 2 + 1, Complex32::default());
        }
        (self.low, self.high) = Self::crossover_bins(size, sr);
    }

    fn crossover_bins(size: usize, sr: f32) -> (usize, usize) {
        let bw = sr / size as f32;
        let max = size / 2;
        (
            (LOW_CROSSOVER / bw).round().min(max as f32) as usize,
            (HIGH_CROSSOVER / bw).round().min(max as f32) as usize,
        )
    }

    fn analyze(&mut self, samples: &[f32]) -> f32 {
        if samples.len() < 2 {
            return 0.5;
        }
        self.buf.fill(0.0);
        let len = samples.len().min(self.size);
        let k = std::f32::consts::PI / len as f32;
        for (i, &s) in samples.iter().take(len).enumerate() {
            self.buf[i] = s * 0.5 * (1.0 - (2.0 * k * i as f32).cos());
        }
        self.out.fill(Complex32::default());
        if self
            .fft
            .process_with_scratch(&mut self.buf, &mut self.out, &mut self.scratch)
            .is_err()
        {
            return 0.5;
        }
        let (l, m, h) =
            self.out
                .iter()
                .enumerate()
                .fold((0.0f32, 0.0f32, 0.0f32), |(l, m, h), (i, c)| {
                    let e = c.norm_sqr();
                    if i <= self.low {
                        (l + e, m, h)
                    } else if i < self.high {
                        (l, m + e, h)
                    } else {
                        (l, m, h + e)
                    }
                });
        let t = l + m + h;
        if t <= f32::EPSILON {
            0.5
        } else {
            (m * 0.5 + h) / t
        }
    }
}

#[derive(Debug, Clone)]
pub struct WaveformProcessor {
    cfg: WaveformConfig,
    snap: WaveformSnapshot,
    ch: usize,
    spc: usize,
    mins: Vec<f32>,
    maxs: Vec<f32>,
    freqs: Vec<f32>,
    head: usize,
    count: usize,
    written: u64,
    acc: Vec<Vec<f32>>,
    acc_min: Vec<f32>,
    acc_max: Vec<f32>,
    fft: BandAnalyzer,
    dirty: bool,
}

impl WaveformProcessor {
    pub fn new(config: WaveformConfig) -> Self {
        let cfg = config.normalized();
        let mut p = Self {
            spc: cfg.samples_per_column(),
            fft: BandAnalyzer::new(cfg.fft_size(), cfg.sample_rate),
            cfg,
            snap: WaveformSnapshot::default(),
            ch: 2,
            mins: Vec::new(),
            maxs: Vec::new(),
            freqs: Vec::new(),
            head: 0,
            count: 0,
            written: 0,
            acc: Vec::new(),
            acc_min: Vec::new(),
            acc_max: Vec::new(),
            dirty: false,
        };
        p.alloc_buffers();
        p
    }

    pub fn config(&self) -> WaveformConfig {
        self.cfg
    }

    fn alloc_buffers(&mut self) {
        let cap = self.cfg.max_columns * self.ch;
        self.mins.resize(cap, 0.0);
        self.maxs.resize(cap, 0.0);
        self.freqs.resize(cap, 0.0);
        self.acc = (0..self.ch).map(|_| Vec::with_capacity(self.spc)).collect();
        self.acc_min = vec![f32::MAX; self.ch];
        self.acc_max = vec![f32::MIN; self.ch];
    }

    fn rebuild(&mut self) {
        self.spc = self.cfg.samples_per_column();
        self.fft
            .reconfigure(self.cfg.fft_size(), self.cfg.sample_rate);
        self.head = 0;
        self.count = 0;
        self.written = 0;
        self.dirty = false;
        self.alloc_buffers();
    }

    fn flush(&mut self) {
        let cap = self.cfg.max_columns;
        for c in 0..self.ch {
            if self.acc[c].is_empty() {
                continue;
            }
            let (mn, mx) = clamp_extrema(self.acc_min[c], self.acc_max[c]);
            let idx = c * cap + self.head;
            self.mins[idx] = mn;
            self.maxs[idx] = mx;
            self.freqs[idx] = self.fft.analyze(&self.acc[c]);
        }
        self.head = (self.head + 1) % cap;
        self.count = (self.count + 1).min(cap);
        self.written = self.written.saturating_add(1);
        self.dirty = true;
        for c in 0..self.ch {
            self.acc[c].clear();
        }
        self.acc_min.fill(f32::MAX);
        self.acc_max.fill(f32::MIN);
    }

    fn ingest(&mut self, samples: &[f32]) {
        for frame in samples.chunks_exact(self.ch) {
            for (c, &s) in frame.iter().enumerate() {
                self.acc_min[c] = self.acc_min[c].min(s);
                self.acc_max[c] = self.acc_max[c].max(s);
                self.acc[c].push(s);
            }
            if self.acc[0].len() >= self.spc {
                self.flush();
            }
        }
    }

    fn sync_ring_to_snapshot(&mut self) {
        let (ch, cap, cols) = (self.ch, self.cfg.max_columns, self.count);
        let sz = cols * ch;
        self.snap.min_values.resize(sz, 0.0);
        self.snap.max_values.resize(sz, 0.0);
        self.snap.frequency_normalized.resize(sz, 0.0);
        self.snap.channels = ch;
        self.snap.columns = cols;
        if cols > 0 {
            let start = if self.count < cap { 0 } else { self.head };
            for c in 0..ch {
                for i in 0..cols {
                    let (src, dst) = (c * cap + (start + i) % cap, c * cols + i);
                    self.snap.min_values[dst] = self.mins[src];
                    self.snap.max_values[dst] = self.maxs[src];
                    self.snap.frequency_normalized[dst] = self.freqs[src];
                }
            }
        }
        self.snap.column_spacing_seconds = 1.0 / self.cfg.scroll_speed;
        self.dirty = false;
    }

    fn progress(&self) -> f32 {
        self.acc.first().map_or(0.0, |a| {
            (a.len() as f32 / self.spc.max(1) as f32).clamp(0.0, 1.0)
        })
    }

    fn sync_preview(&mut self) {
        self.snap.preview.progress = self.progress();
        if self.acc.first().is_none_or(|a| a.is_empty()) {
            self.snap.preview.min_values.clear();
            self.snap.preview.max_values.clear();
            return;
        }
        self.snap.preview.min_values.resize(self.ch, 0.0);
        self.snap.preview.max_values.resize(self.ch, 0.0);
        for c in 0..self.ch {
            let (mn, mx) = clamp_extrema(self.acc_min[c], self.acc_max[c]);
            self.snap.preview.min_values[c] = mn;
            self.snap.preview.max_values[c] = mx;
        }
    }
}

impl AudioProcessor for WaveformProcessor {
    type Output = WaveformSnapshot;

    fn process_block(&mut self, block: &AudioBlock<'_>) -> ProcessorUpdate<Self::Output> {
        if block.frame_count() == 0 {
            return ProcessorUpdate::None;
        }
        let (ch, sr) = (block.channels.max(1), block.sample_rate.max(1.0));
        if ch != self.ch || (self.cfg.sample_rate - sr).abs() > f32::EPSILON {
            self.ch = ch;
            self.cfg.sample_rate = sr;
            self.rebuild();
        }
        self.ingest(block.samples);
        if self.dirty {
            self.sync_ring_to_snapshot();
        }
        self.sync_preview();
        self.snap.scroll_position = self.written as f32 + self.progress();
        ProcessorUpdate::Snapshot(self.snap.clone())
    }

    fn reset(&mut self) {
        self.snap = WaveformSnapshot::default();
        self.rebuild();
    }
}

impl Reconfigurable<WaveformConfig> for WaveformProcessor {
    fn update_config(&mut self, config: WaveformConfig) {
        let c = config.normalized();
        if self.cfg.sample_rate != c.sample_rate
            || self.cfg.scroll_speed != c.scroll_speed
            || self.cfg.max_columns != c.max_columns
        {
            self.cfg = c;
            self.rebuild();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;
    use std::time::Instant;

    fn block(samples: &[f32], ch: usize, sr: f32) -> AudioBlock<'_> {
        AudioBlock::new(samples, ch, sr, Instant::now())
    }
    fn snap(u: ProcessorUpdate<WaveformSnapshot>) -> WaveformSnapshot {
        match u {
            ProcessorUpdate::Snapshot(s) => s,
            _ => panic!("expected snapshot"),
        }
    }

    #[test]
    fn downsampling_produces_min_max_pairs() {
        let cfg = WaveformConfig {
            sample_rate: 48_000.0,
            scroll_speed: 120.0,
            ..Default::default()
        };
        let mut p = WaveformProcessor::new(cfg);
        let samples: Vec<f32> = (0..p.spc)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.25 })
            .collect();
        let s = snap(p.process_block(&block(&samples, 1, 48_000.0)));
        assert_eq!(s.columns, 1);
        assert!((s.max_values[0] - 0.5).abs() < 1e-3);
        assert!((s.min_values[0] + 0.25).abs() < 1e-3);
    }

    #[test]
    fn detects_correct_bands_for_frequencies() {
        let cfg = WaveformConfig {
            sample_rate: 48_000.0,
            scroll_speed: 200.0,
            ..Default::default()
        };
        let spc = cfg.samples_per_column();
        for &(freq, expected) in &[(100.0, 0.0), (440.0, 0.5), (1000.0, 0.5), (5000.0, 1.0)] {
            let mut p = WaveformProcessor::new(cfg);
            let samples: Vec<f32> = (0..spc * 4)
                .map(|n| (2.0 * PI * freq * n as f32 / 48_000.0).sin())
                .collect();
            let band = snap(p.process_block(&block(&samples, 1, 48_000.0)))
                .frequency_normalized
                .last()
                .copied()
                .unwrap_or(0.5);
            assert!(
                (band - expected).abs() < 0.05,
                "{freq:.0} Hz: expected ~{expected:.1}, got {band:.3}"
            );
        }
    }

    #[test]
    fn ring_buffer_wraps_correctly() {
        let cfg = WaveformConfig {
            sample_rate: 48_000.0,
            scroll_speed: 200.0,
            max_columns: MIN_COLUMN_CAPACITY,
        };
        let mut p = WaveformProcessor::new(cfg);
        for batch in 0..MIN_COLUMN_CAPACITY + 10 {
            p.process_block(&block(
                &vec![((batch + 1) as f32 * 0.001).min(1.0); p.spc],
                1,
                48_000.0,
            ));
        }
        assert_eq!(
            p.snap.columns, MIN_COLUMN_CAPACITY,
            "ring buffer should cap at max_columns"
        );
    }
}
