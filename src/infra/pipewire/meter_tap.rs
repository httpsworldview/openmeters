// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::virtual_sink::{self, CaptureBuffer};
use async_channel::{Receiver as AsyncReceiver, Sender as AsyncSender};
use std::sync::{Arc, LazyLock};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{info, warn};

const CHANNEL_CAPACITY: usize = 64;
const POLL_BACKOFF: Duration = Duration::from_millis(50);
const TARGET_BATCH_SAMPLES: usize = 2_048;
const MAX_BATCH_LATENCY: Duration = Duration::from_millis(25);
const DROP_CHECK_INTERVAL: Duration = Duration::from_secs(5);

static AUDIO_STREAM: LazyLock<Arc<AsyncReceiver<AudioBatch>>> = LazyLock::new(|| {
    let (sender, receiver) = async_channel::bounded(CHANNEL_CAPACITY);
    spawn_forwarder(sender, virtual_sink::capture_buffer_handle());
    Arc::new(receiver)
});

#[derive(Debug, Clone)]
pub struct AudioBatch {
    pub samples: Vec<f32>,
    pub format: MeterFormat,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeterFormat {
    pub channels: usize,
    pub sample_rate: f32,
}

struct SampleBatcher {
    target_samples: usize,
    samples: Vec<f32>,
    format: Option<MeterFormat>,
}

impl SampleBatcher {
    fn new(target_samples: usize) -> Self {
        Self {
            target_samples,
            samples: Vec::with_capacity(target_samples),
            format: None,
        }
    }

    fn push(&mut self, samples: &[f32], format: MeterFormat) {
        if samples.is_empty() {
            return;
        }
        if self.samples.is_empty() {
            self.format = Some(format);
        }
        self.samples.extend_from_slice(samples);
    }

    fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    fn should_flush(&self) -> bool {
        self.samples.len() >= self.target_samples
    }

    fn has_different_format(&self, format: MeterFormat) -> bool {
        self.format.is_some_and(|f| f != format)
    }

