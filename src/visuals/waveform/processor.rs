// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::dsp::{AudioBlock, RunningMeans, SampleRate, ThreeBand};
pub(super) use crate::util::audio::BAND_COUNT;
use crate::util::audio::{BAND_SPLITS_HZ, Channel, DB_FLOOR, power_to_db};

pub const MAX_COLUMN_CAPACITY: usize = 8_192;

const DEFAULT_SCROLL_SPEED: f32 = 200.0;
pub const DEFAULT_BAND_DB_FLOOR: f32 = -50.0;
pub(super) const WAVEFORM_CHANNELS: [Channel; 4] =
    [Channel::Left, Channel::Right, Channel::Mid, Channel::Side];
pub(super) const DERIVED_CHANNELS: usize = WAVEFORM_CHANNELS.len();
const REFERENCE_SAMPLE_RATE: f32 = 44_100.0;
// Rate-scaled to preserve duration.
const BAND_COLOR_WINDOW_AT_44K1: usize = 2048;
const BAND_SLOW_WINDOW_AT_44K1: usize = 16_384;
// Color weights only; RMS is unscaled.
const BAND_COLOR_GAINS: [f32; BAND_COUNT] = [1.0, 0.7, 2.0];
// Rendering cutoff: -96 dBFS amplitude, also used for color brightness.
pub(super) const WAVEFORM_SILENCE_AMPLITUDE: f32 = 1.584_893_1e-5;

crate::macros::bounded! {
    // Columns per second. The floor bounds the color window, which spans a column.
    pub struct ScrollSpeed(f32) in 1.0..=f32::MAX;
}

crate::macros::default_struct! {
    #[derive(Debug, Clone, Copy)]
    pub struct WaveformConfig {
        pub sample_rate: SampleRate = SampleRate::DEFAULT,
        pub scroll_speed: ScrollSpeed = ScrollSpeed::must(DEFAULT_SCROLL_SPEED),
        pub max_columns: usize = MAX_COLUMN_CAPACITY,
        pub bands: Bands = Bands::Colors,
    }
}

// What the split bands feed; each level includes the one before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bands {
    Off,
    Colors,
    History,
}

crate::macros::default_struct! {
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct WaveColumn {
        pub min: f32 = 0.0,
        pub max: f32 = 0.0,
        pub color_bands: [f32; BAND_COUNT] = [0.0; BAND_COUNT],
        pub rms_db: [[f32; BAND_COUNT]; 2] = [[DB_FLOOR; BAND_COUNT]; 2],
    }
}

pub(super) type WaveFrame = [WaveColumn; DERIVED_CHANNELS];

#[derive(Debug, Clone, Copy, Default)]
pub struct WaveformPreview {
    pub progress: f32,
    pub columns: Option<WaveFrame>,
}

pub struct WaveformUpdate<'a> {
    pub reset: bool,
    pub columns: &'a [WaveFrame],
    pub preview: WaveformPreview,
}

fn window_len(samples_at_reference_rate: usize, sample_rate: SampleRate) -> usize {
    ((samples_at_reference_rate as f32 * sample_rate.hz() / REFERENCE_SAMPLE_RATE).round() as usize)
        .max(1)
}

type BandFilter = ThreeBand<2, 1, false>;

struct BandTracker {
    color: RunningMeans<{ BAND_COUNT * DERIVED_CHANNELS }, 1>,
    history: Option<RunningMeans<{ BAND_COUNT * DERIVED_CHANNELS }, 2>>,
}

// Spans at least one column so slow scrolling does not skip audio.
fn color_window(config: WaveformConfig) -> usize {
    window_len(BAND_COLOR_WINDOW_AT_44K1, config.sample_rate)
        .max((config.sample_rate.hz() / config.scroll_speed.get()).ceil() as usize)
}

fn history_windows(config: WaveformConfig) -> [usize; 2] {
    [
        color_window(config),
        window_len(BAND_SLOW_WINDOW_AT_44K1, config.sample_rate),
    ]
}

impl BandTracker {
    fn new(config: WaveformConfig) -> Self {
        Self {
            color: RunningMeans::new([color_window(config)]),
            history: (config.bands == Bands::History)
                .then(|| RunningMeans::new(history_windows(config))),
        }
    }

