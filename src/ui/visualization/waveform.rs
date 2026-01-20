use crate::audio::meter_tap::MeterFormat;
use crate::dsp::waveform::{
    DEFAULT_COLUMN_CAPACITY, MAX_COLUMN_CAPACITY, MIN_COLUMN_CAPACITY, WaveformConfig,
    WaveformPreview, WaveformProcessor as CoreWaveformProcessor, WaveformSnapshot,
};
use crate::dsp::{AudioBlock, AudioProcessor, Reconfigurable};
use crate::ui::render::waveform::{PreviewSample, WaveformParams, WaveformPrimitive};
use crate::ui::settings::ChannelMode;
use crate::ui::theme;
use crate::visualization_widget;
use iced::Color;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

const COLUMN_PX: f32 = 2.0;

#[derive(Debug, Clone)]
pub struct WaveformProcessor {
    inner: CoreWaveformProcessor,
    channels: usize,
}

impl WaveformProcessor {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            inner: CoreWaveformProcessor::new(WaveformConfig {
                sample_rate,
                ..Default::default()
            }),
            channels: 2,
        }
    }

    pub fn sync_capacity(state: &RefCell<WaveformState>, processor: &mut Self) {
        let target = state
            .borrow()
            .desired_columns()
            .clamp(MIN_COLUMN_CAPACITY, MAX_COLUMN_CAPACITY);
        let mut cfg = processor.config();
        if cfg.max_columns != target {
            cfg.max_columns = target;
            processor.update_config(cfg);
        }
    }

    pub fn ingest(&mut self, samples: &[f32], format: MeterFormat) -> Option<WaveformSnapshot> {
        if samples.is_empty() {
            return None;
        }
        self.channels = format.channels.max(1);
        let sr = format.sample_rate.max(1.0);
        let mut cfg = self.inner.config();
        if (cfg.sample_rate - sr).abs() > f32::EPSILON {
            cfg.sample_rate = sr;
            self.inner.update_config(cfg);
        }
        self.inner
            .process_block(&AudioBlock::now(samples, self.channels, sr))
            .into()
    }
    pub fn update_config(&mut self, config: WaveformConfig) {
        self.inner.update_config(config);
    }
    pub fn config(&self) -> WaveformConfig {
        self.inner.config()
    }
}

#[derive(Debug, Default, Clone)]
struct RenderCache {
    samples: Arc<[[f32; 2]]>,
    colors: Arc<[[f32; 4]]>,
    preview: Arc<[PreviewSample]>,
}

#[derive(Debug, Clone)]
pub struct WaveformState {
    snapshot: WaveformSnapshot,
    style: WaveformStyle,
    desired_cols: Rc<Cell<usize>>,
    key: u64,
    cache: RefCell<RenderCache>,
    ch_mode: ChannelMode,
}

impl WaveformState {
    fn next_key() -> u64 {
        static K: AtomicU64 = AtomicU64::new(1);
        K.fetch_add(1, Ordering::Relaxed)
    }

    pub fn new() -> Self {
        Self {
            snapshot: WaveformSnapshot::default(),
            style: WaveformStyle::default(),
            desired_cols: Rc::new(Cell::new(DEFAULT_COLUMN_CAPACITY)),
            key: Self::next_key(),
            cache: RefCell::new(RenderCache::default()),
            ch_mode: ChannelMode::default(),
        }
    }

    pub fn apply_snapshot(&mut self, s: WaveformSnapshot) {
        self.snapshot = Self::project(&s, self.ch_mode);
    }
    pub fn set_channel_mode(&mut self, m: ChannelMode) {
        if self.ch_mode != m {
            self.ch_mode = m;
            self.snapshot = Self::project(&self.snapshot, m);
        }
    }
    pub fn channel_mode(&self) -> ChannelMode {
        self.ch_mode
    }
    pub fn set_palette(&mut self, p: &[Color]) {
        self.style.set_palette(p);
        self.key = Self::next_key();
    }
    pub fn palette(&self) -> &[Color] {
        self.style.palette()
    }
    pub fn desired_columns(&self) -> usize {
        self.desired_cols.get()
    }

