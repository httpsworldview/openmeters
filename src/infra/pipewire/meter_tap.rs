// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::virtual_sink::{self, CaptureBuffer};
use async_channel::{Receiver as AsyncReceiver, Sender as AsyncSender};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

const CHANNEL_CAPACITY: usize = 64;
const POLL_BACKOFF: Duration = Duration::from_millis(50);
const TARGET_BATCH_SAMPLES: usize = 2_048;
const MAX_BATCH_LATENCY: Duration = Duration::from_millis(25);
const DROP_CHECK_INTERVAL: Duration = Duration::from_secs(5);

static AUDIO_STREAM: OnceLock<Arc<AsyncReceiver<AudioBatch>>> = OnceLock::new();

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

const MAX_RECYCLED_BUFFERS: usize = 4;
const MAX_RECYCLED_BUFFER_SAMPLES: usize = TARGET_BATCH_SAMPLES * 4;

#[derive(Default)]
struct SampleBatcher {
    target_samples: usize,
    total_samples: usize,
    chunks: Vec<Vec<f32>>,
    recycle: Vec<Vec<f32>>,
    format: Option<MeterFormat>,
}

impl SampleBatcher {
    fn new(target_samples: usize) -> Self {
        Self {
            target_samples,
            ..Self::default()
        }
    }

    fn push(&mut self, chunk: Vec<f32>, format: MeterFormat) {
        if chunk.is_empty() {
            return;
        }
        if self.total_samples == 0 {
            self.format = Some(format);
        }
        self.total_samples += chunk.len();
        self.chunks.push(chunk);
    }

    fn should_flush(&self) -> bool {
        self.total_samples >= self.target_samples
    }

    fn has_different_format(&self, format: MeterFormat) -> bool {
        self.format.is_some_and(|f| f != format)
    }

    fn take(&mut self) -> Option<AudioBatch> {
        if self.total_samples == 0 {
            return None;
        }

        let total_samples = std::mem::take(&mut self.total_samples);
        let format = self.format.take().expect("batch format set");
        let samples = if self.chunks.len() == 1 {
            self.chunks.pop().expect("single chunk present")
        } else {
            let mut batch = self.reuse_buffer(total_samples);
            for chunk in self.chunks.drain(..) {
                batch.extend_from_slice(&chunk);
                Self::stash_recycle(&mut self.recycle, chunk);
            }
            batch
        };

        Some(AudioBatch { samples, format })
    }

    fn reuse_buffer(&mut self, needed: usize) -> Vec<f32> {
        let Some(mut recycled) = self.recycle.pop() else {
            return Vec::with_capacity(needed);
        };
        recycled.clear();
        recycled.reserve(needed.saturating_sub(recycled.capacity()));
        recycled
    }

    fn stash_recycle(recycle: &mut Vec<Vec<f32>>, mut chunk: Vec<f32>) {
        if recycle.len() < MAX_RECYCLED_BUFFERS && chunk.capacity() <= MAX_RECYCLED_BUFFER_SAMPLES {
            chunk.clear();
            recycle.push(chunk);
        }
    }
}

pub fn audio_sample_stream() -> Arc<AsyncReceiver<AudioBatch>> {
    AUDIO_STREAM
        .get_or_init(|| {
            let (sender, receiver) = async_channel::bounded(CHANNEL_CAPACITY);
            spawn_forwarder(sender, virtual_sink::capture_buffer_handle());
            Arc::new(receiver)
        })
        .clone()
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
    let mut last_flush = Instant::now();
    let mut last_drop_check = Instant::now();
    let mut drop_baseline = buffer.dropped_frames();

    // Returns true when the downstream channel is closed (caller should stop).
    let flush = |batcher: &mut SampleBatcher, last_flush: &mut Instant| -> bool {
        let closed = batcher
            .take()
            .is_some_and(|b| sender.send_blocking(b).is_err());
        *last_flush = Instant::now();
        closed
    };

    loop {
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

        let should_time_flush = last_flush.elapsed() >= MAX_BATCH_LATENCY;

        match buffer.pop_wait_timeout(POLL_BACKOFF) {
            Ok(Some(packet)) => {
                let format = MeterFormat {
                    channels: packet.channels.max(1) as usize,
                    sample_rate: packet.sample_rate.max(1) as f32,
                };

                if batcher.has_different_format(format) && flush(&mut batcher, &mut last_flush) {
                    break;
                }

                batcher.push(packet.samples, format);

                if (batcher.should_flush() || should_time_flush)
                    && flush(&mut batcher, &mut last_flush)
                {
                    break;
                }
            }
            Ok(None) if sender.is_closed() => break,
            Ok(None) if should_time_flush => {
                if flush(&mut batcher, &mut last_flush) {
                    break;
                }
            }
            Ok(None) => {}
            Err(_) => {
                error!("[meter-tap] capture buffer unavailable; stopping tap");
                break;
            }
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
    fn batches_and_reuses_buffers() {
        let mut batcher = SampleBatcher::new(4);
        batcher.push(vec![0.0, 1.0], STEREO_48K);
        assert!(!batcher.should_flush());
        batcher.push(vec![2.0, 3.0], STEREO_48K);
        assert!(batcher.should_flush());

        let batch = batcher.take().expect("batch should be available");
        assert_eq!(batch.samples, vec![0.0, 1.0, 2.0, 3.0]);
        assert_eq!(batch.format, STEREO_48K);
        assert!(batcher.take().is_none());

        batcher.push(vec![4.0, 5.0], STEREO_48K);
        batcher.push(vec![6.0, 7.0], STEREO_48K);
        let second = batcher.take().expect("second batch available");
        assert_eq!(second.samples, vec![4.0, 5.0, 6.0, 7.0]);
        assert_eq!(second.format, STEREO_48K);
    }

    #[test]
    fn format_changes_flush_without_mixing_batches() {
        let mut batcher = SampleBatcher::new(8);

        batcher.push(vec![0.0, 1.0], STEREO_48K);
        assert!(!batcher.has_different_format(STEREO_48K));
        assert!(batcher.has_different_format(MONO_44K));

        let first = batcher.take().expect("old-format batch should flush");
        assert_eq!(first.samples, vec![0.0, 1.0]);
        assert_eq!(first.format, STEREO_48K);

        batcher.push(vec![2.0, 3.0], MONO_44K);
        let second = batcher
            .take()
            .expect("new-format batch should remain separate");
        assert_eq!(second.samples, vec![2.0, 3.0]);
        assert_eq!(second.format, MONO_44K);
    }
}
