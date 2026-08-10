// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

//! Presentation-side ownership of the ordered audio timeline.
//!
//! Capture stays at its negotiated PipeWire quantum. DSP work is amortized into
//! sample-rate-scaled batches so compositor cadence does not become DSP cadence.

use crate::dsp::AudioFormat;
use crate::infra::pipewire::{AudioReader, CapturedSpan, MAX_CAPTURE_CHANNELS};
use crate::util::audio::DEFAULT_SAMPLE_RATE;
use crate::visuals::registry::{VisualManager, VisualManagerHandle};
use std::time::Instant;

const SILENCE_CHUNK_FRAMES: usize = 4_096;
const DSP_BATCH_FRAMES_AT_48K: usize = 256;
const MAX_DSP_INGEST_FRAMES_AT_48K: usize = 1_024;
const MAX_SILENCE_SECONDS: u64 = 10;

fn scaled_samples(frames_at_48k: usize, format: AudioFormat) -> usize {
    ((frames_at_48k as f64 * f64::from(format.sample_rate) / f64::from(DEFAULT_SAMPLE_RATE))
        .round()
        .max(1.0) as usize)
        * format.channels.max(1)
}

struct DspBatcher {
    samples: Vec<f32>,
    accum_silence: u64,
    format: Option<AudioFormat>,
}

impl DspBatcher {
    fn new() -> Self {
        Self {
            samples: Vec::with_capacity(DSP_BATCH_FRAMES_AT_48K * MAX_CAPTURE_CHANNELS),
            accum_silence: 0,
            format: None,
        }
    }

    fn push_silence(
        &mut self,
        manager: &mut VisualManager,
        silence: &[f32],
        frames: u64,
        format: AudioFormat,
    ) {
        self.accum_silence = self.accum_silence.saturating_add(frames);

        let limit = (MAX_SILENCE_SECONDS as f64 * f64::from(format.sample_rate))
            .round()
            .max(1.0) as u64;
        if self.accum_silence > limit {
            self.reset(manager);
            return;
        }

        let accum_silence = self.accum_silence;

        let capacity = silence.len() / format.channels.max(1);
        let mut remaining = frames;
        while remaining > 0 {
            let chunk = remaining.min(capacity as u64) as usize;
            self.push(manager, &silence[..chunk * format.channels], format);
            remaining -= chunk as u64;
        }

        self.accum_silence = accum_silence;
    }

    fn push(
        &mut self,
        manager: &mut VisualManager,
        mut samples: &[f32],
        format: AudioFormat,
    ) -> usize {
        self.accum_silence = 0;
        if self.format.is_some_and(|current| current != format) {
            self.samples.clear();
        }
        self.format = Some(format);
        let batch = scaled_samples(DSP_BATCH_FRAMES_AT_48K, format);
        let mut count = 0;
        if !self.samples.is_empty() {
            let take = (batch - self.samples.len()).min(samples.len());
            self.samples.extend_from_slice(&samples[..take]);
            samples = &samples[take..];
            if self.samples.len() == batch {
                manager.ingest_samples(&self.samples, format);
                self.samples.clear();
                count += 1;
            }
        }
        let ready = samples.len() / batch * batch;
        for chunk in samples[..ready].chunks(scaled_samples(MAX_DSP_INGEST_FRAMES_AT_48K, format)) {
            manager.ingest_samples(chunk, format);
            count += 1;
        }
        self.samples.extend_from_slice(&samples[ready..]);
        count
    }

    fn reset(&mut self, manager: &mut VisualManager) {
        self.clear();
        manager.reset_audio();
    }

    fn clear(&mut self) {
        self.samples.clear();
        self.format = None;
    }
}

pub(crate) struct MeterEngine {
    audio: AudioReader,
    visuals: VisualManagerHandle,
    silence: Vec<f32>,
    batcher: DspBatcher,
    active: bool,
    paused: bool,
}

impl MeterEngine {
    pub fn new(audio: AudioReader, visuals: VisualManagerHandle) -> Self {
        Self {
            audio,
            visuals,
            silence: vec![0.0; SILENCE_CHUNK_FRAMES * MAX_CAPTURE_CHANNELS],
            batcher: DspBatcher::new(),
            active: true,
            paused: false,
        }
    }

    pub fn advance(&mut self, now: Instant) {
        if !self.active || self.paused {
            return;
        }
        let Self {
            audio,
            visuals,
            silence,
            batcher,
            ..
        } = self;
        let mut manager = visuals.borrow_mut();
        audio.drain(now, |span| match span {
            CapturedSpan::Pcm { samples, format } => {
                batcher.push(&mut manager, samples, format);
            }
            CapturedSpan::Silence { frames, format } => {
                batcher.push_silence(&mut manager, silence, frames, format);
            }
            CapturedSpan::Reset => batcher.reset(&mut manager),
        });
    }

