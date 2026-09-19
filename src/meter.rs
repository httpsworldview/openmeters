// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

//! Feeds audio and silence to visuals in timeline order.
//!
//! Capture stays at its negotiated PipeWire quantum. DSP work is amortized into
//! sample-rate-scaled batches so compositor cadence does not become DSP cadence.

use crate::dsp::AudioFormat;
use crate::infra::pipewire::{AudioReader, AudioWake, CapturedSpan, MAX_CAPTURE_CHANNELS};
use crate::util::audio::DEFAULT_SAMPLE_RATE;
use crate::visuals::registry::{VisualManager, VisualManagerHandle};
use std::time::Instant;

const SILENCE_CHUNK_FRAMES: usize = 4_096;
const DSP_BATCH_FRAMES_AT_48K: usize = 256;
const MAX_DSP_INGEST_FRAMES_AT_48K: usize = 1_024;
// Per-span limit; longer silence resets audio state.
const MAX_SILENCE_SECONDS: u64 = 4;

fn scaled_samples(frames_at_48k: usize, format: AudioFormat) -> usize {
    ((frames_at_48k as f64 * f64::from(format.sample_rate) / f64::from(DEFAULT_SAMPLE_RATE))
        .round()
        .max(1.0) as usize)
        * format.channels
}

struct DspBatcher {
    samples: Vec<f32>,
    format: Option<AudioFormat>,
}

impl DspBatcher {
    fn new() -> Self {
        Self {
            samples: Vec::with_capacity(DSP_BATCH_FRAMES_AT_48K * MAX_CAPTURE_CHANNELS),
            format: None,
        }
    }

    fn push(&mut self, manager: &mut VisualManager, mut samples: &[f32], format: AudioFormat) {
        if self.format.is_some_and(|current| current != format) {
            self.samples.clear();
        }
        self.format = Some(format);
        let batch = scaled_samples(DSP_BATCH_FRAMES_AT_48K, format);
        if !self.samples.is_empty() {
            let take = (batch - self.samples.len()).min(samples.len());
            self.samples.extend_from_slice(&samples[..take]);
            samples = &samples[take..];
            if self.samples.len() == batch {
                manager.ingest_samples(&self.samples, format);
                self.samples.clear();
            }
        }
        let ready = samples.len() / batch * batch;
        for chunk in samples[..ready].chunks(scaled_samples(MAX_DSP_INGEST_FRAMES_AT_48K, format)) {
            manager.ingest_samples(chunk, format);
        }
        self.samples.extend_from_slice(&samples[ready..]);
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

    pub fn advance(&mut self, now: Instant, allow_quiescence: bool) -> bool {
        if !self.active || self.paused {
            return false;
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
                ingest_silence(&mut manager, silence, batcher, frames, format);
            }
            CapturedSpan::Reset => batcher.reset(&mut manager),
        });
        let quiescent = allow_quiescence
            && manager.is_quiescent()
            && batcher.samples.iter().all(|&sample| sample == 0.0);
        drop(manager);
        quiescent && audio.sleep_until_signal()
    }

    pub fn wake(&mut self) -> bool {
        let active = self.active && !self.paused;
        if active && self.audio.set_active(true) {
            self.batcher.clear();
        }
        active
    }

    pub fn wake_handle(&self) -> AudioWake {
        self.audio.wake_handle()
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

fn ingest_silence(
    manager: &mut VisualManager,
    scratch: &[f32],
    batcher: &mut DspBatcher,
    frames: u64,
    format: AudioFormat,
) {
    let limit = (MAX_SILENCE_SECONDS as f64 * f64::from(format.sample_rate))
        .round()
        .max(1.0) as u64;
    if frames > limit {
        batcher.reset(manager);
        return;
    }
    let capacity = scratch.len() / format.channels;
    let mut remaining = frames;
    while remaining > 0 {
        let chunk = remaining.min(capacity as u64) as usize;
        batcher.push(manager, &scratch[..chunk * format.channels], format);
        remaining -= chunk as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::{AudioBlock, ChannelPosition};
    use crate::util::audio::sine_wave;
    use crate::visuals::loudness::processor::{LoudnessProcessor, LoudnessSnapshot};
    use crate::visuals::registry::{VisualContent, VisualKind};

    fn loudness() -> (VisualManager, impl Fn() -> LoudnessSnapshot) {
        let mut manager = VisualManager::default();
        manager.set_enabled(VisualKind::Loudness, true);
        let state = manager
            .snapshot()
            .into_iter()
            .find_map(|entry| match entry.content {
                VisualContent::Loudness(state) => Some(state),
                _ => None,
            })
            .unwrap();
        (manager, move || state.borrow().snapshot())
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
    fn batches_deliver_audio_without_mixing_generations_or_reallocating() {
        let (mut manager, snapshot) = loudness();
        let mut batcher = DspBatcher::new();
        let storage = (batcher.samples.as_ptr(), batcher.samples.capacity());
        for (rate, generation) in [(48_000, 1), (48_000, 2), (96_000, 3)] {
            let format = AudioFormat::new(2, rate, generation, ChannelPosition::fallback(2));
            let batch = rate as usize / 48_000 * 256 * 2;
            let samples: Vec<_> = sine_wave(1_000.0, rate as f32, batch, 0.5)
                .into_iter()
                .flat_map(|sample| [sample, -sample * 0.5])
                .collect();
            let mut reference = LoudnessProcessor::new(Default::default());
            let before = snapshot();
            batcher.push(&mut manager, &samples[..batch - 2], format);
            assert_eq!(snapshot(), before);
            for (start, end) in [(batch - 2, batch), (batch, batch * 2)] {
                batcher.push(&mut manager, &samples[start..end], format);
                assert_eq!(
                    snapshot(),
                    reference.process_block(&AudioBlock::new(
                        &samples[end - batch..end],
                        2,
                        rate as f32,
                    ))
                );
            }
            assert!(batcher.samples.is_empty());
            batcher.push(&mut manager, &[9.0; 2], format);
            assert_eq!(
                (batcher.samples.as_ptr(), batcher.samples.capacity()),
                storage
            );
        }
    }

    #[test]
    fn silence_preserves_loudness_history_before_the_hard_reset() {
        let (mut manager, snapshot) = loudness();
        let mut batcher = DspBatcher::new();
        let format = AudioFormat::new(1, 48_000, 1, ChannelPosition::fallback(1));
        let scratch = [0.0; SILENCE_CHUNK_FRAMES * MAX_CAPTURE_CHANNELS];
        let signal = sine_wave(1_000.0, 48_000.0, 256, 0.5);
        let reset_level = snapshot().short_term_loudness;
        for frames in [48_000, 144_001, 192_000, 192_001] {
            batcher.reset(&mut manager);
            batcher.push(&mut manager, &signal, format);
            assert!(snapshot().short_term_loudness > reset_level);
            let mut reference = LoudnessProcessor::new(Default::default());
            reference.process_block(&AudioBlock::new(&signal, 1, 48_000.0));
            batcher.push(&mut manager, &[0.0], format);
            ingest_silence(&mut manager, &scratch, &mut batcher, frames, format);
            let expected = if frames > 192_000 {
                assert!(batcher.samples.is_empty());
                reset_level
            } else {
                reference
                    .process_block(&AudioBlock::new(
                        &vec![0.0; (frames as usize + 1) / 256 * 256],
                        1,
                        48_000.0,
                    ))
                    .short_term_loudness
            };
            assert_eq!(snapshot().short_term_loudness, expected, "frames={frames}");
            assert_eq!(batcher.format.is_none(), frames > 192_000);
        }
    }
}