    fn take(&mut self) -> Option<AudioBatch> {
        if self.samples.is_empty() {
            return None;
        }
        let format = self.format.take()?;
        let max_capacity = self.target_samples.saturating_mul(4);
        let next_capacity = self.samples.len().clamp(self.target_samples, max_capacity);
        let samples = std::mem::replace(&mut self.samples, Vec::with_capacity(next_capacity));
        Some(AudioBatch { samples, format })
    }
}

pub fn audio_sample_stream() -> Arc<AsyncReceiver<AudioBatch>> {
    Arc::clone(&AUDIO_STREAM)
}

fn spawn_forwarder(sender: AsyncSender<AudioBatch>, buffer: Arc<CaptureBuffer>) {
    if let Err(err) = thread::Builder::new()
        .name("openmeters-audio-meter-tap".into())
        .spawn(move || forward_loop(sender, buffer))
    {
        tracing::error!("[meter-tap] failed to spawn forwarder thread: {err}");
    }
}

fn forward_loop(sender: AsyncSender<AudioBatch>, buffer: Arc<CaptureBuffer>) {
    let mut batcher = SampleBatcher::new(TARGET_BATCH_SAMPLES);
    let mut batch_started_at = Instant::now();
    let mut last_drop_check = Instant::now();
    let mut drop_baseline = buffer.dropped_frames();

    let flush = |batcher: &mut SampleBatcher, batch_started_at: &mut Instant| -> bool {
        let Some(batch) = batcher.take() else {
            return false;
        };
        let closed = sender.send_blocking(batch).is_err();
        *batch_started_at = Instant::now();
        closed
    };

    loop {
        buffer.grow_recycle_pool();

        if last_drop_check.elapsed() >= DROP_CHECK_INTERVAL {
            let dropped = buffer.dropped_frames();
            if dropped > drop_baseline {
                warn!(
                    "[meter-tap] dropped {} capture frames (total {})",
                    dropped - drop_baseline,
                    dropped
                );
                drop_baseline = dropped;
            }
            last_drop_check = Instant::now();
        }

        let timeout = if batcher.is_empty() {
            POLL_BACKOFF
        } else {
            MAX_BATCH_LATENCY
                .saturating_sub(batch_started_at.elapsed())
                .min(POLL_BACKOFF)
        };

        match buffer.pop_wait_timeout(timeout) {
            Some(packet) => {
                let format = MeterFormat {
                    channels: packet.channels.max(1) as usize,
                    sample_rate: packet.sample_rate.max(1) as f32,
                };

                let batch_expired =
                    !batcher.is_empty() && batch_started_at.elapsed() >= MAX_BATCH_LATENCY;
                if (batch_expired || batcher.has_different_format(format))
                    && flush(&mut batcher, &mut batch_started_at)
                {
                    buffer.recycle_samples_blocking(packet.samples);
                    break;
                }

                let starts_batch = batcher.is_empty();
                batcher.push(&packet.samples, format);
                if starts_batch {
                    batch_started_at = Instant::now();
                }
                buffer.recycle_samples_blocking(packet.samples);

                if (batcher.should_flush() || batch_started_at.elapsed() >= MAX_BATCH_LATENCY)
                    && flush(&mut batcher, &mut batch_started_at)
                {
                    break;
                }
            }
            None if sender.is_closed() => break,
            None if !batcher.is_empty()
                && batch_started_at.elapsed() >= MAX_BATCH_LATENCY
                && flush(&mut batcher, &mut batch_started_at) =>
            {
                break;
            }
            None => {}
        }
    }

    if let Some(batch) = batcher.take() {
        let _ = sender.send_blocking(batch);
    }
    info!(
        "[meter-tap] audio channel closed; {} dropped capture frames",
        buffer.dropped_frames()
    );
}

#[cfg(test)]
mod tests {
    use super::{MeterFormat, SampleBatcher};

    const STEREO_48K: MeterFormat = MeterFormat {
        channels: 2,
        sample_rate: 48_000.0,
    };
    const MONO_44K: MeterFormat = MeterFormat {
        channels: 1,
        sample_rate: 44_100.0,
    };

    #[test]
    fn batches_chunks() {
        let mut batcher = SampleBatcher::new(4);
        batcher.push(&[0.0, 1.0], STEREO_48K);
        assert!(!batcher.should_flush());
        batcher.push(&[2.0, 3.0], STEREO_48K);
        assert!(batcher.should_flush());

        let batch = batcher.take().expect("batch should be available");
        assert_eq!(batch.samples, vec![0.0, 1.0, 2.0, 3.0]);
        assert_eq!(batch.format, STEREO_48K);
        assert!(batcher.take().is_none());

        batcher.push(&[4.0, 5.0], STEREO_48K);
        batcher.push(&[6.0, 7.0], STEREO_48K);
        let second = batcher.take().expect("second batch available");
        assert_eq!(second.samples, vec![4.0, 5.0, 6.0, 7.0]);
        assert_eq!(second.format, STEREO_48K);
    }

    #[test]
    fn format_changes_flush_without_mixing_batches() {
        let mut batcher = SampleBatcher::new(8);

        batcher.push(&[0.0, 1.0], STEREO_48K);
        assert!(!batcher.has_different_format(STEREO_48K));
        assert!(batcher.has_different_format(MONO_44K));

        let first = batcher.take().expect("old-format batch should flush");
        assert_eq!(first.samples, vec![0.0, 1.0]);
        assert_eq!(first.format, STEREO_48K);

        batcher.push(&[2.0, 3.0], MONO_44K);
        let second = batcher
            .take()
            .expect("new-format batch should remain separate");
        assert_eq!(second.samples, vec![2.0, 3.0]);
        assert_eq!(second.format, MONO_44K);
    }
}
