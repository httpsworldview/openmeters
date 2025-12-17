//! Spectrogram DSP - Time-frequency analysis with reassignment
//!
//! # References
//! 1. F. Auger and P. Flandrin, "Improving the readability of time-frequency and
//!    time-scale representations by the reassignment method", IEEE Trans. SP,
//!    vol. 43, no. 5, pp. 1068-1089, May 1995.
//!    Note: in our delta calculations the signs are inverted compared to the original
//!    paper, to match the more common convention.
//! 2. K. Kodera, R. Gendrin & C. de Villedary, "Analysis of time-varying signals
//!    with small BT values", IEEE Trans. ASSP, vol. 26, no. 1, pp. 64-76, Feb 1978.
//! 3. T. Oberlin, S. Meignen, V. Perrier, "Second-order synchrosqueezing transform
//!    or invertible reassignment? Towards ideal time-frequency representations",
//!    IEEE Trans. SP, vol. 63, no. 5, pp. 1335-1344, 2015.
//!    Note: we aren't implementing "true" SST, rather a simpler form of frequency
//!    correction based on second derivatives. *Our spectrogram is not invertible*
//!    by design.
//! 4. F. Auger et al., "Time-Frequency Reassignment and Synchrosqueezing: An
//!    Overview", IEEE Signal Processing Magazine, vol. 30, pp. 32-41, Nov 2013.

use super::{AudioBlock, AudioProcessor, ProcessorUpdate, Reconfigurable};
use crate::util::audio::{
    DB_FLOOR, DEFAULT_SAMPLE_RATE, copy_from_deque, db_to_power, hz_to_mel, mel_to_hz, power_to_db,
};
use parking_lot::RwLock;
use realfft::{RealFftPlanner, RealToComplex};
use rustc_hash::FxHashMap;
use rustfft::num_complex::Complex32;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, OnceLock};
use wide::{CmpGe, CmpGt, CmpLe, CmpLt, f32x8};

const MAX_REASSIGNMENT_SAMPLES: usize = 8192;
pub const PLANCK_BESSEL_DEFAULT_EPSILON: f32 = 0.1;
pub const PLANCK_BESSEL_DEFAULT_BETA: f32 = 5.5;
const CONFIDENCE_SNR_RANGE_DB: f32 = 60.0;
const CONFIDENCE_FLOOR: f32 = 0.01;
const CHIRP_SAFETY_MARGIN: f32 = 2.0;

const GAUSSIAN_KERNEL_3X3: [[f32; 3]; 3] = {
    const C: f32 = 1.0;
    const E: f32 = 0.324_652_5;
    const K: f32 = 0.105_399_2;
    const S: f32 = C + 4.0 * E + 4.0 * K;
    [
        [K / S, E / S, K / S],
        [E / S, C / S, E / S],
        [K / S, E / S, K / S],
    ]
};

// Horner polynomial evaluation: poly!(x; a0, a1, a2) = a0 + x*(a1 + x*a2)
macro_rules! poly {
    ($x:expr; $c0:expr $(, $c:expr)*) => {{
        let x = $x;
        poly!(@acc x; $c0 $(, $c)*)
    }};
    (@acc $x:ident; $acc:expr, $c:expr $(, $rest:expr)*) => {
        poly!(@acc $x; $acc + $x * poly!(@acc $x; $c $(, $rest)*))
    };
    (@acc $x:ident; $acc:expr) => { $acc };
}

#[derive(Debug, Clone, Copy)]
pub struct SpectrogramConfig {
    pub sample_rate: f32,
    pub fft_size: usize,
    pub hop_size: usize,
    pub window: WindowKind,
    pub frequency_scale: FrequencyScale,
    pub history_length: usize,
    pub use_reassignment: bool,
    pub reassignment_power_floor_db: f32,
    pub reassignment_low_bin_limit: usize,
    pub zero_padding_factor: usize,
    pub display_bin_count: usize,
    pub display_min_hz: f32,
    pub reassignment_max_correction_hz: f32,
    pub reassignment_max_time_hops: f32,
}

impl Default for SpectrogramConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            fft_size: 4096,
            hop_size: 256,
            window: WindowKind::Blackman,
            frequency_scale: FrequencyScale::default(),
            history_length: 240,
            use_reassignment: true,
            reassignment_power_floor_db: -80.0,
            reassignment_low_bin_limit: 0,
            zero_padding_factor: 4,
            display_bin_count: 1024,
            display_min_hz: 20.0,
            reassignment_max_correction_hz: 0.0,
            reassignment_max_time_hops: 2.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FrequencyScale {
    Linear,
    #[default]
    Logarithmic,
    Mel,
}

impl std::fmt::Display for FrequencyScale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    Rectangular,
    Hann,
    Hamming,
    Blackman,
    BlackmanHarris,
    PlanckBessel { epsilon: f32, beta: f32 },
}

impl PartialEq for WindowKind {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::PlanckBessel {
                    epsilon: e1,
                    beta: b1,
                },
                Self::PlanckBessel {
                    epsilon: e2,
                    beta: b2,
                },
            ) => e1.to_bits() == e2.to_bits() && b1.to_bits() == b2.to_bits(),
            (a, b) => core::mem::discriminant(a) == core::mem::discriminant(b),
        }
    }
}
impl Eq for WindowKind {}

impl Hash for WindowKind {
    fn hash<H: Hasher>(&self, state: &mut H) {
        core::mem::discriminant(self).hash(state);
        if let Self::PlanckBessel { epsilon, beta } = self {
            epsilon.to_bits().hash(state);
            beta.to_bits().hash(state);
        }
    }
}

