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
    fn clamped(mut self) -> Self {
        self.sample_rate = self.sample_rate.max(1.0);
        self.scroll_speed = self.scroll_speed.clamp(MIN_SCROLL_SPEED, MAX_SCROLL_SPEED);
        self.max_columns = self
            .max_columns
            .clamp(MIN_COLUMN_CAPACITY, MAX_COLUMN_CAPACITY);
        self
    }
    fn samples_per_column(&self) -> usize {
        (self.sample_rate / self.scroll_speed.max(MIN_SCROLL_SPEED)).round() as usize
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

#[derive(Debug, Clone)]
struct Bucket {
    min: f32,
    max: f32,
    samples: Vec<f32>,
}

impl Bucket {
    fn new(cap: usize) -> Self {
        Self {
            min: f32::MAX,
            max: f32::MIN,
            samples: Vec::with_capacity(cap),
        }
    }
    fn push(&mut self, s: f32) {
        self.min = self.min.min(s);
        self.max = self.max.max(s);
        self.samples.push(s);
    }
    fn clear(&mut self) {
        self.min = f32::MAX;
        self.max = f32::MIN;
        self.samples.clear();
    }
    fn extrema(&self) -> (f32, f32) {
        if self.samples.is_empty() {
            (0.0, 0.0)
        } else {
            (
                if self.min == f32::MAX { 0.0 } else { self.min },
                if self.max == f32::MIN { 0.0 } else { self.max },
            )
        }
    }
}

#[derive(Clone)]
struct FftContext {
    fft: Arc<dyn RealToComplex<f32>>,
    size: usize,
    scratch: Vec<Complex32>,
    input: Vec<f32>,
    output: Vec<Complex32>,
    low_bin: usize,
    high_bin: usize,
}

impl std::fmt::Debug for FftContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FftContext")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl FftContext {
    fn new(size: usize, sr: f32) -> Self {
        let fft = RealFftPlanner::new().plan_fft_forward(size);
        let (low, high) = Self::bins(size, sr);
        Self {
            scratch: vec![Complex32::default(); fft.get_scratch_len()],
            input: vec![0.0; size],
            output: vec![Complex32::default(); size / 2 + 1],
            low_bin: low,
            high_bin: high,
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
            self.input.resize(size, 0.0);
            self.output.resize(size / 2 + 1, Complex32::default());
        }
        (self.low_bin, self.high_bin) = Self::bins(size, sr);
    }

    fn bins(size: usize, sr: f32) -> (usize, usize) {
        let bw = sr / size as f32;
        let max = size / 2;
        (
            (LOW_CROSSOVER / bw).round().min(max as f32) as usize,
            (HIGH_CROSSOVER / bw).round().min(max as f32) as usize,
        )
    }

    fn dominant_band(&mut self, samples: &[f32]) -> f32 {
        let n = samples.len();
        if n < 2 {
            return 0.5;
        }
        self.input.fill(0.0);
        let len = n.min(self.size);
        let scale = std::f32::consts::PI / len as f32;
        for (i, &s) in samples.iter().take(len).enumerate() {
            self.input[i] = s * 0.5 * (1.0 - (2.0 * scale * i as f32).cos());
        }
        self.output.fill(Complex32::default());
        if self
            .fft
            .process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch)
            .is_err()
        {
            return 0.5;
        }
        let (low, mid, high) = self.output.iter().enumerate().fold(
            (0.0_f32, 0.0_f32, 0.0_f32),
            |(l, m, h), (i, c)| {
                let e = c.norm_sqr();
                if i <= self.low_bin {
                    (l + e, m, h)
                } else if i < self.high_bin {
                    (l, m + e, h)
                } else {
                    (l, m, h + e)
                }
            },
        );
        if low >= mid && low >= high {
            0.0
        } else if high > mid {
            1.0
        } else {
            0.5
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
    cents: Vec<f32>,
    count: usize,
    head: usize,
    written: u64,
    buckets: Vec<Bucket>,
    fft: FftContext,
    dirty: bool,
}

impl WaveformProcessor {
    pub fn new(config: WaveformConfig) -> Self {
        let cfg = config.clamped();
        let spc = cfg.samples_per_column();
        let fft_size = spc
            .next_power_of_two()
            .clamp(*FFT_SIZE_RANGE.start(), *FFT_SIZE_RANGE.end());
        let mut p = Self {
            cfg,
            snap: WaveformSnapshot::default(),
            ch: 2,
            spc,
            mins: Vec::new(),
            maxs: Vec::new(),
            cents: Vec::new(),
            count: 0,
            head: 0,
            written: 0,
            buckets: Vec::new(),
            fft: FftContext::new(fft_size, cfg.sample_rate),
            dirty: false,
        };
        p.rebuild();
        p
    }

    pub fn config(&self) -> WaveformConfig {
        self.cfg
    }

    fn rebuild(&mut self) {
        self.spc = self.cfg.samples_per_column();
        self.fft.reconfigure(
            self.spc
                .next_power_of_two()
                .clamp(*FFT_SIZE_RANGE.start(), *FFT_SIZE_RANGE.end()),
            self.cfg.sample_rate,
        );
        let cap = self.cfg.max_columns.max(1) * self.ch.max(1);
        self.mins = vec![0.0; cap];
        self.maxs = vec![0.0; cap];
        self.cents = vec![0.0; cap];
        self.count = 0;
        self.head = 0;
        self.written = 0;
        self.buckets = (0..self.ch).map(|_| Bucket::new(self.spc)).collect();
    }

    fn flush(&mut self) {
        let cap = self.cfg.max_columns.max(1);
        for (c, b) in self.buckets.iter().enumerate() {
            if b.samples.is_empty() {
                continue;
            }
            let (mn, mx) = b.extrema();
            let off = c * cap + self.head;
            self.mins[off] = mn;
            self.maxs[off] = mx;
            self.cents[off] = self.fft.dominant_band(&b.samples);
        }
        self.head = (self.head + 1) % cap;
        self.count = self.count.saturating_add(1).min(cap);
        self.written = self.written.saturating_add(1);
        self.dirty = true;
        self.buckets.iter_mut().for_each(Bucket::clear);
    }

    fn ingest(&mut self, samples: &[f32], ch: usize) {
        if samples.is_empty() || ch == 0 {
            return;
        }
        for frame in samples.chunks_exact(ch) {
            for (c, &s) in frame.iter().enumerate().take(ch) {
                self.buckets[c].push(s);
            }
            if self.buckets[0].samples.len() >= self.spc {
                self.flush();
            }
        }
    }

    fn sync_snapshot(&mut self) {
        let (ch, cap, cols) = (
            self.ch.max(1),
            self.cfg.max_columns.max(1),
            self.count.min(self.cfg.max_columns.max(1)),
        );
        let sz = cols * ch;
        self.snap.min_values.resize(sz, 0.0);
        self.snap.max_values.resize(sz, 0.0);
        self.snap.frequency_normalized.resize(sz, 0.0);
        if cols == 0 {
            self.snap.channels = ch;
            self.snap.columns = 0;
            self.snap.column_spacing_seconds = 1.0 / self.cfg.scroll_speed.max(MIN_SCROLL_SPEED);
            return;
        }
        let start = if self.count < cap { 0 } else { self.head };
        for c in 0..ch {
            for i in 0..cols {
                let src = c * cap + (start + i) % cap;
                let dst = c * cols + i;
                self.snap.min_values[dst] = self.mins[src];
                self.snap.max_values[dst] = self.maxs[src];
                self.snap.frequency_normalized[dst] = self.cents[src];
            }
        }
        self.snap.channels = ch;
        self.snap.columns = cols;
        self.snap.column_spacing_seconds = 1.0 / self.cfg.scroll_speed.max(MIN_SCROLL_SPEED);
        self.dirty = false;
    }

    fn progress(&self) -> f32 {
        self.buckets.first().map_or(0.0, |b| {
            (b.samples.len() as f32 / self.spc.max(1) as f32).clamp(0.0, 1.0)
        })
    }

    fn sync_preview(&mut self) {
        let ch = self.ch.max(1);
        self.snap.preview.progress = self.progress();
        if self.buckets.first().is_none_or(|b| b.samples.is_empty()) {
            self.snap.preview.min_values.clear();
            self.snap.preview.max_values.clear();
            return;
        }
        self.snap.preview.min_values.resize(ch, 0.0);
        self.snap.preview.max_values.resize(ch, 0.0);
        for (c, b) in self.buckets.iter().enumerate().take(ch) {
            let (mn, mx) = b.extrema();
            self.snap.preview.min_values[c] = mn;
            self.snap.preview.max_values[c] = mx;
        }
    }

    fn migrate(&mut self, old_cap: usize, new_cap: usize) {
        if old_cap == new_cap {
            return;
        }
        let (ch, keep) = (self.ch.max(1), self.count.min(old_cap).min(new_cap));
        let mut nmins = vec![0.0; ch * new_cap];
        let mut nmaxs = vec![0.0; ch * new_cap];
        let mut ncents = vec![0.0; ch * new_cap];
        if keep > 0 {
            let wrap = self.count >= old_cap;
            let st = if wrap {
                (self.head + old_cap - keep) % old_cap
            } else {
                self.count.saturating_sub(keep)
            };
            for c in 0..ch {
                for i in 0..keep {
                    let src = if wrap { (st + i) % old_cap } else { st + i };
                    nmins[c * new_cap + i] = self.mins[c * old_cap + src];
                    nmaxs[c * new_cap + i] = self.maxs[c * old_cap + src];
                    ncents[c * new_cap + i] = self.cents[c * old_cap + src];
                }
            }
        }
        self.mins = nmins;
        self.maxs = nmaxs;
        self.cents = ncents;
        self.count = keep;
        self.head = if keep >= new_cap { 0 } else { keep };
        self.dirty = true;
    }
}

impl AudioProcessor for WaveformProcessor {
    type Output = WaveformSnapshot;
    fn process_block(&mut self, block: &AudioBlock<'_>) -> ProcessorUpdate<Self::Output> {
        if block.frame_count() == 0 {
            return ProcessorUpdate::None;
        }
        let ch = block.channels.max(1);
        if ch != self.ch {
            self.ch = ch;
            self.rebuild();
        }
        let sr = block.sample_rate.max(1.0);
        if (self.cfg.sample_rate - sr).abs() > f32::EPSILON {
            self.cfg.sample_rate = sr;
            self.rebuild();
        }
        self.ingest(block.samples, ch);
        if self.dirty {
            self.sync_snapshot();
        }
        self.sync_preview();
        self.snap.scroll_position = self.written as f32 + self.progress();
        ProcessorUpdate::Snapshot(self.snap.clone())
    }
    fn reset(&mut self) {
        self.snap = WaveformSnapshot::default();
        self.count = 0;
        self.head = 0;
        self.written = 0;
        self.mins.clear();
        self.maxs.clear();
        self.cents.clear();
        self.buckets.clear();
        self.dirty = false;
        self.rebuild();
    }
}

impl Reconfigurable<WaveformConfig> for WaveformProcessor {
    fn update_config(&mut self, config: WaveformConfig) {
        let c = config.clamped();
        let old_cap = self.cfg.max_columns;
        let rebuild = (self.cfg.sample_rate - c.sample_rate).abs() > f32::EPSILON
            || (self.cfg.scroll_speed - c.scroll_speed).abs() > f32::EPSILON;
        self.cfg = c;
        if rebuild {
            self.rebuild();
        } else if old_cap != c.max_columns {
            self.migrate(old_cap, c.max_columns);
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
        let spc = p.cfg.samples_per_column();
        let samples: Vec<f32> = (0..spc)
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
                (band - expected).abs() < f32::EPSILON,
                "{freq:.0} Hz: expected {expected:.1}, got {band:.1}"
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
        let spc = p.cfg.samples_per_column();
        for batch in 0..MIN_COLUMN_CAPACITY + 10 {
            p.process_block(&block(
                &vec![((batch + 1) as f32 * 0.001).min(1.0); spc],
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