    pub fn set_active(&mut self, active: bool) {
        if std::mem::replace(&mut self.active, active) == active {
            return;
        }
        self.audio.set_active(active && !self.paused);
        self.batcher.clear();
    }

    pub fn set_paused(&mut self, paused: bool, now: Instant) {
        if std::mem::replace(&mut self.paused, paused) == paused {
            return;
        }
        if !self.audio.set_active(self.active && !paused) {
            self.audio.discard(now);
        }
        self.batcher.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::ChannelPosition;

    fn format(channels: usize, sample_rate: f32, generation: u64) -> AudioFormat {
        AudioFormat {
            channels,
            sample_rate,
            generation,
            positions: ChannelPosition::fallback(channels),
        }
    }

    #[test]
    fn pause_gates_capture_at_the_producer() {
        let visuals = std::rc::Rc::new(std::cell::RefCell::new(VisualManager::default()));
        let mut meter = MeterEngine::new(crate::infra::pipewire::test_audio_reader(), visuals);
        assert!(meter.audio.is_active());
        meter.set_paused(true, Instant::now());
        assert!(!meter.audio.is_active());
        meter.set_paused(false, Instant::now());
        assert!(meter.audio.is_active());
    }

    #[test]
    fn dsp_batches_are_sample_driven_and_reuse_storage() {
        let mut manager = VisualManager::default();
        let mut batcher = DspBatcher::new();
        let format = format(2, 48_000.0, 1);
        let block = [0.25; 64 * 2];
        let storage = (batcher.samples.as_ptr(), batcher.samples.capacity());
        for index in 0..4 {
            assert_eq!(
                batcher.push(&mut manager, &block, format),
                usize::from(index == 3)
            );
        }
        assert!(batcher.samples.is_empty());
        assert_eq!(
            (batcher.samples.as_ptr(), batcher.samples.capacity()),
            storage
        );

        let high_rate = AudioFormat {
            sample_rate: 96_000.0,
            ..format
        };
        for index in 0..8 {
            assert_eq!(
                batcher.push(&mut manager, &block, high_rate),
                usize::from(index == 7)
            );
        }
        assert_eq!(
            (batcher.samples.as_ptr(), batcher.samples.capacity()),
            storage
        );
    }

    #[test]
    fn dsp_batches_coalesce_large_capture_backlogs() {
        let mut manager = VisualManager::default();
        let mut batcher = DspBatcher::new();
        let format = format(2, 48_000.0, 1);
        assert_eq!(
            batcher.push(&mut manager, &[0.25; (256 * 6 + 17) * 2], format),
            2
        );
        assert_eq!(batcher.samples.len(), 17 * 2);
        assert_eq!(batcher.push(&mut manager, &[0.25; 239 * 2], format), 1);
        assert!(batcher.samples.is_empty());
    }

    #[test]
    fn dsp_batches_never_mix_format_generations() {
        let mut manager = VisualManager::default();
        let mut batcher = DspBatcher::new();
        let old = format(2, 48_000.0, 1);
        assert_eq!(batcher.push(&mut manager, &[0.25; 128 * 2], old), 0);
        let new = AudioFormat {
            generation: 2,
            ..old
        };
        assert_eq!(batcher.push(&mut manager, &[0.5; 2], new), 0);
        assert_eq!(batcher.samples.as_slice(), &[0.5, 0.5]);
        assert_eq!(batcher.format, Some(new));
    }

    #[test]
    fn long_silence_resets_without_replaying_samples() {
        let mut manager = VisualManager::default();
        let mut batcher = DspBatcher::new();
        let format = format(MAX_CAPTURE_CHANNELS, 192_000.0, 1);
        assert_eq!(
            batcher.push(&mut manager, &[0.25; 128 * MAX_CAPTURE_CHANNELS], format),
            0
        );
        let scratch = [0.0; SILENCE_CHUNK_FRAMES * MAX_CAPTURE_CHANNELS];
        batcher.push_silence(
            &mut manager,
            &scratch,
            MAX_SILENCE_SECONDS * 192_000 + 1,
            format,
        );
        assert!(batcher.samples.is_empty());
        assert_eq!(batcher.format, None);
    }

    #[test]
    fn many_silences_reset_without_replaying_samples() {
        let mut manager = VisualManager::default();
        let mut batcher = DspBatcher::new();
        let format = format(MAX_CAPTURE_CHANNELS, 192_000.0, 1);
        assert_eq!(
            batcher.push(&mut manager, &[0.25; 128 * MAX_CAPTURE_CHANNELS], format),
            0
        );
        let scratch = [0.0; SILENCE_CHUNK_FRAMES * MAX_CAPTURE_CHANNELS];
        for _ in 0..10 {
            batcher.push_silence(
                &mut manager,
                &scratch,
                MAX_SILENCE_SECONDS * 192_000 / 10,
                format,
            );
        }
        batcher.push_silence(&mut manager, &scratch, 1, format);
        assert!(batcher.samples.is_empty());
        assert_eq!(batcher.format, None);
    }
}