impl WindowKind {
    pub(crate) fn coefficients(self, len: usize) -> Vec<f32> {
        if len <= 1 {
            return vec![1.0; len];
        }
        match self {
            Self::Rectangular => vec![1.0; len],
            Self::Hann => cosine_window(len, &[0.5, -0.5]),
            Self::Hamming => cosine_window(len, &[0.54, -0.46]),
            Self::Blackman => cosine_window(len, &[0.42, -0.5, 0.08]),
            Self::BlackmanHarris => cosine_window(len, &[0.35875, -0.48829, 0.14128, -0.01168]),
            Self::PlanckBessel { epsilon, beta } => planck_bessel(len, epsilon, beta),
        }
    }
}

fn cosine_window(len: usize, c: &[f32]) -> Vec<f32> {
    let s = core::f32::consts::TAU / (len.saturating_sub(1).max(1) as f32);
    (0..len)
        .map(|n| {
            let phi = n as f32 * s;
            c.iter()
                .enumerate()
                .fold(0.0, |a, (k, &v)| a + v * (phi * k as f32).cos())
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct WindowKey {
    kind: WindowKind,
    len: usize,
}

struct WindowCache(RwLock<FxHashMap<WindowKey, Arc<[f32]>>>);

impl WindowCache {
    fn get(kind: WindowKind, len: usize) -> Arc<[f32]> {
        static INSTANCE: OnceLock<WindowCache> = OnceLock::new();
        let cache = INSTANCE.get_or_init(|| WindowCache(RwLock::new(FxHashMap::default())));
        if len == 0 {
            return Arc::from([]);
        }
        let key = WindowKey { kind, len };
        if let Some(v) = cache.0.read().get(&key) {
            return v.clone();
        }
        cache
            .0
            .write()
            .entry(key)
            .or_insert_with(|| Arc::from(kind.coefficients(len)))
            .clone()
    }
}

// Modified Bessel functions I_0 and I_1 using rational polynomial approximations
fn bessel_i0(x: f64) -> f64 {
    let ax = x.abs();
    if ax < 3.75 {
        let y = (x / 3.75).powi(2);
        poly!(y; 1.0, 3.5156229, 3.0899424, 1.2067492, 0.2659732, 0.0360768, 0.0045813, 0.00032411)
    } else {
        let y = 3.75 / ax;
        poly!(y; 0.39894228, 0.01328592, 0.00225319, -0.00157565, 0.00916281,
              -0.02057706, 0.02635537, -0.01647633, 0.00392377)
            * ax.exp()
            / ax.sqrt()
    }
}

fn bessel_i1(x: f64) -> f64 {
    let ax = x.abs();
    let ans = if ax < 3.75 {
        let y2 = (x / 3.75).powi(2);
        x * poly!(y2; 0.5, 0.87890594, 0.51498869, 0.15084934, 0.02658733, 0.00301532, 0.00032411)
    } else {
        let y = 3.75 / ax;
        poly!(y; 0.39894228, -0.03988024, -0.00362018, 0.00163801, -0.01031555,
              0.02282967, -0.02895312, 0.01787654, -0.00420059)
            * ax.exp()
            / ax.sqrt()
    };
    if x < 0.0 { -ans } else { ans }
}

fn planck_bessel(len: usize, epsilon: f32, beta: f32) -> Vec<f32> {
    let eps = epsilon.clamp(1e-6, 0.5 - 1e-6);
    let beta = beta.max(0.0) as f64;
    let denom = bessel_i0(beta);
    let (span, n_max) = ((len - 1) as f64, (len - 1) as f32);
    let taper = (eps * n_max).min(n_max * 0.5);
    (0..len)
        .map(|i| {
            let p = if taper <= 0.0 {
                1.0
            } else {
                let x = if (i as f32) <= n_max * 0.5 {
                    i as f32
                } else {
                    n_max - i as f32
                };
                if x <= 0.0 {
                    0.0
                } else if x >= taper {
                    1.0
                } else {
                    1.0 / ((taper / x - taper / (taper - x)).exp() + 1.0)
                }
            };
            let k = if beta == 0.0 {
                1.0
            } else {
                let r = 2.0 * i as f64 / span - 1.0;
                (bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / denom) as f32
            };
            p * k
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct ReassignedSample {
    pub frequency_hz: f32,
    pub group_delay_samples: f32,
    pub magnitude_db: f32,
}

struct ReassignmentBuffers {
    d_win: Vec<f32>,
    t_win: Vec<f32>,
    d2_win: Vec<f32>,
    d_buf: Vec<f32>,
    t_buf: Vec<f32>,
    d2_buf: Vec<f32>,
    d_spec: Vec<Complex32>,
    t_spec: Vec<Complex32>,
    d2_spec: Vec<Complex32>,
    cache: Vec<ReassignedSample>,
    floor_lin: f32,
    inv_sigma_t: f32,
    max_chirp: f32,
}

impl ReassignmentBuffers {
    fn new(
        kind: WindowKind,
        win: &[f32],
        fft: &Arc<dyn RealToComplex<f32>>,
        size: usize,
        floor_db: f32,
    ) -> Self {
        let mut s = Self {
            d_win: vec![],
            t_win: vec![],
            d2_win: vec![],
            d_buf: vec![],
            t_buf: vec![],
            d2_buf: vec![],
            d_spec: vec![],
            t_spec: vec![],
            d2_spec: vec![],
            cache: Vec::with_capacity(size / 32),
            floor_lin: 0.0,
            inv_sigma_t: 0.0,
            max_chirp: 0.0,
        };
        s.resize(kind, win, fft, size, floor_db);
        s
    }

    fn resize(
        &mut self,
        kind: WindowKind,
        win: &[f32],
        fft: &Arc<dyn RealToComplex<f32>>,
        size: usize,
        floor_db: f32,
    ) {
        self.d_win = compute_derivative(kind, win);
        self.t_win = compute_time_weighted(win);
        self.d2_win = finite_diff(&self.d_win, true);
        for buf in [&mut self.d_buf, &mut self.t_buf, &mut self.d2_buf] {
            buf.resize(size, 0.0);
        }
        self.d_spec = fft.make_output_vec();
        self.t_spec = fft.make_output_vec();
        self.d2_spec = fft.make_output_vec();
        self.floor_lin = db_to_power(floor_db);
        let sigma_t = compute_sigma_t(win);
        self.inv_sigma_t = 1.0 / sigma_t;
        self.max_chirp = 0.5 / (sigma_t * sigma_t);
    }
}

#[derive(Clone, Copy, Default)]
struct FreqScaleParams {
    min: f32,
    max: f32,
    log_min: f32,
    inv_log: f32,
    mel_min: f32,
    inv_mel: f32,
    inv_lin: f32,
    bins_m1: f32,
}

impl FreqScaleParams {
    fn compute(min: f32, max: f32, bins: usize) -> Self {
        if bins == 0 || max <= min {
            return Self::default();
        }
        Self {
            min,
            max,
            log_min: min.ln(),
            inv_log: 1.0 / (max.ln() - min.ln()).max(1e-9),
            mel_min: hz_to_mel(min),
            inv_mel: 1.0 / (hz_to_mel(max) - hz_to_mel(min)).max(1e-9),
            inv_lin: if max - min > f32::EPSILON {
                1.0 / (max - min)
            } else {
                0.0
            },
            bins_m1: (bins - 1) as f32,
        }
    }

    #[inline(always)]
    fn hz_to_bin_simd(&self, hz: f32x8, scale: FrequencyScale) -> f32x8 {
        let norm = match scale {
            FrequencyScale::Linear => (hz - self.min) * self.inv_lin,
            FrequencyScale::Logarithmic => (hz.ln() - self.log_min) * self.inv_log,
            FrequencyScale::Mel => {
                let mel = f32x8::splat(2595.0)
                    * ((f32x8::splat(1.0) + hz / f32x8::splat(700.0)).ln()
                        * f32x8::splat(core::f32::consts::LOG10_E));
                (mel - self.mel_min) * self.inv_mel
            }
        };
        (f32x8::splat(1.0) - norm) * self.bins_m1
    }
}

struct Reassignment2DGrid {
    display_bins: usize,
    max_hops: usize,
    hop: usize,
    cols: usize,
    center: usize,
    grid: Vec<f32>,
    out: Vec<f32>,
    mags: Vec<f32>,
    scale: FrequencyScale,
    params: FreqScaleParams,
    bin_freqs: Arc<[f32]>,
    min_hz: f32,
    max_hz: f32,
    enabled: bool,
}

impl Reassignment2DGrid {
    fn new(cfg: &SpectrogramConfig) -> Self {
        let mut s = Self {
            display_bins: 0,
            max_hops: 1,
            hop: cfg.hop_size,
            cols: 3,
            center: 1,
            grid: vec![],
            out: vec![],
            mags: vec![],
            scale: cfg.frequency_scale,
            params: Default::default(),
            bin_freqs: Arc::from([]),
            min_hz: 0.0,
            max_hz: 0.0,
            enabled: false,
        };
        s.reconfigure(cfg);
        s
    }

    fn reconfigure(&mut self, cfg: &SpectrogramConfig) {
        let en = cfg.use_reassignment && cfg.display_bin_count > 0 && cfg.sample_rate > 0.0;
        if !en {
            self.enabled = false;
            self.grid.clear();
            return;
        }

        let bins = cfg.display_bin_count.max(2);
        let hops = (cfg.reassignment_max_time_hops.ceil() as usize).max(1);
        let cols = 2 * hops + 1;
        let min = cfg.display_min_hz.max(1.0).min(cfg.sample_rate * 0.5);
        let max = (cfg.sample_rate * 0.5).max(min * 1.001);

        if self.display_bins != bins
            || self.scale != cfg.frequency_scale
            || (self.min_hz - min).abs() > f32::EPSILON
            || (self.max_hz - max).abs() > f32::EPSILON
        {
            self.params = FreqScaleParams::compute(min, max, bins);
            self.bin_freqs = Self::compute_freqs(cfg.frequency_scale, bins, &self.params);
        }
        if self.display_bins != bins || self.cols != cols || self.grid.len() != bins * cols {
            self.grid = vec![0.0; bins * cols];
            self.out = vec![0.0; bins];
            self.mags = vec![DB_FLOOR; bins];
        }
        self.display_bins = bins;
        self.max_hops = hops;
        self.hop = cfg.hop_size.max(1);
        self.cols = cols;
        self.center = hops;
        self.scale = cfg.frequency_scale;
        self.min_hz = min;
        self.max_hz = max;
        self.enabled = true;
    }

    fn compute_freqs(scale: FrequencyScale, bins: usize, p: &FreqScaleParams) -> Arc<[f32]> {
        if bins == 0 {
            return Arc::from([]);
        }
        let mut f: Vec<_> = (0..bins)
            .map(|i| {
                let t = i as f32 / (bins as f32 - 1.0);
                match scale {
                    FrequencyScale::Linear => p.min + (p.max - p.min) * t,
                    FrequencyScale::Logarithmic => (p.log_min + t / p.inv_log).exp(),
                    FrequencyScale::Mel => mel_to_hz(p.mel_min + t / p.inv_mel),
                }
            })
            .collect();
        f.reverse();
        Arc::from(f)
    }

    #[inline(always)]
    fn accumulate_simd(&mut self, freq: f32x8, time: f32x8, pow: f32x8, conf: f32x8, mask: f32x8) {
        let (fs, ts, ps, cs, ms) = (
            freq.to_array(),
            time.to_array(),
            pow.to_array(),
            conf.to_array(),
            mask.to_array(),
        );
        let (w, h) = (self.display_bins as i32, self.cols as i32);
        let inv_hop = 1.0 / self.hop as f32;
        let (center, max_off) = (self.center as f32, self.max_hops as f32);

        for i in 0..8 {
            if ms[i] == 0.0 {
                continue;
            }
            let val = ps[i] * cs[i];
            if val <= 0.0 || !val.is_finite() || !fs[i].is_finite() {
                continue;
            }
            let tc = ((ts[i] * inv_hop).clamp(-max_off, max_off) + center).round() as i32;
            let fc = fs[i].round() as i32;
            self.apply_kernel(fc, tc, w, h, val);
        }
    }

    #[inline(always)]
    fn apply_kernel(&mut self, fc: i32, tc: i32, w: i32, h: i32, val: f32) {
        if fc >= 1 && fc < w - 1 && tc >= 1 && tc < h - 1 {
            let base = (tc as usize) * (w as usize) + (fc as usize);
            for (kx, col) in GAUSSIAN_KERNEL_3X3.iter().enumerate() {
                for (ky, &k) in col.iter().enumerate() {
                    self.grid[base + ky * w as usize + kx - w as usize - 1] += val * k;
                }
            }
        } else if fc >= 0 && fc < w && tc >= 0 && tc < h {
            for (kx, col) in GAUSSIAN_KERNEL_3X3.iter().enumerate() {
                for (ky, &k) in col.iter().enumerate() {
                    let (t, f) = (tc + ky as i32 - 1, fc + kx as i32 - 1);
                    if t >= 0 && t < h && f >= 0 && f < w {
                        self.grid[(t as usize) * (w as usize) + f as usize] += val * k;
                    }
                }
            }
        }
    }

    fn advance(&mut self, e_scale: &[f32], b_norm: &[f32]) {
        self.out.copy_from_slice(&self.grid[..self.display_bins]);
        self.grid.rotate_left(self.display_bins);
        self.grid[(self.cols - 1) * self.display_bins..].fill(0.0);
        for i in 0..self.mags.len().min(self.out.len()) {
            let es = e_scale.get(i).or(e_scale.get(1)).copied().unwrap_or(1.0);
            let bn = b_norm.get(i).or(b_norm.get(1)).copied().unwrap_or(1.0);
            self.mags[i] = power_to_db(if es > f32::EPSILON {
                self.out[i] * bn / es
            } else {
                0.0
            });
        }
    }
}

#[derive(Debug, Clone)]
pub struct SpectrogramColumn {
    pub magnitudes_db: Arc<[f32]>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub reassigned: Option<Arc<[ReassignedSample]>>,
}

#[derive(Debug, Clone)]
pub struct SpectrogramUpdate {
    pub fft_size: usize,
    pub sample_rate: f32,
    pub frequency_scale: FrequencyScale,
    pub history_length: usize,
    pub reset: bool,
    pub display_bins_hz: Option<Arc<[f32]>>,
    pub new_columns: Vec<SpectrogramColumn>,
}

pub struct SpectrogramProcessor {
    cfg: SpectrogramConfig,
    planner: RealFftPlanner<f32>,
    fft: Arc<dyn RealToComplex<f32>>,
    win_size: usize,
    fft_size: usize,
    win: Arc<[f32]>,
    real: Vec<f32>,
    spec: Vec<Complex32>,
    scratch: Vec<Complex32>,
    mags: Vec<f32>,
    reassign: ReassignmentBuffers,
    grid: Reassignment2DGrid,
    bin_norm: Vec<f32>,
    energy_norm: Vec<f32>,
    pcm: VecDeque<f32>,
    hist: VecDeque<SpectrogramColumn>,
    hist_cap: usize,
    pool: Vec<Arc<[f32]>>,
    reset: bool,
    out_buf: Vec<SpectrogramColumn>,
}

impl SpectrogramProcessor {
    pub fn new(cfg: SpectrogramConfig) -> Self {
        let dummy_fft = realfft::RealFftPlanner::new().plan_fft_forward(1024);
        let mut s = Self {
            cfg,
            planner: RealFftPlanner::new(),
            fft: dummy_fft.clone(),
            win_size: 0,
            fft_size: 0,
            win: Arc::from([]),
            real: vec![],
            spec: vec![],
            scratch: vec![],
            mags: vec![],
            reassign: ReassignmentBuffers::new(WindowKind::Hann, &[], &dummy_fft, 1024, -80.0),
            grid: Reassignment2DGrid::new(&cfg),
            bin_norm: vec![],
            energy_norm: vec![],
            pcm: VecDeque::new(),
            hist: VecDeque::with_capacity(cfg.history_length),
            hist_cap: cfg.history_length,
            pool: vec![],
            reset: true,
            out_buf: vec![],
        };
        s.rebuild_fft();
        s
    }

    pub fn config(&self) -> SpectrogramConfig {
        self.cfg
    }

    fn rebuild_fft(&mut self) {
        self.win_size = self.cfg.fft_size;
        self.fft_size = self.win_size * self.cfg.zero_padding_factor.max(1);
        self.fft = self.planner.plan_fft_forward(self.fft_size);
        self.win = WindowCache::get(self.cfg.window, self.win_size);
        self.real.resize(self.fft_size, 0.0);
        self.spec = self.fft.make_output_vec();
        self.scratch = self.fft.make_scratch_vec();
        self.mags.resize(self.fft_size / 2 + 1, 0.0);
        self.reassign.resize(
            self.cfg.window,
            &self.win,
            &self.fft,
            self.fft_size,
            self.cfg.reassignment_power_floor_db,
        );
        self.grid.reconfigure(&self.cfg);
        self.bin_norm = crate::util::audio::compute_fft_bin_normalization(&self.win, self.fft_size);
        self.energy_norm = compute_energy_norm(&self.win, self.fft_size);
        let out_bins = if self.grid.enabled {
            self.grid.display_bins
        } else {
            self.fft_size / 2 + 1
        };
        self.pool.retain(|b| b.len() == out_bins);
        self.pcm.truncate(self.win_size * 2);
        self.clear_history();
    }

    fn process_ready_windows(&mut self) -> Vec<SpectrogramColumn> {
        self.out_buf.clear();
        if self.win_size == 0 {
            return vec![];
        }
        let (hop, sr) = (self.cfg.hop_size, self.cfg.sample_rate);
        let re_en = self.cfg.use_reassignment && sr > f32::EPSILON && self.grid.enabled;
        let bin_lim = if self.cfg.reassignment_low_bin_limit == 0 {
            self.fft_size / 2 + 1
        } else {
            self.cfg
                .reassignment_low_bin_limit
                .min(self.fft_size / 2 + 1)
        };

        while self.pcm.len() >= self.win_size {
            copy_from_deque(&mut self.real[..self.win_size], &self.pcm);
            crate::util::audio::remove_dc(&mut self.real[..self.win_size]);

            if re_en {
                for i in 0..self.win_size {
                    let s = self.real[i];
                    self.reassign.d_buf[i] = s * self.reassign.d_win[i];
                    self.reassign.t_buf[i] = s * self.reassign.t_win[i];
                    self.reassign.d2_buf[i] = s * self.reassign.d2_win[i];
                }
                self.reassign.d_buf[self.win_size..].fill(0.0);
                self.reassign.t_buf[self.win_size..].fill(0.0);
                self.reassign.d2_buf[self.win_size..].fill(0.0);
            }

            crate::util::audio::apply_window(&mut self.real[..self.win_size], &self.win);
            self.real[self.win_size..].fill(0.0);
            self.fft
                .process_with_scratch(&mut self.real, &mut self.spec, &mut self.scratch)
                .ok();

            let (mags, reassigned) = if re_en {
                for (buf, spec) in [
                    (&mut self.reassign.d_buf, &mut self.reassign.d_spec),
                    (&mut self.reassign.t_buf, &mut self.reassign.t_spec),
                    (&mut self.reassign.d2_buf, &mut self.reassign.d2_spec),
                ] {
                    self.fft
                        .process_with_scratch(buf, spec, &mut self.scratch)
                        .ok();
                }
                let samples = self.compute_reassigned(sr, bin_lim);
                (
                    Self::fill_arc(self.acquire_mags(self.grid.display_bins), &self.grid.mags),
                    samples,
                )
            } else {
                for i in 0..self.mags.len() {
                    let c = self.spec[i];
                    self.mags[i] = power_to_db((c.re * c.re + c.im * c.im) * self.bin_norm[i]);
                }
                (
                    Self::fill_arc(self.acquire_mags(self.mags.len()), &self.mags),
                    None,
                )
            };

            let col = SpectrogramColumn {
                magnitudes_db: mags.clone(),
                reassigned: reassigned.clone(),
            };
            self.hist_push(col);
            self.out_buf.push(SpectrogramColumn {
                magnitudes_db: mags,
                reassigned,
            });
            self.pcm.drain(..hop.min(self.pcm.len()));
        }
        std::mem::take(&mut self.out_buf)
    }

    fn compute_reassigned(&mut self, sr: f32, limit: usize) -> Option<Arc<[ReassignedSample]>> {
        let bin_hz = sr / self.fft_size as f32;
        let inv_2pi = sr / core::f32::consts::TAU;
        let max_corr = if self.cfg.reassignment_max_correction_hz > 0.0 {
            self.cfg.reassignment_max_correction_hz
        } else {
            bin_hz
        };
        self.reassign.cache.clear();

        let (v_floor, v_bin_hz) = (f32x8::splat(self.reassign.floor_lin), f32x8::splat(bin_hz));
        let (v_inv_2pi, v_max_corr) = (f32x8::splat(inv_2pi), f32x8::splat(max_corr));
        let v_max_chirp = f32x8::splat(self.reassign.max_chirp * CHIRP_SAFETY_MARGIN);
        let v_inv_sigma = f32x8::splat(self.reassign.inv_sigma_t);
        let (v_min_hz, v_max_hz) = (
            f32x8::splat(self.grid.min_hz),
            f32x8::splat(self.grid.max_hz),
        );
        let v_snr_range = f32x8::splat(CONFIDENCE_SNR_RANGE_DB);
        let v_eps = f32x8::splat(f32::MIN_POSITIVE);

        for chunk in 0..limit.div_ceil(8) {
            let off = chunk * 8;
            let k_idx = f32x8::new(std::array::from_fn(|j| (off + j) as f32));
            if k_idx.simd_ge(f32x8::splat(limit as f32)).all() {
                break;
            }

            let (base_re, base_im) = load_complex_simd(&self.spec, off);
            let (d_re, d_im) = load_complex_simd(&self.reassign.d_spec, off);
            let (t_re, t_im) = load_complex_simd(&self.reassign.t_spec, off);
            let (d2_re, d2_im) = load_complex_simd(&self.reassign.d2_spec, off);
            let bn = load_f32_simd(&self.bin_norm, off);
            let es = load_f32_simd(&self.energy_norm, off);

            let pow = base_re * base_re + base_im * base_im;
            let disp_pow = pow * bn;
            let mask = disp_pow.simd_ge(v_floor) & es.simd_gt(f32x8::splat(0.0));
            if mask.none() {
                continue;
            }

            let inv_pow = f32x8::splat(1.0) / pow.max(v_eps);
            let d_omega = -(d_im * base_re - d_re * base_im) * inv_pow;
            let mut f_corr = d_omega * v_inv_2pi;

            // Chirp correction (second-order instantaneous frequency)
            let chirp = -(d2_im * base_re - d2_re * base_im) * inv_pow * v_inv_2pi;
            let t_mag = (t_re * t_re + t_im * t_im).sqrt();
            let weight = (-(t_mag * v_inv_sigma / pow.sqrt().max(v_eps)))
                .exp()
                .min(f32x8::splat(1.0));
            f_corr = chirp
                .abs()
                .simd_lt(v_max_chirp)
                .blend(f_corr + chirp * weight * f32x8::splat(0.5), f_corr);

            let freq = k_idx.mul_add(v_bin_hz, f_corr);
            let final_mask = mask
                & f_corr.abs().simd_le(v_max_corr)
                & freq.simd_ge(v_min_hz)
                & freq.simd_lt(v_max_hz);
            if final_mask.none() {
                continue;
            }

            let d_tau = -(t_re * base_re + t_im * base_im) * inv_pow;
            let snr = ((disp_pow.ln() - v_floor.ln()) * f32x8::splat(4.3429448))
                .max(f32x8::splat(0.0))
                / v_snr_range;
            let coh = f32x8::splat(1.0) - (f_corr.abs() / v_bin_hz).min(f32x8::splat(1.0));
            let conf = (snr.min(f32x8::splat(1.0)) * coh).max(f32x8::splat(CONFIDENCE_FLOOR));

            self.grid.accumulate_simd(
                self.grid.params.hz_to_bin_simd(freq, self.grid.scale),
                d_tau,
                pow * es,
                conf,
                final_mask.blend(f32x8::splat(1.0), f32x8::splat(0.0)),
            );

            if self.reassign.cache.len() < MAX_REASSIGNMENT_SAMPLES {
                let (fa, ga, pa, ma) = (
                    freq.to_array(),
                    d_tau.to_array(),
                    disp_pow.to_array(),
                    final_mask
                        .blend(f32x8::splat(1.0), f32x8::splat(0.0))
                        .to_array(),
                );
                for j in 0..8 {
                    if ma[j] > 0.0 {
                        self.reassign.cache.push(ReassignedSample {
                            frequency_hz: fa[j],
                            group_delay_samples: ga[j],
                            magnitude_db: power_to_db(pa[j]),
                        });
                    }
                }
            }
        }
        self.grid.advance(&self.energy_norm, &self.bin_norm);
        (!self.reassign.cache.is_empty()).then(|| Arc::from(self.reassign.cache.as_slice()))
    }

    fn acquire_mags(&mut self, bins: usize) -> Arc<[f32]> {
        if bins == 0 {
            return Arc::from([]);
        }
        self.pool
            .iter()
            .rposition(|a| a.len() == bins)
            .map(|i| self.pool.swap_remove(i))
            .unwrap_or_else(|| Arc::from(vec![0.0; bins]))
    }

    fn fill_arc(mut arc: Arc<[f32]>, data: &[f32]) -> Arc<[f32]> {
        if arc.len() != data.len() {
            return Arc::from(data);
        }
        if let Some(b) = Arc::get_mut(&mut arc) {
            b.copy_from_slice(data);
            arc
        } else {
            Arc::from(data)
        }
    }

    fn recycle(&mut self, col: SpectrogramColumn) {
        if Arc::strong_count(&col.magnitudes_db) == 1 {
            self.pool.push(col.magnitudes_db);
        }
    }

    fn hist_push(&mut self, col: SpectrogramColumn) {
        if self.hist_cap == 0 {
            return;
        }
        if self.hist.len() == self.hist_cap
            && let Some(ev) = self.hist.pop_front()
        {
            self.recycle(ev);
        }
        self.hist.push_back(col);
    }

    fn clear_history(&mut self) {
        let cols: Vec<_> = self.hist.drain(..).collect();
        for c in cols {
            self.recycle(c);
        }
        self.reset = true;
    }
}

impl AudioProcessor for SpectrogramProcessor {
    type Output = SpectrogramUpdate;

    fn process_block(&mut self, block: &AudioBlock<'_>) -> ProcessorUpdate<Self::Output> {
        if block.frame_count() == 0 || block.channels == 0 {
            return ProcessorUpdate::None;
        }
        if self.cfg.sample_rate <= 0.0
            || (self.cfg.sample_rate - block.sample_rate).abs() > f32::EPSILON
        {
            self.cfg.sample_rate = block.sample_rate;
            self.rebuild_fft();
        }
        if self.win_size != self.cfg.fft_size {
            self.rebuild_fft();
        }
        crate::util::audio::mixdown_into_deque(&mut self.pcm, block.samples, block.channels);
        let cols = self.process_ready_windows();
        if cols.is_empty() {
            ProcessorUpdate::None
        } else {
            ProcessorUpdate::Snapshot(SpectrogramUpdate {
                fft_size: self.fft_size,
                sample_rate: self.cfg.sample_rate,
                frequency_scale: self.cfg.frequency_scale,
                history_length: self.cfg.history_length,
                reset: std::mem::take(&mut self.reset),
                display_bins_hz: self.grid.bin_freqs.clone().into(),
                new_columns: cols,
            })
        }
    }

    fn reset(&mut self) {
        self.clear_history();
        self.pcm.clear();
        self.grid.grid.fill(0.0);
        self.grid.mags.fill(DB_FLOOR);
    }
}

impl Reconfigurable<SpectrogramConfig> for SpectrogramProcessor {
    fn update_config(&mut self, cfg: SpectrogramConfig) {
        let prev = self.cfg;
        self.cfg = cfg;
        if prev.fft_size != cfg.fft_size
            || prev.zero_padding_factor != cfg.zero_padding_factor
            || prev.window != cfg.window
            || (prev.sample_rate - cfg.sample_rate).abs() > f32::EPSILON
        {
            self.rebuild_fft();
            return;
        }
        if prev.history_length != cfg.history_length {
            self.hist_cap = cfg.history_length;
            while self.hist.len() > self.hist_cap {
                if let Some(c) = self.hist.pop_front() {
                    self.recycle(c);
                }
            }
        }
        if prev.reassignment_power_floor_db != cfg.reassignment_power_floor_db {
            self.reassign.floor_lin = db_to_power(cfg.reassignment_power_floor_db);
        }
        if prev.use_reassignment != cfg.use_reassignment
            || prev.display_bin_count != cfg.display_bin_count
            || (prev.display_min_hz - cfg.display_min_hz).abs() > f32::EPSILON
            || prev.frequency_scale != cfg.frequency_scale
            || (prev.reassignment_max_time_hops - cfg.reassignment_max_time_hops).abs()
                > f32::EPSILON
        {
            self.grid.reconfigure(&self.cfg);
            let bins = if self.grid.enabled {
                self.grid.display_bins
            } else {
                self.fft_size / 2 + 1
            };
            self.pool.retain(|b| b.len() == bins);
            self.clear_history();
        }
    }
}

// Unified finite difference for first and second derivatives
fn finite_diff(d: &[f32], second: bool) -> Vec<f32> {
    let len = d.len();
    if len == 0 {
        return vec![];
    }
    (0..len)
        .map(|i| {
            let (p, c, n) = (
                if i == 0 { d[0] } else { d[i - 1] },
                d[i],
                *d.get(i + 1).unwrap_or(&d[len - 1]),
            );
            if second {
                if len <= 4 || i < 2 || i >= len - 2 {
                    if i == 0 {
                        d.get(2).copied().unwrap_or(d[1]) - 2.0 * d[1] + d[0]
                    } else if i == len - 1 {
                        d[len - 1] - 2.0 * d[len - 2]
                            + d.get(len.saturating_sub(3)).copied().unwrap_or(d[len - 2])
                    } else {
                        n - 2.0 * c + p
                    }
                } else {
                    (-d[i - 2] + 16.0 * d[i - 1] - 30.0 * d[i] + 16.0 * d[i + 1] - d[i + 2]) / 12.0
                }
            } else if len <= 4 || i < 2 || i >= len - 2 {
                0.5 * (n - p)
            } else {
                (d[i - 2] - 8.0 * d[i - 1] + 8.0 * d[i + 1] - d[i + 2]) / 12.0
            }
        })
        .collect()
}

fn compute_derivative(kind: WindowKind, win: &[f32]) -> Vec<f32> {
    if let WindowKind::PlanckBessel { epsilon, beta } = kind {
        let (len, eps) = (win.len(), epsilon.clamp(1e-6, 0.5 - 1e-6));
        let (n_max, beta64) = ((len - 1) as f32, beta.max(0.0) as f64);
        let taper = (eps * n_max).min(n_max * 0.5);
        if taper > 0.0 {
            let denom = bessel_i0(beta64);
            return (0..len)
                .map(|i| {
                    let (pos, sign) = if (i as f32) < n_max * 0.5 {
                        (i as f32, 1.0)
                    } else {
                        (n_max - i as f32, -1.0)
                    };
                    let (k_val, k_der) = if beta64 == 0.0 {
                        (1.0, 0.0)
                    } else {
                        let r = (2.0 * i as f64) / (len - 1) as f64 - 1.0;
                        let ins = (1.0 - r * r).max(0.0).sqrt();
                        let arg = beta64 * ins;
                        (
                            (bessel_i0(arg) / denom) as f32,
                            if ins <= 1e-10 {
                                0.0
                            } else {
                                (bessel_i1(arg) * beta64 * (-2.0 * r / ((len - 1) as f64 * ins))
                                    / denom) as f32
                            },
                        )
                    };
                    let (p_val, p_der) = if beta64.abs() > 1e-6 && k_val.abs() > 1e-10 {
                        (win[i] / k_val, 0.0)
                    } else {
                        let z = taper / pos - taper / (taper - pos);
                        let l = 1.0 / (z.exp() + 1.0);
                        (
                            l,
                            if pos <= 0.0 || pos >= taper {
                                0.0
                            } else {
                                l * (1.0 - l)
                                    * (taper / (pos * pos) + taper / ((taper - pos).powi(2)))
                            },
                        )
                    };
                    (p_der * sign) * k_val + p_val * k_der
                })
                .collect();
        }
    }
    finite_diff(win, false)
}

fn compute_time_weighted(win: &[f32]) -> Vec<f32> {
    let c = (win.len().saturating_sub(1)) as f32 * 0.5;
    win.iter()
        .enumerate()
        .map(|(i, &w)| (i as f32 - c) * w)
        .collect()
}

fn compute_sigma_t(win: &[f32]) -> f32 {
    let c = (win.len().saturating_sub(1)) as f32 * 0.5;
    let (s1, s2) = win
        .iter()
        .enumerate()
        .fold((0.0, 0.0), |(s1, s2), (i, &g)| {
            let (t, g2) = (i as f32 - c, (g * g) as f64);
            (s1 + (t * t) as f64 * g2, s2 + g2)
        });
    if s2 < 1e-10 {
        1.0
    } else {
        (s1 / s2).sqrt().max(1.0) as f32
    }
}

fn compute_energy_norm(win: &[f32], size: usize) -> Vec<f32> {
    let e: f32 = win.iter().map(|c| c * c).sum();
    let (dc, ac) = (1.0 / e, 2.0 / e);
    let mut n = vec![ac; size / 2 + 1];
    let len = n.len();
    if len > 0 {
        n[0] = dc;
        if len > 1 {
            n[len - 1] = dc;
        }
    }
    n
}

#[inline(always)]
fn load_f32_simd(d: &[f32], off: usize) -> f32x8 {
    if d.len() >= off + 8 {
        f32x8::new(d[off..off + 8].try_into().unwrap())
    } else {
        let mut a = [0.0; 8];
        a[..d.len().saturating_sub(off)].copy_from_slice(&d[off..]);
        f32x8::new(a)
    }
}

#[inline(always)]
fn load_complex_simd(d: &[Complex32], off: usize) -> (f32x8, f32x8) {
    let (mut re, mut im) = ([0.0; 8], [0.0; 8]);
    for (i, c) in d[off..].iter().take(8).enumerate() {
        re[i] = c.re;
        im[i] = c.im;
    }
    (f32x8::new(re), f32x8::new(im))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::{AudioBlock, ProcessorUpdate};
    use std::time::Instant;

    fn make_block(s: Vec<f32>, c: usize, r: f32) -> AudioBlock<'static> {
        AudioBlock::new(Box::leak(s.into_boxed_slice()), c, r, Instant::now())
    }
    fn sine(f: f32, r: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (core::f32::consts::TAU * f * i as f32 / r).sin())
            .collect()
    }
    fn unwrap(r: ProcessorUpdate<SpectrogramUpdate>) -> SpectrogramUpdate {
        match r {
            ProcessorUpdate::Snapshot(u) => u,
            _ => panic!(),
        }
    }

    #[test]
    fn detects_sine_frequency_peak() {
        let cfg = SpectrogramConfig {
            fft_size: 1024,
            hop_size: 512,
            history_length: 8,
            window: WindowKind::Hann,
            zero_padding_factor: 1,
            use_reassignment: false,
            ..Default::default()
        };
        let mut p = SpectrogramProcessor::new(cfg);
        let freq = 200.0 * cfg.sample_rate / 1024.0;
        let b = make_block(sine(freq, cfg.sample_rate, 2048), 1, cfg.sample_rate);
        let u = unwrap(p.process_block(&b));
        let (idx, &db) = u
            .new_columns
            .last()
            .unwrap()
            .magnitudes_db
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        assert_eq!(idx, 200);
        assert!(db > -1.5 && db < 2.0);
    }

    #[test]
    fn mel_conversions_are_invertible() {
        for &h in &[20.0, 100.0, 440.0, 1000.0, 4000.0, 10000.0] {
            assert!((h - mel_to_hz(hz_to_mel(h))).abs() < 0.01);
        }
    }

    #[test]
    fn reassignment_2d_with_group_delay() {
        let cfg = SpectrogramConfig {
            fft_size: 2048,
            hop_size: 512,
            history_length: 4,
            use_reassignment: true,
            reassignment_low_bin_limit: 0,
            zero_padding_factor: 1,
            ..Default::default()
        };
        let mut p = SpectrogramProcessor::new(cfg);
        let freq = 50.3 * cfg.sample_rate / 2048.0;
        let b = make_block(sine(freq, cfg.sample_rate, 4096), 1, cfg.sample_rate);
        let u = unwrap(p.process_block(&b));
        let peak = u
            .new_columns
            .last()
            .unwrap()
            .reassigned
            .as_ref()
            .unwrap()
            .iter()
            .max_by(|a, b| a.magnitude_db.partial_cmp(&b.magnitude_db).unwrap())
            .unwrap();
        assert!((peak.frequency_hz - freq).abs() < 1.0);
        assert!(peak.group_delay_samples.abs() < 204.8);
    }

    #[test]
    fn window_sigma_t_matches_theoretical_ratios() {
        let h = WindowKind::Hann.coefficients(4096);
        assert!((compute_sigma_t(&h) / 4096.0 - 0.1414).abs() < 0.01);
        let b = WindowKind::Blackman.coefficients(4096);
        assert!((compute_sigma_t(&b) / 4096.0 - 0.1188).abs() < 0.01);
    }

    #[test]
    fn chirp_correction_tracks_linear_fm() {
        let sr = 48000.0;
        let cfg = SpectrogramConfig {
            fft_size: 2048,
            hop_size: 256,
            use_reassignment: true,
            zero_padding_factor: 2,
            window: WindowKind::Hann,
            ..Default::default()
        };
        let mut p = SpectrogramProcessor::new(cfg);
        let (f0, rate) = (1000.0, 2000.0);
        let s: Vec<f32> = (0..2048 * 3)
            .map(|n| {
                (core::f32::consts::TAU
                    * (f0 * n as f32 / sr + 0.5 * rate * (n as f32 / sr).powi(2)))
                .sin()
            })
            .collect();
        let u = unwrap(p.process_block(&make_block(s, 1, sr)));
        let mid = u.new_columns.len() / 2;
        let peak = u.new_columns[mid]
            .reassigned
            .as_ref()
            .unwrap()
            .iter()
            .max_by(|a, b| a.magnitude_db.partial_cmp(&b.magnitude_db).unwrap())
            .unwrap();
        let exp = f0 + rate * ((256 * mid + 1024) as f32 / sr);
        assert!((peak.frequency_hz - exp).abs() < (exp * 0.01).max(20.0));
    }
}