    // Keeps what is already averaged, so a scroll change does not restart the slow means.
    fn resize(&mut self, config: WaveformConfig) {
        self.color = self.color.resized([color_window(config)]);
        if let Some(history) = &mut self.history {
            *history = history.resized(history_windows(config));
        }
    }

    fn process(&mut self, bands: [[f32; BAND_COUNT]; DERIVED_CHANNELS]) {
        let bands = bands.as_flattened();
        self.color.push_nonnegative_finite(std::array::from_fn(|i| {
            crate::util::finite_or(bands[i].abs() * BAND_COLOR_GAINS[i % BAND_COUNT], 0.0)
        }));
        if let Some(history) = &mut self.history {
            history.push_nonnegative_finite(std::array::from_fn(|i| {
                crate::util::finite_or(bands[i] * bands[i], 0.0)
            }));
        }
    }
}

crate::macros::default_struct! {
    pub struct WaveformProcessor {
        config: WaveformConfig = WaveformConfig::default(),
        band_analysis: Option<(BandFilter, BandTracker)> = None,
        column_phase: f64 = 0.0,
        current: [Option<(f32, f32, Option<f32>)>; DERIVED_CHANNELS] = [None; DERIVED_CHANNELS],
        last_sample: [Option<f32>; DERIVED_CHANNELS] = [None; DERIVED_CHANNELS],
        pending_columns: Vec<WaveFrame> = Vec::new(),
        reset_pending: bool = true,
    }
}

impl WaveformProcessor {
    pub fn new(config: WaveformConfig) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    pub fn config(&self) -> WaveformConfig {
        self.config
    }

    pub(in crate::visuals) fn prepare(&mut self) {
        if self.band_analysis.is_none() {
            self.band_analysis = Self::band_analysis(self.config);
        }
    }

    pub fn reset_audio(&mut self) {
        self.column_phase = 0.0;
        self.last_sample = [None; DERIVED_CHANNELS];
        self.pending_columns.clear();
        self.current = [None; DERIVED_CHANNELS];
        if self.band_analysis.is_some() {
            self.band_analysis = Self::band_analysis(self.config);
        }
        self.reset_pending = true;
    }

    fn band_analysis(config: WaveformConfig) -> Option<(BandFilter, BandTracker)> {
        (config.bands != Bands::Off).then(|| {
            (
                BandFilter::new(config.sample_rate, BAND_SPLITS_HZ),
                BandTracker::new(config),
            )
        })
    }

    fn columns(&self) -> WaveFrame {
        let mut columns = std::array::from_fn(|channel| {
            let (min, max) = self.current[channel].map_or((0.0, 0.0), |(min, max, _)| {
                self.last_sample[channel].map_or((min, max), |last| (min.min(last), max.max(last)))
            });
            WaveColumn {
                min,
                max,
                ..WaveColumn::default()
            }
        });
        if let Some((_, tracker)) = &self.band_analysis {
            let means = tracker.color.mean(0).map(|mean| mean.max(0.0) as f32);
            for (column, bands) in columns.iter_mut().zip(means.as_chunks::<BAND_COUNT>().0) {
                column.color_bands = *bands;
            }
            if let Some(history) = &tracker.history {
                for window in 0..2 {
                    let db = history
                        .mean(window)
                        .map(|mean| power_to_db(mean.max(0.0) as f32, DB_FLOOR));
                    for (column, bands) in columns.iter_mut().zip(db.as_chunks::<BAND_COUNT>().0) {
                        column.rms_db[window] = *bands;
                    }
                }
            }
        }
        columns
    }

    fn emit_column(&mut self) {
        let columns = self.columns();
        for channel in 0..DERIVED_CHANNELS {
            if let Some((_, _, Some(last))) = self.current[channel] {
                self.last_sample[channel] = Some(last);
            }
        }
        self.pending_columns.push(columns);
        if self.pending_columns.len() >= self.config.max_columns * 2 {
            self.cap_pending_columns();
        }
        self.current = [None; DERIVED_CHANNELS];
    }

