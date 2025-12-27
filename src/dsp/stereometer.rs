//! Stereometer (vectorscope & correlation meter) DSP.

use super::{AudioBlock, AudioProcessor, ProcessorUpdate, Reconfigurable};
use crate::util::audio::DEFAULT_SAMPLE_RATE;
use std::collections::VecDeque;

const LOW_MID_HZ: f32 = 250.0;
const MID_HIGH_HZ: f32 = 4000.0;

#[derive(Debug, Clone, Copy)]
pub struct StereometerConfig {
    pub sample_rate: f32,
    pub segment_duration: f32,
    pub target_sample_count: usize,
    pub correlation_window: f32,
}

impl Default for StereometerConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            segment_duration: 0.02,
            target_sample_count: 1_024,
            correlation_window: 0.3,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BandCorrelation {
    pub low: f32,
    pub mid: f32,
    pub high: f32,
}

#[derive(Debug, Clone, Default)]
pub struct StereometerSnapshot {
    pub xy_points: Vec<(f32, f32)>,
    pub correlation: f32,
    pub band_correlation: BandCorrelation,
}

/// Linkwitz-Riley 4th-order crossover (two cascaded 2nd-order Butterworth).
#[derive(Debug, Clone, Copy, Default)]
struct LR4 {
    // Coefficients: b[0..3], a[0..2] for each of 2 stages
    b: [[f32; 3]; 2],
    a: [[f32; 2]; 2],
    // State: x[n-1], x[n-2], y[n-1], y[n-2] for each stage
    state: [[f32; 4]; 2],
}

impl LR4 {
    fn new(sr: f32, freq: f32, highpass: bool) -> Self {
        let w = std::f32::consts::TAU * freq / sr;
        let (s, c) = w.sin_cos();
        let alpha = s * std::f32::consts::FRAC_1_SQRT_2;
        let a0_inv = 1.0 / (1.0 + alpha);
        let k = if highpass { 1.0 + c } else { 1.0 - c };
        let (b0, b1) = (k * 0.5 * a0_inv, if highpass { -k } else { k } * a0_inv);
        let coef = ([b0, b1, b0], [-2.0 * c * a0_inv, (1.0 - alpha) * a0_inv]);
        Self {
            b: [coef.0; 2],
            a: [coef.1; 2],
            state: [[0.0; 4]; 2],
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let mut v = x;
        for i in 0..2 {
            let [b0, b1, b2] = self.b[i];
            let [a1, a2] = self.a[i];
            let [x1, x2, y1, y2] = self.state[i];
            let y = b0 * v + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            self.state[i] = [v, x1, y, y1];
            v = y;
        }
        v
    }
}

/// EMA-based stereo correlation with continuous update.
#[derive(Debug, Clone, Copy, Default)]
struct Correlator {
    lr: f64,
    l2: f64,
    r2: f64,
    alpha: f64,
}

impl Correlator {
    #[inline]
    fn update(&mut self, l: f32, r: f32) {
        let (l, r) = (l as f64, r as f64);
        self.lr += self.alpha * (l * r - self.lr);
        self.l2 += self.alpha * (l * l - self.l2);
        self.r2 += self.alpha * (r * r - self.r2);
    }

    #[inline]
    fn value(&self) -> f32 {
        let denom = (self.l2 * self.r2).sqrt();
        if denom < 1e-12 {
            0.0
        } else {
            (self.lr / denom).clamp(-1.0, 1.0) as f32
        }
    }
}

#[derive(Debug, Clone)]
pub struct StereometerProcessor {
    config: StereometerConfig,
    snapshot: StereometerSnapshot,
    history: VecDeque<f32>,
    history_ch: usize,
    // Crossovers: [L low/mid, R low/mid, L mid/high, R mid/high]
    xover: [LR4; 4],
    // Correlators: [full, low, mid, high]
    corr: [Correlator; 4],
}

impl StereometerProcessor {
    pub fn new(config: StereometerConfig) -> Self {
        let alpha = ema_alpha(config.sample_rate, config.correlation_window);
        Self {
            config,
            snapshot: StereometerSnapshot::default(),
            history: VecDeque::new(),
            history_ch: 0,
            xover: Self::build_xover(config.sample_rate),
            corr: [Correlator {
                alpha,
                ..Default::default()
            }; 4],
        }
    }

