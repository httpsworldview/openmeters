use crate::audio::meter_tap::MeterFormat;
use crate::dsp::spectrogram::FrequencyScale;
use crate::dsp::spectrum::{
    SpectrumConfig, SpectrumProcessor as CoreSpectrumProcessor, SpectrumSnapshot,
};
use crate::dsp::{AudioBlock, AudioProcessor, Reconfigurable};
use crate::ui::render::spectrum::{SpectrumParams, SpectrumPrimitive};
use crate::ui::settings::{SpectrumDisplayMode, SpectrumSettings, SpectrumWeightingMode};
use crate::ui::theme;
use crate::util::audio::musical::MusicalNote;
use crate::util::audio::{hz_to_mel, lerp, mel_to_hz};
use iced::advanced::graphics::text::Paragraph as RenderParagraph;
use iced::advanced::renderer::{self, Quad};
use iced::advanced::text::{self, Paragraph as _, Renderer as _};
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Layout, Renderer as _, Widget, layout, mouse};
use iced::{Background, Color, Element, Length, Point, Rectangle, Size};
use iced_wgpu::primitive::Renderer as _;
use std::cell::RefCell;
use std::sync::Arc;

const EPSILON: f32 = 1e-6;
const GRID_FREQS: &[(f32, u8)] = &[
    (10.0, 0),
    (20.0, 2),
    (31.5, 3),
    (40.0, 2),
    (50.0, 2),
    (63.0, 3),
    (80.0, 2),
    (100.0, 1),
    (125.0, 2),
    (160.0, 2),
    (200.0, 1),
    (250.0, 2),
    (315.0, 3),
    (400.0, 2),
    (500.0, 1),
    (630.0, 2),
    (800.0, 2),
    (1_000.0, 0),
    (1_250.0, 2),
    (1_600.0, 2),
    (2_000.0, 1),
    (2_500.0, 2),
    (3_150.0, 3),
    (4_000.0, 1),
    (5_000.0, 2),
    (6_300.0, 3),
    (8_000.0, 1),
    (10_000.0, 0),
    (16_000.0, 1),
];

pub(crate) struct SpectrumProcessor {
    inner: CoreSpectrumProcessor,
}

impl std::fmt::Debug for SpectrumProcessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpectrumProcessor").finish_non_exhaustive()
    }
}