    fn ingest_samples(&mut self, block: &AudioBlock<'_>) {
        let step = (f64::from(self.config.scroll_speed.get())
            / f64::from(self.config.sample_rate.get()))
        .min(1.0);
        for stereo in block.stereo_frames() {
            let derived = WAVEFORM_CHANNELS.map(|channel| channel.project(stereo));
            let finite = derived.map(f32::is_finite);
            if let Some((filter, tracker)) = &mut self.band_analysis {
                let bands = filter.process([
                    if finite[0] { derived[0] } else { 0.0 },
                    if finite[1] { derived[1] } else { 0.0 },
                ]);
                tracker.process(std::array::from_fn(|channel| {
                    if finite[channel] {
                        bands.map(|band| WAVEFORM_CHANNELS[channel].project(band))
                    } else {
                        [0.0; BAND_COUNT]
                    }
                }));
            }
            self.ingest_derived(derived, finite, step);
        }
    }

    fn ingest_derived(
        &mut self,
        derived: [f32; DERIVED_CHANNELS],
        finite: [bool; DERIVED_CHANNELS],
        step: f64,
    ) {
        for channel in 0..DERIVED_CHANNELS {
            if finite[channel] {
                let sample = derived[channel];
                self.current[channel] = Some(match self.current[channel] {
                    Some((min, max, _)) => (min.min(sample), max.max(sample), Some(sample)),
                    None => (sample, sample, Some(sample)),
                });
            } else {
                if let Some((_, _, last)) = &mut self.current[channel] {
                    *last = None;
                }
                self.last_sample[channel] = None;
            }
        }
        self.column_phase += step;
        if self.column_phase >= 1.0 {
            self.emit_column();
            self.column_phase -= 1.0;
        }
    }

    fn cap_pending_columns(&mut self) {
        let columns = &mut self.pending_columns;
        columns.drain(..columns.len().saturating_sub(self.config.max_columns));
    }

    fn preview(&self) -> WaveformPreview {
        let progress = self.column_phase as f32;
        WaveformPreview {
            progress,
            columns: (progress > 0.0).then(|| self.columns()),
        }
    }