    fn build_xover(sr: f32) -> [LR4; 4] {
        [
            LR4::new(sr, LOW_MID_HZ, false),  // L lowpass
            LR4::new(sr, LOW_MID_HZ, false),  // R lowpass
            LR4::new(sr, MID_HIGH_HZ, false), // L mid lowpass
            LR4::new(sr, MID_HIGH_HZ, false), // R mid lowpass
        ]
    }

    pub fn config(&self) -> StereometerConfig {
        self.config
    }
    pub fn snapshot(&self) -> &StereometerSnapshot {
        &self.snapshot
    }
}

fn ema_alpha(sr: f32, window: f32) -> f64 {
    1.0 - (-1.0 / (sr as f64 * window as f64).max(1.0)).exp()
}

impl AudioProcessor for StereometerProcessor {
    type Output = StereometerSnapshot;

    fn process_block(&mut self, block: &AudioBlock<'_>) -> ProcessorUpdate<Self::Output> {
        let ch = block.channels.max(1);
        if block.frame_count() == 0 || ch < 2 {
            return ProcessorUpdate::None;
        }
        if self.history_ch != ch {
            self.history.clear();
            self.history_ch = ch;
        }

        // Process audio through crossovers and correlators
        for frame in block.samples.chunks_exact(ch) {
            let (l, r) = (frame[0], frame[1]);
            self.corr[0].update(l, r);

            // 3-band split: low < 250Hz, mid 250-4000Hz, high > 4000Hz
            let (ll, lr) = (self.xover[0].process(l), self.xover[1].process(r));
            let (ml, mr) = (self.xover[2].process(l - ll), self.xover[3].process(r - lr));
            let (hl, hr) = (l - ll - ml, r - lr - mr);

            self.corr[1].update(ll, lr);
            self.corr[2].update(ml, mr);
            self.corr[3].update(hl, hr);
        }

        // Manage history buffer for XY display
        let frames = (self.config.sample_rate * self.config.segment_duration)
            .round()
            .max(1.0) as usize;
        let capacity = frames * ch;

        if block.samples.len() >= capacity {
            self.history.clear();
            self.history
                .extend(&block.samples[block.samples.len() - capacity..]);
        } else {
            let total = self.history.len() + block.samples.len();
            if total > capacity {
                let drain = (total - capacity).div_ceil(ch) * ch;
                self.history.drain(..drain.min(self.history.len()));
            }
            self.history.extend(block.samples);
        }

        if self.history.len() < capacity {
            return ProcessorUpdate::None;
        }

        // Downsample to target point count
        let data = self.history.make_contiguous();
        let target = self.config.target_sample_count.clamp(1, frames);
        self.snapshot.xy_points.clear();
        self.snapshot.xy_points.reserve(target);
        for i in 0..target {
            let idx = (i * frames / target) * ch;
            self.snapshot.xy_points.push((data[idx], data[idx + 1]));
        }

        self.snapshot.correlation = self.corr[0].value();
        self.snapshot.band_correlation = BandCorrelation {
            low: self.corr[1].value(),
            mid: self.corr[2].value(),
            high: self.corr[3].value(),
        };

        ProcessorUpdate::Snapshot(self.snapshot.clone())
    }

    fn reset(&mut self) {
        let alpha = ema_alpha(self.config.sample_rate, self.config.correlation_window);
        self.snapshot = StereometerSnapshot::default();
        self.history.clear();
        self.history_ch = 0;
        self.xover = Self::build_xover(self.config.sample_rate);
        self.corr = [Correlator {
            alpha,
            ..Default::default()
        }; 4];
    }
}

impl Reconfigurable<StereometerConfig> for StereometerProcessor {
    fn update_config(&mut self, config: StereometerConfig) {
        let sr_changed = (self.config.sample_rate - config.sample_rate).abs() > f32::EPSILON;
        let win_changed =
            (self.config.correlation_window - config.correlation_window).abs() > f32::EPSILON;
        self.config = config;

        if sr_changed {
            self.reset();
        } else if win_changed {
            let alpha = ema_alpha(config.sample_rate, config.correlation_window);
            self.corr.iter_mut().for_each(|c| c.alpha = alpha);
        }
    }
}