impl SpectrumProcessor {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            inner: CoreSpectrumProcessor::new(SpectrumConfig {
                sample_rate,
                ..Default::default()
            }),
        }
    }

    pub fn ingest(&mut self, samples: &[f32], format: MeterFormat) -> Option<SpectrumSnapshot> {
        if samples.is_empty() {
            return None;
        }
        let sr = format.sample_rate.max(1.0);
        let mut cfg = self.inner.config();
        if (cfg.sample_rate - sr).abs() > f32::EPSILON {
            cfg.sample_rate = sr;
            self.inner.update_config(cfg);
        }
        self.inner
            .process_block(&AudioBlock::now(samples, format.channels.max(1), sr))
    }

    pub fn update_config(&mut self, c: SpectrumConfig) {
        self.inner.update_config(c);
    }
    pub fn config(&self) -> SpectrumConfig {
        self.inner.config()
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SpectrumStyle {
    pub min_db: f32,
    pub max_db: f32,
    pub min_frequency: f32,
    pub max_frequency: f32,
    pub resolution: usize,
    pub line_thickness: f32,
    pub secondary_line_thickness: f32,
    pub smoothing_radius: usize,
    pub smoothing_passes: usize,
    pub highlight_threshold: f32,
    pub spectrum_palette: [Color; 6],
    pub frequency_scale: FrequencyScale,
    pub reverse_frequency: bool,
    pub show_grid: bool,
    pub show_peak_label: bool,
    pub display_mode: SpectrumDisplayMode,
    pub weighting_mode: SpectrumWeightingMode,
    pub show_secondary_line: bool,
    pub bar_count: usize,
    pub bar_gap: f32,
}

impl Default for SpectrumStyle {
    fn default() -> Self {
        let defaults = SpectrumSettings::default();
        Self {
            min_db: -120.0,
            max_db: 0.0,
            min_frequency: 10.0,
            max_frequency: 20_000.0,
            resolution: 1024,
            line_thickness: 0.5,
            secondary_line_thickness: 0.5,
            smoothing_radius: defaults.smoothing_radius,
            smoothing_passes: defaults.smoothing_passes,
            highlight_threshold: defaults.highlight_threshold,
            spectrum_palette: theme::spectrum::COLORS,
            frequency_scale: defaults.frequency_scale,
            reverse_frequency: defaults.reverse_frequency,
            show_grid: defaults.show_grid,
            show_peak_label: defaults.show_peak_label,
            display_mode: defaults.display_mode,
            weighting_mode: defaults.weighting_mode,
            show_secondary_line: defaults.show_secondary_line,
            bar_count: defaults.bar_count,
            bar_gap: defaults.bar_gap,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PeakLabel {
    text: String,
    x: f32,
    y: f32,
    opacity: f32,
}

#[derive(Debug, Clone)]
pub(crate) struct SpectrumState {
    style: SpectrumStyle,
    weighted: Arc<[[f32; 2]]>,
    unweighted: Arc<[[f32; 2]]>,
    key: u64,
    peak: Option<PeakLabel>,
    grid: Arc<[(f32, String, u8)]>,
    scratch: Vec<f32>,
}

impl SpectrumState {
    pub fn new() -> Self {
        Self {
            style: SpectrumStyle::default(),
            weighted: Arc::from([]),
            unweighted: Arc::from([]),
            key: super::next_key(),
            peak: None,
            grid: Arc::from([]),
            scratch: Vec::new(),
        }
    }

    pub fn style(&self) -> &SpectrumStyle {
        &self.style
    }
    pub fn style_mut(&mut self) -> &mut SpectrumStyle {
        &mut self.style
    }

    pub fn update_show_grid(&mut self, show: bool) {
        self.style.show_grid = show;
        if !show {
            self.grid = Arc::from([]);
        }
    }

    pub fn update_show_peak_label(&mut self, show: bool) {
        self.style.show_peak_label = show;
        if !show {
            self.peak = None;
        }
    }

    pub fn set_palette(&mut self, palette: &[Color]) {
        if palette.len() == 6 && !theme::palettes_equal(&self.style.spectrum_palette, palette) {
            self.style.spectrum_palette.copy_from_slice(palette);
        }
    }

    pub fn palette(&self) -> [Color; 6] {
        self.style.spectrum_palette
    }

    pub fn apply_snapshot(&mut self, snap: &SpectrumSnapshot) {
        if snap.frequency_bins.is_empty()
            || snap.magnitudes_db.is_empty()
            || snap.frequency_bins.len() != snap.magnitudes_db.len()
        {
            self.fade_peak(None);
            return;
        }
        let nyq = snap
            .frequency_bins
            .last()
            .copied()
            .unwrap_or(self.style.max_frequency);
        let (min_f, mut max_f) = (
            self.style.min_frequency.max(EPSILON),
            self.style.max_frequency.min(nyq),
        );
        if max_f <= min_f {
            max_f = nyq.max(min_f * 1.02);
        }
        if max_f <= min_f {
            self.fade_peak(None);
            return;
        }

        let scale = Scale::new(min_f, max_f);
        let res = self.style.resolution.max(32);
        let (mut w, mut u) = (Vec::new(), Vec::new());
        build_points(&self.style, &mut w, &mut u, res, &scale, snap);

        if self.style.smoothing_radius > 0 && self.style.smoothing_passes > 0 {
            smooth(
                &mut w[..],
                self.style.smoothing_radius,
                self.style.smoothing_passes,
                &mut self.scratch,
            );
            smooth(
                &mut u[..],
                self.style.smoothing_radius,
                self.style.smoothing_passes,
                &mut self.scratch,
            );
        }
        if self.style.reverse_frequency {
            w.reverse();
            u.reverse();
            reindex(&mut w[..]);
            reindex(&mut u[..]);
        }

        self.weighted = Arc::from(w);
        self.unweighted = Arc::from(u);

        self.grid = if self.style.show_grid {
            let mut v = Vec::new();
            for &(f, imp) in GRID_FREQS {
                if f < min_f || f > max_f {
                    continue;
                }
                let mut p = scale.pos_of(self.style.frequency_scale, f);
                if self.style.reverse_frequency {
                    p = 1.0 - p;
                }
                if p.is_finite() {
                    v.push((p.clamp(0.0, 1.0), fmt_freq(f), imp));
                }
            }
            Arc::from(v)
        } else {
            Arc::from([])
        };

        let pk = self
            .style
            .show_peak_label
            .then(|| self.build_peak(snap, &scale))
            .flatten();
        self.fade_peak(pk);
    }

    fn build_peak(&self, snap: &SpectrumSnapshot, sc: &Scale) -> Option<PeakLabel> {
        let f = snap
            .peak_frequency_hz
            .filter(|&f| f.is_finite() && f > 0.0)?;
        let mut x = sc.pos_of(self.style.frequency_scale, f);
        if self.style.reverse_frequency {
            x = 1.0 - x;
        }
        let m = interp(&snap.frequency_bins, &snap.magnitudes_db, f);
        let y = ((m - self.style.min_db) / (self.style.max_db - self.style.min_db).max(EPSILON))
            .clamp(0.0, 1.0);
        if y < 0.08 {
            return None;
        }
        let text = MusicalNote::from_frequency(f).map_or_else(
            || format!("{:.1} Hz", f),
            |n| format!("{:.1} Hz | {}", f, n.format()),
        );
        Some(PeakLabel {
            text,
            x: x.clamp(0.0, 1.0),
            y,
            opacity: 0.0,
        })
    }

    fn fade_peak(&mut self, incoming: Option<PeakLabel>) {
        match (incoming, &mut self.peak) {
            (Some(new), Some(p)) => {
                p.text = new.text;
                p.x = new.x;
                p.y = new.y;
                p.opacity = (p.opacity + (1.0 - p.opacity) * 0.35).min(1.0);
            }
            (Some(new), None) => {
                self.peak = Some(new);
            }
            (None, Some(p)) => {
                p.opacity += (0.0 - p.opacity) * 0.12;
                if p.opacity < 0.01 {
                    self.peak = None;
                }
            }
            (None, None) => {}
        }
    }

    pub fn grid(&self) -> Arc<[(f32, String, u8)]> {
        Arc::clone(&self.grid)
    }

    pub fn peak(&self) -> Option<&PeakLabel> {
        self.style
            .show_peak_label
            .then_some(self.peak.as_ref())
            .flatten()
    }

    fn visual_params(&self, bounds: Rectangle, theme: &iced::Theme) -> Option<SpectrumParams> {
        if self.weighted.len() < 2 {
            return None;
        }
        let pal = theme.extended_palette();

        let (primary, secondary) = match self.style.weighting_mode {
            SpectrumWeightingMode::AWeighted => (&self.weighted, &self.unweighted),
            SpectrumWeightingMode::Raw => (&self.unweighted, &self.weighted),
        };

        Some(SpectrumParams {
            bounds,
            normalized_points: Arc::clone(primary),
            secondary_points: Arc::clone(secondary),
            key: self.key,
            line_color: theme::color_to_rgba(theme::mix_colors(
                pal.primary.base.color,
                pal.background.base.text,
                0.35,
            )),
            line_width: self.style.line_thickness,
            secondary_line_color: theme::color_to_rgba(theme::with_alpha(
                pal.secondary.weak.text,
                0.3,
            )),
            secondary_line_width: self.style.secondary_line_thickness,
            highlight_threshold: self.style.highlight_threshold,
            spectrum_palette: self
                .style
                .spectrum_palette
                .map(theme::color_to_rgba)
                .to_vec(),
            display_mode: self.style.display_mode == SpectrumDisplayMode::Bar,
            show_secondary_line: self.style.show_secondary_line,
            bar_count: self.style.bar_count,
            bar_gap: self.style.bar_gap,
        })
    }
}

#[derive(Debug)]
pub(crate) struct Spectrum<'a>(&'a RefCell<SpectrumState>);
impl<'a> Spectrum<'a> {
    pub fn new(state: &'a RefCell<SpectrumState>) -> Self {
        Self(state)
    }
}

impl<'a, M> Widget<M, iced::Theme, iced::Renderer> for Spectrum<'a> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<()>()
    }
    fn state(&self) -> tree::State {
        tree::State::None
    }
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }
    fn children(&self) -> Vec<Tree> {
        Vec::new()
    }
    fn diff(&self, _: &mut Tree) {}
    fn layout(&mut self, _: &mut Tree, _: &iced::Renderer, lim: &layout::Limits) -> layout::Node {
        layout::Node::new(lim.resolve(Length::Fill, Length::Fill, Size::ZERO))
    }
    fn draw(
        &self,
        _: &Tree,
        r: &mut iced::Renderer,
        th: &iced::Theme,
        _: &renderer::Style,
        lay: Layout<'_>,
        _: mouse::Cursor,
        _: &Rectangle,
    ) {
        let b = lay.bounds();
        let state = self.0.borrow();
        let Some(params) = state.visual_params(b, th) else {
            r.fill_quad(
                Quad {
                    bounds: b,
                    border: Default::default(),
                    shadow: Default::default(),
                    snap: true,
                },
                Background::Color(th.extended_palette().background.base.color),
            );
            return;
        };
        let grid = state.grid();
        if !grid.is_empty() {
            r.with_layer(b, |r| draw_grid(r, th, b, &grid));
        }
        r.draw_primitive(b, SpectrumPrimitive::new(params));
        if let Some(pk) = state.peak() {
            r.with_layer(b, |r| draw_peak(r, th, b, pk));
        }
    }
}