    pub fn process_block(&mut self, block: &AudioBlock<'_>) -> WaveformUpdate<'_> {
        self.pending_columns.clear();

        if self.config.sample_rate != block.sample_rate {
            self.update_config(WaveformConfig {
                sample_rate: block.sample_rate,
                ..self.config
            });
        }

        self.prepare();
        self.ingest_samples(block);
        if let Some((filter, _)) = &mut self.band_analysis {
            filter.flush_denormals();
        }

        self.cap_pending_columns();
        let reset = std::mem::take(&mut self.reset_pending);
        let preview = self.preview();
        WaveformUpdate {
            reset,
            columns: &self.pending_columns,
            preview,
        }
    }

    pub fn update_config(&mut self, config: WaveformConfig) {
        let rebuild = self.config.sample_rate != config.sample_rate;
        let reset_analysis = self.config.bands != config.bands;
        let resize = color_window(self.config) != color_window(config);
        self.config = config;
        if rebuild {
            self.reset_audio();
        } else if reset_analysis && self.band_analysis.is_some() {
            self.band_analysis = Self::band_analysis(self.config);
        } else if resize && let Some((_, tracker)) = &mut self.band_analysis {
            tracker.resize(self.config);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::audio::sine_wave;

    const SAMPLE_RATE: SampleRate = SampleRate::DEFAULT;
    const RATE: f32 = SAMPLE_RATE.hz();

    fn config(scroll_speed: f32, max_columns: usize) -> WaveformConfig {
        WaveformConfig {
            sample_rate: SAMPLE_RATE,
            scroll_speed: ScrollSpeed::must(scroll_speed),
            max_columns,
            ..Default::default()
        }
    }

    fn process<'a>(
        processor: &'a mut WaveformProcessor,
        samples: &[f32],
        channels: usize,
    ) -> WaveformUpdate<'a> {
        processor.process_block(&AudioBlock::fallback(samples, channels, SAMPLE_RATE))
    }

    fn extrema(update: &WaveformUpdate, channel: usize, col: usize) -> (f32, f32) {
        let column = update.columns[col][channel];
        (column.min, column.max)
    }

    fn latest(update: &WaveformUpdate, channel: usize) -> WaveColumn {
        update.columns[update.columns.len() - 1][channel]
    }

    #[test]
    fn derived_band_outputs_match_independent_mono_processing() {
        let samples: Vec<_> = sine_wave(137.0, RATE, RATE as usize, 1.0)
            .into_iter()
            .zip(sine_wave(263.0, RATE, RATE as usize, 1.0))
            .flat_map(|(left, right)| [left, right])
            .collect();
        let config = WaveformConfig {
            bands: Bands::History,
            ..config(200.0, 512)
        };
        let mut processor = WaveformProcessor::new(config);
        let mut actual = Vec::new();
        for chunk in samples.chunks(2048) {
            actual.extend_from_slice(process(&mut processor, chunk, 2).columns);
        }
        let values = |column: WaveColumn| {
            column
                .color_bands
                .into_iter()
                .chain(column.rms_db.into_iter().flatten())
        };
        for (channel, source) in WAVEFORM_CHANNELS.into_iter().enumerate() {
            let mono: Vec<_> = AudioBlock::fallback(&samples, 2, SAMPLE_RATE)
                .projected_frames(source)
                .collect();
            let mut reference = WaveformProcessor::new(config);
            let expected = process(&mut reference, &mono, 1);
            assert_eq!(actual.len(), expected.columns.len());
            for (actual, expected) in actual.iter().zip(expected.columns) {
                for (actual, expected) in values(actual[channel]).zip(values(expected[0])) {
                    assert!(
                        (actual - expected).abs() < 1.0e-4,
                        "channel {channel}: {actual} vs {expected}"
                    );
                }
            }
        }
    }

    #[test]
    fn channel_projection_feeds_extrema() {
        let mut processor = WaveformProcessor::new(config(RATE / 2.0, 8));
        assert!(processor.band_analysis.is_none());
        let update = process(&mut processor, &[1.0, 0.0, 0.0, 1.0], 2);
        assert_eq!(extrema(&update, 2, 0), (0.5, 0.5));
        assert_eq!(extrema(&update, 3, 0), (-0.5, 0.5));

        let mut processor = WaveformProcessor::new(config(RATE / 2.0, 8));
        let update = process(&mut processor, &[0.25, -0.5], 1);
        for channel in 0..3 {
            assert_eq!(extrema(&update, channel, 0), (-0.5, 0.25));
        }
        assert_eq!(extrema(&update, 3, 0), (0.0, 0.0));

        let mut processor = WaveformProcessor::new(config(RATE / 2.0, 8));
        let update = process(&mut processor, &[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0], 4);
        assert_eq!(
            extrema(&update, 2, 0),
            (0.5, 0.5 + std::f32::consts::FRAC_1_SQRT_2)
        );
    }

    #[test]
    fn previous_sample_continuity_catches_column_boundary_steps() {
        let mut processor = WaveformProcessor::new(config(RATE / 2.0, 8));
        let update = process(&mut processor, &[0.0, 0.0, 1.0, 1.0], 1);

        assert_eq!(update.columns.len(), 2);
        assert_eq!(extrema(&update, 0, 1), (0.0, 1.0));
    }

    #[test]
    fn non_finite_samples_are_sanitized_and_break_column_continuity() {
        let mut processor = WaveformProcessor::new(config(RATE, 8));
        let update = process(&mut processor, &[0.0, f32::NAN, f32::INFINITY, 1.0], 1);

        assert_eq!(update.columns.len(), 4);
        assert_eq!(extrema(&update, 0, 3), (1.0, 1.0));
        assert!(
            update
                .columns
                .iter()
                .flatten()
                .all(|c| c.min.is_finite() && c.max.is_finite())
        );
        assert!(
            update
                .columns
                .iter()
                .flatten()
                .flat_map(|c| c.color_bands)
                .all(f32::is_finite)
        );

        for (right, channel) in [(f32::MAX, 2), (-f32::MAX, 3)] {
            let mut processor = WaveformProcessor::new(config(RATE, 8));
            let update = process(&mut processor, &[f32::MAX, right].repeat(128), 2);
            for frame in update.columns {
                assert!(frame.iter().all(|c| c.min.is_finite() && c.max.is_finite()));
                assert_eq!(
                    (frame[channel].min, frame[channel].max),
                    (f32::MAX, f32::MAX)
                );
                assert_eq!(frame[channel], frame[0]);
            }
        }
    }

    #[test]
    fn disabled_band_analysis_emits_zero_band_data() {
        let mut processor = WaveformProcessor::new(config(RATE, 128));
        let _ = process(&mut processor, &[1.0; 32], 1);

        let mut updated = processor.config();
        updated.bands = Bands::Off;
        processor.update_config(updated);
        let latest = latest(&process(&mut processor, &[0.0], 1), 0);

        assert_eq!(latest.color_bands, [0.0; BAND_COUNT]);
        assert_eq!(latest.rms_db, [[DB_FLOOR; BAND_COUNT]; 2]);
    }

    #[test]
    fn bands_follow_sine_frequency() {
        fn latest_bands_for(freq: f32) -> [f32; BAND_COUNT] {
            let mut processor = WaveformProcessor::new(config(200.0, 512));
            let samples = sine_wave(freq, RATE, RATE as usize, 0.8);
            latest(&process(&mut processor, &samples, 1), 0).color_bands
        }

        let low = latest_bands_for(80.0);
        let mid = latest_bands_for(500.0);
        let high = latest_bands_for(5_000.0);

        assert!(low[0] > low[1] && low[0] > low[2], "low bands: {low:?}");
        assert!(mid[1] > mid[0] && mid[1] > mid[2], "mid bands: {mid:?}");
        assert!(
            high[2] > high[0] && high[2] > high[1],
            "high bands: {high:?}"
        );
    }

    #[test]
    fn fast_rms_reacts_before_slow_rms() {
        let mut cfg = config(100.0, 512);
        cfg.bands = Bands::History;
        let mut processor = WaveformProcessor::new(cfg);
        let mut samples = vec![0.0; RATE as usize];
        samples.extend(vec![1.0; BAND_COLOR_WINDOW_AT_44K1]);
        let latest = latest(&process(&mut processor, &samples, 1), 0);

        assert!(latest.rms_db[0][0] > latest.rms_db[1][0]);
    }

    #[test]
    fn rms_history_returns_to_floor_after_silence() {
        let mut cfg = config(300.0, 1024);
        cfg.bands = Bands::History;
        let mut processor = WaveformProcessor::new(cfg);
        let signal = sine_wave(80.0, RATE, RATE as usize, 1.0);
        let silence = vec![0.0; RATE as usize];

        let _ = process(&mut processor, &signal, 1);
        let latest = latest(&process(&mut processor, &silence, 1), 0);

        assert_eq!(latest.rms_db, [[DB_FLOOR; BAND_COUNT]; 2]);
    }

    #[test]
    fn fractional_timing_matches_requested_average_speed() {
        let mut processor = WaveformProcessor::new(WaveformConfig {
            sample_rate: SampleRate::must(1_000),
            scroll_speed: ScrollSpeed::must(333.0),
            max_columns: 4_000,
            ..Default::default()
        });
        let samples = vec![0.0; 10_000];
        let update = processor.process_block(&AudioBlock::fallback(
            &samples,
            1,
            processor.config().sample_rate,
        ));

        assert!((update.columns.len() as isize - 3330).abs() <= 1);
        assert!(
            processor.column_phase.abs() < 1.0e-8,
            "timing phase drifted to {}",
            processor.column_phase
        );
    }

    #[test]
    fn update_payload_is_capped_to_configured_history() {
        let mut processor = WaveformProcessor::new(config(RATE, 4));
        let update = process(&mut processor, &[0.1, 0.2, 0.3, 0.4, 0.5], 1);

        assert_eq!(update.columns.len(), 4);
        assert_eq!(
            (0..4).map(|i| extrema(&update, 0, i).1).collect::<Vec<_>>(),
            [0.2, 0.3, 0.4, 0.5]
        );
    }
}