    pub fn visual(&self, bounds: iced::Rectangle) -> Option<WaveformParams> {
        let (ch, cols) = (self.snapshot.channels.max(1), self.snapshot.columns);
        if bounds.width <= 0.0 {
            return None;
        }
        let need = ((bounds.width / COLUMN_PX).ceil() as usize).clamp(1, MAX_COLUMN_CAPACITY);
        self.desired_cols.set(need);
        let exp = cols * ch;
        if cols == 0
            || [
                &self.snapshot.min_values,
                &self.snapshot.max_values,
                &self.snapshot.frequency_normalized,
            ]
            .iter()
            .any(|v| v.len() != exp)
        {
            return None;
        }

        let (vis, start) = (need.min(cols), cols - need.min(cols));
        let mut c = self.cache.borrow_mut();
        let mut samples = Vec::with_capacity(vis * ch);
        for ci in 0..ch {
            let base = ci * cols;
            for i in start..cols {
                let (v0, v1) = (
                    self.snapshot.min_values[base + i],
                    self.snapshot.max_values[base + i],
                );
                samples.push([v0.min(v1), v0.max(v1)]);
            }
        }
        let mut colors = Vec::with_capacity(vis * ch);
        for ci in 0..ch {
            let base = ci * cols;
            for i in start..cols {
                colors.push(theme::color_to_rgba(
                    self.style
                        .freq_color(self.snapshot.frequency_normalized[base + i]),
                ));
            }
        }
        c.samples = Arc::from(samples);
        c.colors = Arc::from(colors);
        let (samples, colors) = (Arc::clone(&c.samples), Arc::clone(&c.colors));
        let pv = &self.snapshot.preview;
        let pv_ok = pv.progress > 0.0 && pv.min_values.len() >= ch && pv.max_values.len() >= ch;
        let pv_prog = if pv_ok {
            pv.progress.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let preview = {
            let mut buf = Vec::new();
            if pv_ok {
                buf.reserve(ch);
                for ci in 0..ch {
                    let (v0, v1) = (pv.min_values[ci], pv.max_values[ci]);
                    buf.push(PreviewSample {
                        min: v0.min(v1).clamp(-1.0, 1.0),
                        max: v0.max(v1).clamp(-1.0, 1.0),
                        color: theme::color_to_rgba(
                            self.style.freq_color(freq_hint(&self.snapshot, ci)),
                        ),
                    });
                }
            }
            c.preview = Arc::from(buf);
            Arc::clone(&c.preview)
        };

        Some(WaveformParams {
            bounds,
            channels: ch,
            column_width: COLUMN_PX,
            columns: vis,
            samples,
            colors,
            preview_samples: preview,
            preview_progress: pv_prog,
            fill_alpha: self.style.fill_alpha,
            line_alpha: self.style.line_alpha,
            vertical_padding: self.style.vert_pad,
            channel_gap: self.style.ch_gap,
            amplitude_scale: self.style.amp_scale,
            stroke_width: self.style.stroke,
            instance_key: self.key,
        })
    }

    fn project(src: &WaveformSnapshot, mode: ChannelMode) -> WaveformSnapshot {
        let (ch, cols) = (src.channels.max(1), src.columns);
        let exp = ch * cols;
        if cols == 0
            || [&src.min_values, &src.max_values, &src.frequency_normalized]
                .iter()
                .any(|v| v.len() < exp)
        {
            return WaveformSnapshot::default();
        }
        let proj = |d: &[f32], s| mode.project_data(d, s, ch);
        let p = &src.preview;
        let pv_ok = p.min_values.len() >= ch && p.max_values.len() >= ch;
        WaveformSnapshot {
            channels: mode.output_channels(ch),
            columns: cols,
            min_values: proj(&src.min_values, cols),
            max_values: proj(&src.max_values, cols),
            frequency_normalized: proj(&src.frequency_normalized, cols),
            column_spacing_seconds: src.column_spacing_seconds,
            scroll_position: src.scroll_position,
            preview: if pv_ok {
                WaveformPreview {
                    progress: p.progress,
                    min_values: proj(&p.min_values, 1),
                    max_values: proj(&p.max_values, 1),
                }
            } else {
                WaveformPreview::default()
            },
        }
    }
}

fn freq_hint(s: &WaveformSnapshot, ch: usize) -> f32 {
    let c = s.columns;
    if c == 0 {
        return 0.0;
    }
    s.frequency_normalized
        .get(ch * c..(ch + 1) * c)
        .and_then(|r| r.iter().rev().copied().find(|v| v.is_finite() && *v > 0.0))
        .unwrap_or(0.0)
}

#[derive(Debug, Clone)]
pub struct WaveformStyle {
    pub fill_alpha: f32,
    pub line_alpha: f32,
    pub vert_pad: f32,
    pub ch_gap: f32,
    pub amp_scale: f32,
    pub stroke: f32,
    palette: Vec<Color>,
}

impl Default for WaveformStyle {
    fn default() -> Self {
        Self {
            fill_alpha: 1.0,
            line_alpha: 1.0,
            vert_pad: 8.0,
            ch_gap: 12.0,
            amp_scale: 1.0,
            stroke: 1.0,
            palette: theme::DEFAULT_WAVEFORM_PALETTE.to_vec(),
        }
    }
}

impl WaveformStyle {
    fn freq_color(&self, v: f32) -> Color {
        theme::sample_gradient(&self.palette, v)
    }
    fn set_palette(&mut self, p: &[Color]) {
        if !theme::palettes_equal(&self.palette, p) {
            self.palette = p.to_vec();
        }
    }
    pub fn palette(&self) -> &[Color] {
        &self.palette
    }
}

visualization_widget!(
    Waveform,
    WaveformState,
    WaveformPrimitive,
    |state, bounds| state.visual(bounds),
    |params| WaveformPrimitive::new(params)
);