pub(crate) fn widget<'a, M: 'a>(state: &'a RefCell<SpectrumState>) -> Element<'a, M> {
    Element::new(Spectrum::new(state))
}

// --- Helpers ---

#[derive(Clone, Copy)]
struct Scale {
    min: f32,
    max: f32,
    log_min: f32,
    log_range: f32,
    mel_min: f32,
    mel_range: f32,
}

impl Scale {
    fn new(min: f32, max: f32) -> Self {
        let log_min = min.max(EPSILON).log10();
        let log_range = (max.max(min * 1.01).log10() - log_min).max(EPSILON);
        let mel_min = hz_to_mel(min);
        Self {
            min,
            max,
            log_min,
            log_range,
            mel_min,
            mel_range: (hz_to_mel(max) - mel_min).max(EPSILON),
        }
    }
    fn freq_at(&self, s: FrequencyScale, t: f32) -> f32 {
        match s {
            FrequencyScale::Linear => self.min + (self.max - self.min) * t,
            FrequencyScale::Logarithmic => 10f32.powf(self.log_min + self.log_range * t),
            FrequencyScale::Mel => mel_to_hz(self.mel_min + self.mel_range * t),
        }
    }
    fn pos_of(&self, s: FrequencyScale, f: f32) -> f32 {
        let f = f.clamp(self.min, self.max);
        match s {
            FrequencyScale::Linear => (f - self.min) / (self.max - self.min).max(EPSILON),
            FrequencyScale::Logarithmic => (f.max(EPSILON).log10() - self.log_min) / self.log_range,
            FrequencyScale::Mel => (hz_to_mel(f) - self.mel_min) / self.mel_range,
        }
        .clamp(0.0, 1.0)
    }
}

fn interp(bins: &[f32], mags: &[f32], t: f32) -> f32 {
    if bins.is_empty() || t <= bins[0] {
        return mags.first().copied().unwrap_or(0.0);
    }
    if t >= *bins.last().unwrap() {
        return mags.last().copied().unwrap_or(0.0);
    }
    match bins.binary_search_by(|p| p.partial_cmp(&t).unwrap_or(std::cmp::Ordering::Less)) {
        Ok(i) => mags.get(i).copied().unwrap_or(0.0),
        Err(i) => {
            let (lo, hi) = (i.saturating_sub(1), i.min(bins.len() - 1));
            lerp(
                mags.get(lo).copied().unwrap_or(0.0),
                mags.get(hi).copied().unwrap_or(0.0),
                (t - bins[lo]) / (bins[hi] - bins[lo]).max(EPSILON),
            )
        }
    }
}

fn build_points(
    style: &SpectrumStyle,
    w: &mut Vec<[f32; 2]>,
    u: &mut Vec<[f32; 2]>,
    res: usize,
    sc: &Scale,
    snap: &SpectrumSnapshot,
) {
    w.clear();
    u.clear();
    w.reserve(res);
    u.reserve(res);
    let dr = (style.max_db - style.min_db).max(EPSILON);
    for i in 0..res {
        let t = if res > 1 {
            i as f32 / (res - 1) as f32
        } else {
            0.0
        };
        let f = sc.freq_at(style.frequency_scale, t);
        let mw = interp(&snap.frequency_bins, &snap.magnitudes_db, f);
        let mu = interp(&snap.frequency_bins, &snap.magnitudes_unweighted_db, f);
        w.push([t, ((mw - style.min_db) / dr).clamp(0.0, 1.0)]);
        u.push([t, ((mu - style.min_db) / dr).clamp(0.0, 1.0)]);
    }
}

fn smooth(pts: &mut [[f32; 2]], r: usize, passes: usize, scratch: &mut Vec<f32>) {
    if r == 0 || passes == 0 || pts.len() < 3 {
        return;
    }
    scratch.resize(pts.len(), 0.0);
    for _ in 0..passes {
        for (d, p) in scratch.iter_mut().zip(pts.iter()) {
            *d = p[1];
        }
        for (i, p) in pts.iter_mut().enumerate() {
            let (s, e) = (i.saturating_sub(r), (i + r + 1).min(scratch.len()));
            let (mut sum, mut wsum) = (0.0f32, 0.0f32);
            for (j, &v) in scratch[s..e].iter().enumerate() {
                let w = (r - (s + j).abs_diff(i) + 1) as f32;
                sum += v * w;
                wsum += w;
            }
            p[1] = sum / wsum;
        }
    }
}

fn reindex(pts: &mut [[f32; 2]]) {
    let n = pts.len();
    for (i, p) in pts.iter_mut().enumerate() {
        p[0] = if n > 1 {
            i as f32 / (n - 1) as f32
        } else {
            0.0
        };
    }
}

fn fmt_freq(f: f32) -> String {
    match f {
        f if f >= 10_000.0 => format!("{:.0} kHz", f / 1000.0),
        f if f >= 1_000.0 => format!("{:.1} kHz", f / 1000.0),
        f if f >= 100.0 => format!("{:.0} Hz", f),
        f if f >= 10.0 => format!("{:.1} Hz", f),
        _ => format!("{:.2} Hz", f),
    }
}

fn measure_text(s: &str, px: f32) -> Size {
    RenderParagraph::with_text(text::Text {
        content: s,
        bounds: Size::INFINITE,
        size: iced::Pixels(px),
        font: iced::Font::default(),
        align_x: iced::alignment::Horizontal::Left.into(),
        align_y: iced::alignment::Vertical::Top,
        line_height: text::LineHeight::default(),
        shaping: text::Shaping::Basic,
        wrapping: text::Wrapping::None,
    })
    .min_bounds()
}

fn make_text(s: &str, px: f32, bounds: Size) -> text::Text<String> {
    text::Text {
        content: s.to_string(),
        bounds,
        size: iced::Pixels(px),
        font: iced::Font::default(),
        align_x: iced::alignment::Horizontal::Left.into(),
        align_y: iced::alignment::Vertical::Top,
        line_height: text::LineHeight::default(),
        shaping: text::Shaping::Basic,
        wrapping: text::Wrapping::None,
    }
}

fn draw_grid(r: &mut iced::Renderer, th: &iced::Theme, b: Rectangle, lines: &[(f32, String, u8)]) {
    if b.width <= 0.0 || b.height <= 0.0 {
        return;
    }
    let pal = th.extended_palette();
    let (lc, tc) = (
        theme::with_alpha(pal.background.base.text, 0.25),
        theme::with_alpha(pal.background.base.text, 0.75),
    );
    let cands: Vec<_> = lines
        .iter()
        .filter_map(|(pos, lbl, imp)| {
            let x = b.x + b.width * pos;
            (x >= b.x - 1.0 && x <= b.x + b.width + 1.0)
                .then(|| {
                    let sz = measure_text(lbl, 10.0);
                    (sz.width > 0.0 && sz.height > 0.0).then_some((x, lbl, *imp, sz))
                })
                .flatten()
        })
        .collect();
    let mut acc = Vec::with_capacity(cands.len());
    let mut indices: Vec<usize> = (0..cands.len()).collect();
    indices.sort_by_key(|&i| cands[i].2);

    let bounds = |i| {
        let (x, _, _, sz): (f32, &String, u8, Size) = cands[i];
        let l =
            (x - sz.width * 0.5).clamp(b.x + 6.0, (b.x + b.width - 6.0 - sz.width).max(b.x + 6.0));
        (l, l + sz.width + 6.0)
    };

    for i in indices {
        let (l, r) = bounds(i);
        if !acc.iter().any(|&j| {
            let (ol, or) = bounds(j);
            l < or && r > ol
        }) {
            acc.push(i);
        }
    }
    for &i in &acc {
        let (x, lbl, _, sz) = &cands[i];
        let (ty, lt) = (b.y + 6.0, b.y + 6.0 + sz.height + 6.0);
        let lh = (b.y + b.height - lt).max(0.0);
        if lh > 0.0 {
            r.fill_quad(
                Quad {
                    bounds: Rectangle::new(
                        Point::new((x - 0.5).clamp(b.x, b.x + b.width - 1.0), lt),
                        Size::new(1.0, lh),
                    ),
                    border: Default::default(),
                    shadow: Default::default(),
                    snap: true,
                },
                Background::Color(lc),
            );
        }
        let tx =
            (x - sz.width * 0.5).clamp(b.x + 6.0, (b.x + b.width - 6.0 - sz.width).max(b.x + 6.0));
        r.fill_text(
            make_text(lbl, 10.0, *sz),
            Point::new(tx, ty),
            tc,
            Rectangle::new(Point::new(tx, ty), *sz),
        );
    }
}

fn draw_peak(r: &mut iced::Renderer, th: &iced::Theme, b: Rectangle, pk: &PeakLabel) {
    if pk.opacity < 0.01 || b.width < 8.0 || b.height < 8.0 {
        return;
    }
    let sz = measure_text(&pk.text, 12.0);
    if sz.width <= 0.0 || sz.height <= 0.0 {
        return;
    }
    let (ax, ay) = (
        b.x + b.width * pk.x.clamp(0.0, 1.0),
        b.y + b.height * (1.0 - pk.y.clamp(0.0, 1.0)),
    );
    let tx =
        (ax - sz.width * 0.5).clamp(b.x + 4.0, (b.x + b.width - 4.0 - sz.width).max(b.x + 4.0));
    let ty =
        (ay - 8.0 - sz.height).clamp(b.y + 4.0, (b.y + b.height - 4.0 - sz.height).max(b.y + 4.0));
    let bg = Rectangle::new(
        Point::new(tx - 4.0, ty - 4.0),
        Size::new(sz.width + 8.0, sz.height + 8.0),
    );
    let mut bdr = theme::sharp_border();
    bdr.color = theme::with_alpha(bdr.color, pk.opacity);
    let pal = th.extended_palette();
    r.fill_quad(
        Quad {
            bounds: bg,
            border: bdr,
            shadow: Default::default(),
            snap: true,
        },
        Background::Color(theme::with_alpha(pal.background.strong.color, pk.opacity)),
    );
    r.fill_text(
        make_text(&pk.text, 12.0, sz),
        Point::new(tx, ty),
        theme::with_alpha(pal.background.base.text, pk.opacity),
        Rectangle::new(Point::new(tx, ty), sz),
    );
}
