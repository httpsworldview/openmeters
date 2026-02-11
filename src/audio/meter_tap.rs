use super::pw_virtual_sink::{self, CaptureBuffer};
use crate::util::audio::DEFAULT_SAMPLE_RATE;
use async_channel::{Receiver as AsyncReceiver, Sender as AsyncSender};
use parking_lot::RwLock;
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

const CHANNEL_CAPACITY: usize = 64;
const POLL_BACKOFF: Duration = Duration::from_millis(50);
const TARGET_BATCH_SAMPLES: usize = 2_048;
const MAX_BATCH_LATENCY: Duration = Duration::from_millis(25);
const DROP_CHECK_INTERVAL: Duration = Duration::from_secs(5);

static AUDIO_STREAM: OnceLock<Arc<AsyncReceiver<Vec<f32>>>> = OnceLock::new();
static FORMAT_STATE: RwLock<MeterFormat> = RwLock::new(MeterFormat::new());

#[derive(Debug, Clone, Copy)]
pub struct MeterFormat {
    pub channels: usize,
    pub sample_rate: f32,
}

impl MeterFormat {
    const fn new() -> Self {
        Self {
            channels: 2,
            sample_rate: DEFAULT_SAMPLE_RATE,
        }
    }

    fn differs_from(&self, channels: usize, sample_rate: f32) -> bool {
        self.channels != channels || (self.sample_rate - sample_rate).abs() > f32::EPSILON
    }
}

const MAX_RECYCLED_BUFFERS: usize = 4;

#[derive(Default)]
struct SampleBatcher {
    target_samples: usize,
    total_samples: usize,
    chunks: Vec<Vec<f32>>,
    recycle: Vec<Vec<f32>>,
}

impl SampleBatcher {
    fn new(target_samples: usize) -> Self {
        Self {
            target_samples,
            ..Self::default()
        }
    }

    fn is_empty(&self) -> bool {
        self.total_samples == 0
    }

    fn push(&mut self, chunk: Vec<f32>) {
        self.total_samples = self.total_samples.saturating_add(chunk.len());
        self.chunks.push(chunk);
    }

    fn should_flush(&self) -> bool {
        self.total_samples >= self.target_samples
    }

    fn take(&mut self) -> Option<Vec<f32>> {
        if self.total_samples == 0 {
            return None;
        }

        self.total_samples = 0;

        if self.chunks.len() == 1 {
            return self.chunks.pop();
        }

        let total_samples = self.chunks.iter().map(|c| c.len()).sum();
        let mut batch = self.reuse_buffer(total_samples);

        for chunk in self.chunks.drain(..) {
            batch.extend_from_slice(&chunk);
            Self::stash_recycle(&mut self.recycle, chunk);
        }

        Some(batch)
    }

    fn reuse_buffer(&mut self, needed: usize) -> Vec<f32> {
        if let Some(mut recycled) = self.recycle.pop() {
            recycled.clear();
            if recycled.capacity() < needed {
                recycled.reserve(needed - recycled.capacity());
            }
            recycled
        } else {
            Vec::with_capacity(needed)
        }
    }

    fn stash_recycle(recycle: &mut Vec<Vec<f32>>, mut chunk: Vec<f32>) {
        if recycle.len() < MAX_RECYCLED_BUFFERS {
            chunk.clear();
            recycle.push(chunk);
        }
    }
}

pub fn current_format() -> MeterFormat {
    *FORMAT_STATE.read()
}

pub fn audio_sample_stream() -> Arc<AsyncReceiver<Vec<f32>>> {
    AUDIO_STREAM
        .get_or_init(|| {
            let (sender, receiver) = async_channel::bounded(CHANNEL_CAPACITY);
            spawn_forwarder(sender, pw_virtual_sink::capture_buffer_handle());
            Arc::new(receiver)
        })
        .clone()
}

fn spawn_forwarder(sender: AsyncSender<Vec<f32>>, buffer: Arc<CaptureBuffer>) {
    thread::Builder::new()
        .name("openmeters-audio-meter-tap".into())
        .spawn(move || forward_loop(sender, buffer))
        .expect("failed to spawn audio meter tap thread");
}

fn forward_loop(sender: AsyncSender<Vec<f32>>, buffer: Arc<CaptureBuffer>) {
    let mut batcher = SampleBatcher::new(TARGET_BATCH_SAMPLES);
    let mut last_flush = Instant::now();
    let mut last_drop_check = Instant::now();
    let mut drop_baseline = buffer.dropped_frames();

    let flush = |batcher: &mut SampleBatcher| {
        batcher
            .take()
            .is_some_and(|b| sender.send_blocking(b).is_err())
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
                let (channels, sample_rate) = (
                    packet.channels.max(1) as usize,
                    packet.sample_rate.max(1) as f32,
                );

                if !batcher.is_empty() && FORMAT_STATE.read().differs_from(channels, sample_rate) {
                    if flush(&mut batcher) {
                        break;
                    }
                    last_flush = Instant::now();
                }

                *FORMAT_STATE.write() = MeterFormat {
                    channels,
                    sample_rate,
                };

                if !packet.samples.is_empty() {
                    batcher.push(packet.samples);
                }

                if batcher.should_flush() || should_time_flush {
                    if flush(&mut batcher) {
                        break;
                    }
                    last_flush = Instant::now();
                }
            }
            Ok(None) if sender.is_closed() => break,
            Ok(None) if should_time_flush => {
                if flush(&mut batcher) {
                    break;
                }
                last_flush = Instant::now();
            }
            Ok(None) => {}
            Err(_) => {
                error!("[meter-tap] capture buffer unavailable; stopping tap");
                break;
            }
        }
    }

    let _ = batcher.take().map(|b| sender.send_blocking(b));
    info!(
        "[meter-tap] audio channel closed; {} dropped capture frames",
        buffer.dropped_frames()
    );
}

#[cfg(test)]
mod tests {
    use super::SampleBatcher;

    #[test]
    fn batches_and_reuses_buffers() {
        let mut batcher = SampleBatcher::new(4);
        batcher.push(vec![0.0, 1.0]);
        assert!(!batcher.should_flush());
        batcher.push(vec![2.0, 3.0]);
        assert!(batcher.should_flush());

        let batch = batcher.take().expect("batch should be available");
        assert_eq!(batch, vec![0.0, 1.0, 2.0, 3.0]);
        assert!(batcher.take().is_none());

        batcher.push(vec![4.0, 5.0]);
        batcher.push(vec![6.0, 7.0]);
        let second = batcher.take().expect("second batch available");
        assert_eq!(second, vec![4.0, 5.0, 6.0, 7.0]);
    }
}
