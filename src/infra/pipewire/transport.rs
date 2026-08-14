// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{MAX_CAPTURE_CHANNELS, MAX_CAPTURE_SAMPLE_RATE};
use crate::dsp::{AudioFormat, ChannelPosition};
use crate::util::{audio::DEFAULT_SAMPLE_RATE, unpoison};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tracing::info;

const BLOCK_FRAMES: usize = 256;
const BLOCK_SAMPLES: usize = BLOCK_FRAMES * MAX_CAPTURE_CHANNELS;
const MAX_BACKLOG: Duration = Duration::from_secs(1);
const RING_BLOCKS: usize = (MAX_CAPTURE_SAMPLE_RATE as usize * 4).div_ceil(BLOCK_FRAMES * 3);
const PCM_FLUSH_SAMPLES: usize = BLOCK_SAMPLES * 4;
const PACKET_FLUSH_INTERVAL: Duration = Duration::from_millis(50);
const IDLE_WATCHDOG: Duration = Duration::from_millis(100);

fn packet_frame_limit(rate: u64) -> usize {
    (u128::from(rate) * PACKET_FLUSH_INTERVAL.as_nanos() / 1_000_000_000)
        .clamp(1, BLOCK_FRAMES as u128) as usize
}

fn packet_pool_limit(rate: u64, queue_capacity: usize) -> usize {
    ((rate as usize * 4).div_ceil(packet_frame_limit(rate) * 3)).min(queue_capacity) + 1
}

fn idle_watchdog_ns(rate: u64) -> u64 {
    nanos(IDLE_WATCHDOG).max(
        frames_ns(packet_frame_limit(rate) as u64, rate)
            .saturating_add(nanos(PACKET_FLUSH_INTERVAL)),
    )
}

pub enum CapturedSpan<'a> {
    Pcm {
        samples: &'a [f32],
        format: AudioFormat,
    },
    Silence {
        frames: u64,
        format: AudioFormat,
    },
    Reset,
}

trait SpanConsumer: for<'a> FnMut(CapturedSpan<'a>) {}
impl<T: for<'a> FnMut(CapturedSpan<'a>)> SpanConsumer for T {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum StreamStatus {
    Starting,
    Paused,
    Streaming,
    Failed,
    Stopped,
}

#[derive(Clone)]
pub(crate) struct AudioWake(async_channel::Receiver<()>, usize);

impl Hash for AudioWake {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.1.hash(state);
    }
}

pub(crate) fn audio_wake(wake: &AudioWake) -> async_channel::Receiver<()> {
    wake.0.clone()
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum IngestState {
    Disabled,
    Dormant,
    Active,
}

struct Shared {
    epoch: Instant,
    status: AtomicU8,
    format: RwLock<AudioFormat>,
    fault_epoch: AtomicU64,
    activity_epoch: AtomicU64,
    ingest: AtomicU8,
    pending_signal: AtomicBool,
    wake_sender: async_channel::Sender<()>,
    wake_receiver: async_channel::Receiver<()>,
    reconnects: AtomicU64,
}

impl Shared {
    fn new() -> Self {
        let (wake_sender, wake_receiver) = async_channel::bounded(1);
        Self {
            epoch: Instant::now(),
            status: AtomicU8::new(StreamStatus::Starting as u8),
            format: RwLock::new(AudioFormat::new(
                2,
                DEFAULT_SAMPLE_RATE as u32,
                0,
                [ChannelPosition::Unknown; MAX_CAPTURE_CHANNELS],
            )),
            fault_epoch: AtomicU64::new(0),
            activity_epoch: AtomicU64::new(0),
            ingest: AtomicU8::new(IngestState::Active as u8),
            pending_signal: AtomicBool::new(false),
            wake_sender,
            wake_receiver,
            reconnects: AtomicU64::new(0),
        }
    }

    fn now_ns(&self) -> u64 {
        nanos(self.epoch.elapsed())
    }

    fn format(&self) -> AudioFormat {
        *unpoison(self.format.read())
    }

    fn fault(&self) {
        self.fault_epoch.fetch_add(1, Ordering::SeqCst);
        if self.wake_ingestion() {
            self.notify();
        }
    }

    fn notify(&self) {
        let _ = self.wake_sender.try_send(());
    }

    fn ingest(&self) -> IngestState {
        match self.ingest.load(Ordering::Acquire) {
            value if value == IngestState::Disabled as u8 => IngestState::Disabled,
            value if value == IngestState::Dormant as u8 => IngestState::Dormant,
            _ => IngestState::Active,
        }
    }

    fn wake_ingestion(&self) -> bool {
        self.ingest
            .fetch_update(Ordering::SeqCst, Ordering::Relaxed, |state| {
                (state == IngestState::Dormant as u8).then_some(IngestState::Active as u8)
            })
            .is_ok()
    }
}

fn frames_ns(frames: u64, rate: u64) -> u64 {
    scale(frames, 1_000_000_000, rate)
}

fn ns_frames(ns: u64, rate: u64) -> u64 {
    scale(ns, rate, 1_000_000_000)
}

fn ns_frames_ceil(ns: u64, rate: u64) -> u64 {
    (u128::from(ns) * u128::from(rate))
        .div_ceil(1_000_000_000)
        .min(u128::from(u64::MAX)) as u64
}

fn nanos(duration: Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
}

fn scale(value: u64, numerator: u64, denominator: u64) -> u64 {
    (u128::from(value) * u128::from(numerator) / u128::from(denominator)).min(u128::from(u64::MAX))
        as u64
}

pub(super) fn pcm_chunk(
    max: usize,
    offset: u32,
    size: u32,
    frame_bytes: usize,
) -> Option<[Range<usize>; 2]> {
    if max == 0 || frame_bytes == 0 {
        return None;
    }
    let len = (size as usize).min(max) / frame_bytes * frame_bytes;
    if len == 0 {
        return None;
    }
    let start = offset as usize % max;
    let first = len.min(max - start);
    Some([start..start + first, 0..len - first])
}

fn pcm_has_signal(bytes: &[u8], chunk: &[Range<usize>; 2]) -> bool {
    let mut source = Read::chain(&bytes[chunk[0].clone()], &bytes[chunk[1].clone()]);
    let mut raw = [0; 4];
    std::iter::from_fn(|| {
        source.read_exact(&mut raw).ok()?;
        Some(f32::from_ne_bytes(raw))
    })
    .any(|sample| sample != 0.0 && sample.is_finite())
}

struct Packet {
    samples: Option<Box<[f32]>>,
    frames: u64,
    format: AudioFormat,
    epoch: u64,
    timeline: Range<u64>,
}

pub(super) struct CaptureWriter {
    producer: Producer<Packet>,
    recycled: Consumer<Box<[f32]>>,
    shared: Arc<Shared>,
    pub(super) format: Option<AudioFormat>,
    pending: Option<Packet>,
    pool: Vec<Box<[f32]>>,
    retired: Vec<Box<[f32]>>,
    pool_samples: usize,
    pool_limit: usize,
    activity_epoch: u64,
    previous_timing: Range<u64>,
    disconnected: bool,
    overflowed: bool,
}

impl CaptureWriter {
    pub(super) fn set_status(&mut self, status: StreamStatus) {
        if status != StreamStatus::Streaming {
            self.flush_pending();
        }
        self.shared.status.store(status as u8, Ordering::Release);
    }

    pub(super) fn status(&self) -> StreamStatus {
        match self.shared.status.load(Ordering::Acquire) {
            value if value == StreamStatus::Paused as u8 => StreamStatus::Paused,
            value if value == StreamStatus::Streaming as u8 => StreamStatus::Streaming,
            value if value == StreamStatus::Failed as u8 => StreamStatus::Failed,
            value if value == StreamStatus::Stopped as u8 => StreamStatus::Stopped,
            _ => StreamStatus::Starting,
        }
    }

    pub(super) fn clear_format(&mut self) {
        self.flush_pending();
        self.format = None;
        self.clear_pool();
    }

    pub(super) fn reclaim_buffers(&mut self) {
        self.retired.clear();
        while let Ok(samples) = self.recycled.pop() {
            if self.can_pool(&samples) {
                self.pool.push(samples);
            }
        }
    }

    pub(super) fn disconnect(&mut self) {
        self.discard_pending();
        self.clear_pool();
        self.format = None;
        if !self.disconnected {
            self.shared.fault();
            self.disconnected = true;
        }
        self.set_status(StreamStatus::Failed);
    }

    pub(super) fn set_format(
        &mut self,
        channels: usize,
        rate: u32,
        positions: [ChannelPosition; MAX_CAPTURE_CHANNELS],
    ) -> AudioFormat {
        self.flush_pending();
        let format = self.publish_format(channels, rate, positions);
        if self.format.replace(format) != Some(format) {
            self.configure_pool(format);
        }
        self.disconnected = false;
        format
    }

    pub(super) fn publish_format(
        &self,
        channels: usize,
        rate: u32,
        positions: [ChannelPosition; MAX_CAPTURE_CHANNELS],
    ) -> AudioFormat {
        let current = self.shared.format();
        let mut format = AudioFormat::new(channels, rate, current.generation, positions);
        if current.generation != 0 && format == current {
            return current;
        }
        format.generation = current.generation.saturating_add(1);
        *unpoison(self.shared.format.write()) = format;
        if self.shared.wake_ingestion() {
            self.shared.notify();
        }
        format
    }

    pub(super) fn mark_reconnect(&self) {
        self.shared.reconnects.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn push_pcm(&mut self, bytes: &[u8], chunk: &[Range<usize>; 2], frames: u64) {
        let Some(format) = self.format else { return };
        let mut wake = false;
        loop {
            let dormant = self.shared.ingest() == IngestState::Dormant;
            let mut signal = dormant && pcm_has_signal(bytes, chunk);
            if dormant && !signal {
                self.push_frames(format, frames, true, |samples| samples.fill(0.0));
                return;
            }
            wake |= signal && self.shared.wake_ingestion();

            let previous_timing = self.previous_timing.clone();
            let mut source = Read::chain(&bytes[chunk[0].clone()], &bytes[chunk[1].clone()]);
            let accepted = self.push_frames(format, frames, true, |samples| {
                source
                    .read_exact(bytemuck::cast_slice_mut(samples))
                    .expect("valid PCM bounds");
                for sample in samples {
                    if sample.is_finite() {
                        signal |= *sample != 0.0;
                    } else {
                        *sample = 0.0;
                    }
                }
            });
            if accepted {
                if signal && self.pending.is_some() {
                    self.shared.pending_signal.store(true, Ordering::SeqCst);
                }
                wake |= signal && self.shared.wake_ingestion();
                if wake {
                    self.flush_pending();
                    self.shared.notify();
                }
                return;
            }

            signal |= pcm_has_signal(bytes, chunk);
            wake |= signal && self.shared.wake_ingestion();
            if self.shared.ingest() != IngestState::Active {
                return;
            }
            self.previous_timing = previous_timing;
        }
    }

    pub(super) fn push_silence(&mut self, frames: u64) {
        let Some(format) = self.format.filter(|_| frames > 0) else {
            return;
        };
        self.push_frames(format, frames, false, |samples| samples.fill(0.0));
    }

    fn push_frames(
        &mut self,
        format: AudioFormat,
        frames: u64,
        pcm: bool,
        mut write: impl FnMut(&mut [f32]),
    ) -> bool {
        if !self.accepting() {
            self.timing(frames, format);
            return false;
        }
        let (start, end) = self.timing(frames, format);
        let packet_frames = packet_frame_limit(format.rate()) as u64;
        let mut offset = 0;
        while offset < frames {
            let block_start = start + scale(end - start, offset, frames);
            if !self.start_packet(pcm, format, block_start) {
                self.overflow();
                return true;
            }
            let packet = self.pending.as_mut().expect("pending packet");
            let count = (frames - offset).min(packet_frames - packet.frames);
            if let Some(samples) = &mut packet.samples {
                let from = packet.frames as usize * format.channels;
                let to = (packet.frames + count) as usize * format.channels;
                write(&mut samples[from..to]);
            }
            offset += count;
            packet.frames += count;
            packet.timeline.end = start + scale(end - start, offset, frames);
            if packet.frames == packet_frames && !self.flush_pending() {
                self.overflow();
                return true;
            }
        }
        true
    }

    pub(super) fn push_fault(&mut self, frames: u64) {
        let Some(format) = self.format else {
            return;
        };
        self.timing(frames, format);
        let active = self.shared.ingest() != IngestState::Disabled;
        self.discard_pending();
        if active {
            self.shared.fault();
        }
    }

    fn accepting(&mut self) -> bool {
        let epoch = self.shared.activity_epoch.load(Ordering::Acquire);
        let changed = self.activity_epoch != epoch;
        self.activity_epoch = epoch;
        if changed {
            self.discard_pending();
        }
        match self.shared.ingest() {
            IngestState::Active => true,
            IngestState::Dormant
                if !changed
                    && self.shared.pending_signal.load(Ordering::SeqCst)
                    && self.shared.wake_ingestion() =>
            {
                self.flush_pending();
                self.shared.notify();
                true
            }
            _ => {
                self.discard_pending();
                false
            }
        }
    }

    fn start_packet(&mut self, pcm: bool, format: AudioFormat, start: u64) -> bool {
        if self
            .pending
            .as_ref()
            .is_some_and(|packet| packet.format != format || packet.timeline.end != start)
            && !self.flush_pending()
        {
            return false;
        }
        if self.pending.is_none() {
            let samples = if pcm {
                let Some(samples) = self.take_samples() else {
                    return false;
                };
                Some(samples)
            } else {
                None
            };
            self.pending = Some(Packet {
                samples,
                frames: 0,
                format,
                epoch: self.activity_epoch,
                timeline: start..start,
            });
        } else if pcm
            && self
                .pending
                .as_ref()
                .is_some_and(|packet| packet.samples.is_none())
        {
            let Some(mut samples) = self.take_samples() else {
                self.discard_pending();
                return false;
            };
            let packet = self.pending.as_mut().expect("pending packet");
            samples[..packet.frames as usize * format.channels].fill(0.0);
            packet.samples = Some(samples);
        }
        true
    }

    fn configure_pool(&mut self, format: AudioFormat) {
        self.clear_pool();
        self.retired.reserve(self.producer.buffer().capacity() + 1);
        self.pool_samples = format.channels * packet_frame_limit(format.rate());
        self.pool_limit = packet_pool_limit(format.rate(), self.producer.buffer().capacity());
        self.pool.reserve(self.pool_limit);
        self.pool
            .extend((0..self.pool_limit).map(|_| vec![0.0; self.pool_samples].into_boxed_slice()));
    }

    fn clear_pool(&mut self) {
        self.pool.clear();
        self.retired.clear();
        while self.recycled.pop().is_ok() {}
        self.pool_samples = 0;
        self.pool_limit = 0;
    }

    fn can_pool(&self, samples: &[f32]) -> bool {
        samples.len() == self.pool_samples && self.pool.len() < self.pool_limit
    }

    fn take_samples(&mut self) -> Option<Box<[f32]>> {
        while let Ok(samples) = self.recycled.pop() {
            if self.can_pool(&samples) {
                return Some(samples);
            }
            self.retired.push(samples);
        }
        self.pool.pop()
    }

    fn reclaim_samples(&mut self, samples: Option<Box<[f32]>>) {
        if let Some(samples) = samples {
            if self.can_pool(&samples) {
                self.pool.push(samples);
            } else {
                self.retired.push(samples);
            }
        }
    }

    fn discard_pending(&mut self) {
        let samples = self.pending.take().and_then(|packet| packet.samples);
        self.shared.pending_signal.store(false, Ordering::SeqCst);
        self.reclaim_samples(samples);
    }

    fn flush_pending(&mut self) -> bool {
        let Some(packet) = self.pending.take().filter(|packet| packet.frames > 0) else {
            return true;
        };
        let result = self.producer.push(packet);
        self.shared.pending_signal.store(false, Ordering::SeqCst);
        if let Err(PushError::Full(packet)) = result {
            self.reclaim_samples(packet.samples);
            self.overflow();
            false
        } else {
            self.overflowed = false;
            true
        }
    }

    fn timing(&mut self, frames: u64, format: AudioFormat) -> (u64, u64) {
        let now = self.shared.now_ns();
        let duration = frames_ns(frames, format.rate()).max(1);
        let watchdog = idle_watchdog_ns(format.rate());
        let continuous = self.previous_timing.end != 0
            && now.saturating_sub(self.previous_timing.start) <= watchdog
            && self.previous_timing.end.abs_diff(now) <= watchdog;
        let start = if continuous {
            self.previous_timing.end
        } else {
            now.saturating_sub(duration)
        };
        self.previous_timing = now..start.saturating_add(duration);
        (start, self.previous_timing.end)
    }

    fn overflow(&mut self) {
        if !self.overflowed {
            self.shared.fault();
            self.overflowed = true;
        }
    }
}

impl Drop for CaptureWriter {
    fn drop(&mut self) {
        if self.status() != StreamStatus::Stopped {
            self.disconnect();
        }
    }
}

pub struct AudioReader {
    consumer: Consumer<Packet>,
    recycler: Producer<Box<[f32]>>,
    shared: Arc<Shared>,
    scratch: Vec<f32>,
    format: AudioFormat,
    cursor: u64,
    align_next_packet: bool,
    fault_epoch: u64,
    dormant: bool,
}

impl AudioReader {
    pub fn drain<F>(&mut self, now: Instant, mut consume: F)
    where
        F: for<'a> FnMut(CapturedSpan<'a>),
    {
        if self.shared.ingest() != IngestState::Active {
            return;
        }
        let now_ns = nanos(now.saturating_duration_since(self.shared.epoch));
        if std::mem::take(&mut self.dormant)
            && self.fault_epoch == self.shared.fault_epoch.load(Ordering::Acquire)
            && self.format == self.shared.format()
        {
            self.scratch.clear();
            self.cursor = now_ns;
            self.align_next_packet = true;
        }
        if self.consumer.peek().is_ok_and(|packet| {
            packet.epoch == self.shared.activity_epoch.load(Ordering::Acquire)
                && now_ns.saturating_sub(packet.timeline.end) > nanos(MAX_BACKLOG)
        }) {
            self.shared.fault();
        }
        if self.synchronize_fault(&mut consume) {
            return;
        }

        while let Ok(packet) = self.consumer.pop() {
            self.accept(packet, &mut consume);
            if self.scratch.len() >= PCM_FLUSH_SAMPLES {
                self.flush(&mut consume);
            }
        }
        self.flush(&mut consume);
        if self.synchronize_fault(&mut consume) {
            return;
        }

        let format = self.shared.format();
        let streaming = self.shared.status.load(Ordering::Acquire) == StreamStatus::Streaming as u8;
        if !streaming {
            self.align_next_packet = true;
        }
        let target = now_ns.saturating_sub(if streaming {
            idle_watchdog_ns(format.rate())
        } else {
            0
        });
        if format.generation == 0 {
            self.cursor = target;
            return;
        }
        if target > self.cursor {
            self.switch(format, &mut consume);
            let frames = ns_frames(target - self.cursor, format.rate());
            if frames > 0 {
                self.cursor = self.cursor.saturating_add(frames_ns(frames, format.rate()));
                self.align_next_packet = true;
                consume(CapturedSpan::Silence { frames, format });
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn is_active(&self) -> bool {
        self.shared.ingest() == IngestState::Active
    }

    pub fn set_active(&mut self, active: bool) -> bool {
        let next = if active {
            IngestState::Active
        } else {
            IngestState::Disabled
        };
        if self.shared.ingest.swap(next as u8, Ordering::AcqRel) == next as u8 {
            return false;
        }
        self.shared.activity_epoch.fetch_add(1, Ordering::AcqRel);
        self.reset_timeline(self.shared.now_ns());
        true
    }

    pub fn sleep_until_signal(&mut self) -> bool {
        if let Err(state) =
            self.shared
                .ingest
                .fetch_update(Ordering::SeqCst, Ordering::Relaxed, |state| {
                    (state == IngestState::Active as u8).then_some(IngestState::Dormant as u8)
                })
        {
            return state == IngestState::Dormant as u8;
        }
        self.dormant = true;
        if self.shared.pending_signal.load(Ordering::SeqCst)
            || self.consumer.peek().is_ok()
            || self.fault_epoch != self.shared.fault_epoch.load(Ordering::SeqCst)
            || self.format != self.shared.format()
        {
            let _ = self.shared.wake_ingestion();
        }
        self.dormant = self.shared.ingest() == IngestState::Dormant;
        self.dormant
    }

    pub(crate) fn wake_handle(&self) -> AudioWake {
        let identity = Arc::as_ptr(&self.shared) as usize;
        AudioWake(self.shared.wake_receiver.clone(), identity)
    }

    pub fn discard(&mut self, now: Instant) {
        self.shared.activity_epoch.fetch_add(1, Ordering::AcqRel);
        self.reset_timeline(nanos(now.saturating_duration_since(self.shared.epoch)));
    }

    #[cfg(test)]
    pub(super) fn reconnects(&self) -> u64 {
        self.shared.reconnects.load(Ordering::Relaxed)
    }

    fn synchronize_fault(&mut self, consume: &mut impl SpanConsumer) -> bool {
        let fault = self.shared.fault_epoch.load(Ordering::Acquire);
        if fault == self.fault_epoch {
            return false;
        }
        self.reset_timeline(self.shared.now_ns());
        self.fault_epoch = fault;
        self.format = self.shared.format();
        consume(CapturedSpan::Reset);
        true
    }

    fn accept(&mut self, packet: Packet, consume: &mut impl SpanConsumer) {
        if packet.epoch != self.shared.activity_epoch.load(Ordering::Acquire) {
            if let Some(samples) = packet.samples {
                let _ = self.recycler.push(samples);
            }
            return;
        }
        let Packet {
            samples,
            frames,
            format,
            timeline,
            ..
        } = packet;
        self.switch(format, consume);
        if std::mem::take(&mut self.align_next_packet) {
            self.cursor = timeline.start;
        }
        let gap = (timeline.start > self.cursor)
            .then(|| ns_frames(timeline.start - self.cursor, format.rate()));
        let skip = if self.cursor > timeline.start {
            ns_frames_ceil(
                self.cursor.min(timeline.end) - timeline.start,
                format.rate(),
            )
            .min(frames)
        } else {
            0
        };
        self.cursor = self.cursor.max(timeline.end);

        if let Some(gap) = gap.filter(|frames| *frames > 0) {
            self.flush(consume);
            consume(CapturedSpan::Silence {
                frames: gap,
                format,
            });
        }
        if let Some(samples) = samples {
            if skip < frames {
                let from = skip as usize * format.channels;
                self.scratch
                    .extend_from_slice(&samples[from..frames as usize * format.channels]);
            }
            let _ = self.recycler.push(samples);
        } else if skip < frames {
            self.flush(consume);
            consume(CapturedSpan::Silence {
                frames: frames - skip,
                format,
            });
        }
    }

    fn switch(&mut self, format: AudioFormat, consume: &mut impl SpanConsumer) {
        if self.format != format {
            self.flush(consume);
            self.format = format;
        }
    }

    fn flush(&mut self, consume: &mut impl SpanConsumer) {
        if self.scratch.is_empty() {
            return;
        }
        consume(CapturedSpan::Pcm {
            samples: &self.scratch,
            format: self.format,
        });
        self.scratch.clear();
    }

    fn reset_timeline(&mut self, cursor: u64) {
        while let Ok(packet) = self.consumer.pop() {
            if let Some(samples) = packet.samples {
                let _ = self.recycler.push(samples);
            }
        }
        self.scratch.clear();
        self.cursor = cursor;
        self.align_next_packet = true;
        self.fault_epoch = self.shared.fault_epoch.load(Ordering::Acquire);
        self.dormant = false;
    }
}

impl Drop for AudioReader {
    fn drop(&mut self) {
        info!(
            "[capture] stopped after {} fault(s) and {} reconnect(s)",
            self.shared.fault_epoch.load(Ordering::Relaxed),
            self.shared.reconnects.load(Ordering::Relaxed)
        );
    }
}

pub(super) fn channel() -> (CaptureWriter, AudioReader) {
    channel_with_capacity(RING_BLOCKS)
}

fn channel_with_capacity(capacity: usize) -> (CaptureWriter, AudioReader) {
    let shared = Arc::new(Shared::new());
    let (producer, consumer) = RingBuffer::new(capacity);
    let (recycler, recycled) = RingBuffer::new(capacity + 1);
    let format = shared.format();
    (
        CaptureWriter {
            producer,
            recycled,
            shared: Arc::clone(&shared),
            format: None,
            pending: None,
            pool: Vec::new(),
            retired: Vec::new(),
            pool_samples: 0,
            pool_limit: 0,
            activity_epoch: 0,
            previous_timing: 0..0,
            disconnected: false,
            overflowed: false,
        },
        AudioReader {
            consumer,
            recycler,
            shared,
            scratch: Vec::with_capacity(PCM_FLUSH_SAMPLES),
            format,
            cursor: 0,
            align_next_packet: true,
            fault_epoch: 0,
            dormant: false,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    fn mono(capacity: usize, rate: u32) -> (CaptureWriter, AudioReader, AudioFormat) {
        let (mut writer, mut reader) = channel_with_capacity(capacity);
        let format = writer.set_format(1, rate, ChannelPosition::fallback(1));
        reader.format = format;
        (writer, reader, format)
    }

    fn packet(format: AudioFormat, start: u64, samples: &[f32]) -> Packet {
        let frames = samples.len() as u64 / format.channels as u64;
        let start = frames_ns(start, format.rate());
        Packet {
            samples: Some(samples.into()),
            frames,
            format,
            epoch: 0,
            timeline: start..start + frames_ns(frames, format.rate()),
        }
    }

    #[test]
    fn format_and_packet_timeline_remain_authoritative() {
        let (mut writer, mut reader) = channel_with_capacity(4);
        let start = reader.shared.epoch + Duration::from_millis(10);
        let mut emitted = false;
        reader.drain(start, |_| emitted = true);
        assert!(!emitted);
        let positions = ChannelPosition::fallback(2);
        let hint = writer.publish_format(2, DEFAULT_SAMPLE_RATE as u32, positions);
        let mut seeded = 0;
        reader.drain(start + Duration::from_millis(10), |span| {
            if let CapturedSpan::Silence { frames, .. } = span {
                seeded = frames;
            }
        });
        assert_eq!(seeded, 480);
        assert_eq!(writer.format.map(|format| format.channels), None);
        assert_eq!(writer.set_format(2, 48_000, positions), hint);
        assert_ne!(
            writer.set_format(2, 96_000, positions).generation,
            hint.generation
        );

        let (_, mut reader, format) = mono(4, 1_000);
        let mut spans = Vec::new();
        for packet in [
            packet(format, 0, &[1.0; 4]),
            packet(format, 6, &[2.0; 4]),
            packet(format, 8, &[3.0; 4]),
        ] {
            reader.accept(packet, &mut |span| match span {
                CapturedSpan::Pcm { samples, .. } => spans.push((samples.len() as u64, false)),
                CapturedSpan::Silence { frames, .. } => spans.push((frames, true)),
                CapturedSpan::Reset => unreachable!(),
            });
        }
        reader.flush(&mut |span| match span {
            CapturedSpan::Pcm { samples, .. } => spans.push((samples.len() as u64, false)),
            _ => unreachable!(),
        });
        assert_eq!(spans, [(4, false), (2, true), (6, false)]);
    }

    #[test]
    fn pcm_copy_handles_wrapping_and_sanitizes_non_finite_samples() {
        let (mut writer, mut reader, _) = mono(4, 1_000);
        let mut bytes = bytemuck::cast_slice(&[1.0_f32, f32::NAN, 2.0]).to_vec();
        bytes.rotate_right(3);
        let chunk = pcm_chunk(bytes.len(), 3, bytes.len() as u32, size_of::<f32>()).unwrap();
        writer.push_pcm(&bytes, &chunk, 3);
        assert!(writer.flush_pending());
        let samples = reader.consumer.pop().unwrap().samples.unwrap();
        assert_eq!(&samples[..3], &[1.0, 0.0, 2.0]);
    }

    #[test]
    fn capture_faults_reset_instead_of_replaying_audio() {
        let (mut writer, mut reader, _) = mono(1, 48_000);
        writer.pool.clear();
        let bytes = bytemuck::cast_slice(&[0.25_f32]);
        let chunk = pcm_chunk(bytes.len(), 0, bytes.len() as u32, size_of::<f32>()).unwrap();
        writer.push_pcm(bytes, &chunk, 1);
        let mut reset = false;
        reader.drain(writer.shared.epoch, |span| {
            reset |= matches!(span, CapturedSpan::Reset)
        });
        assert!(reset);

        let (mut writer, mut reader, _) = mono(1, 48_000);
        writer.push_silence(BLOCK_FRAMES as u64);
        reset = false;
        reader.drain(
            writer.shared.epoch + MAX_BACKLOG + Duration::from_millis(10),
            |span| reset |= matches!(span, CapturedSpan::Reset),
        );
        assert!(reset);
        assert_eq!(reader.consumer.slots(), 0);
    }

    #[test]
    fn dormant_capture_wakes_on_the_first_signal() {
        let (mut writer, mut reader, _) = mono(4, 48_000);
        let wake = reader.wake_handle();
        assert!(reader.sleep_until_signal());

        writer.push_silence(BLOCK_FRAMES as u64);
        let zero = 0.0_f32.to_ne_bytes();
        let chunk = pcm_chunk(zero.len(), 0, zero.len() as u32, zero.len()).unwrap();
        writer.push_pcm(&zero, &chunk, 1);
        assert!(!reader.is_active());
        assert!(audio_wake(&wake).try_recv().is_err());

        let mut signal = 0.25_f32.to_ne_bytes();
        signal.rotate_right(1);
        let chunk = pcm_chunk(signal.len(), 1, signal.len() as u32, signal.len()).unwrap();
        writer.push_pcm(&signal, &chunk, 1);
        assert!(reader.is_active());
        assert_eq!(audio_wake(&wake).try_recv(), Ok(()));
        let mut captured = false;
        reader.drain(Instant::now(), |span| {
            if let CapturedSpan::Pcm { samples, .. } = span {
                captured |= samples == [0.25];
            }
        });
        assert!(captured);

        let (mut writer, mut reader, _) = mono(4, 48_000);
        writer.push_pcm(&signal, &chunk, 1);
        assert!(!reader.sleep_until_signal());

        let (mut writer, mut reader, _) = mono(4, 48_000);
        assert!(reader.sleep_until_signal());
        writer.push_fault(1);
        assert!(reader.is_active());
    }

    #[test]
    fn inactive_and_reconnect_boundaries_do_not_queue_stale_audio() {
        let (mut writer, mut reader, _) = mono(1, 48_000);
        assert!(reader.set_active(false));
        writer.push_silence(BLOCK_FRAMES as u64);
        assert_eq!(reader.consumer.slots(), 0);
        assert!(reader.set_active(true));

        writer.disconnect();
        writer.disconnect();
        assert_eq!(writer.shared.fault_epoch.load(Ordering::Relaxed), 1);
        writer.set_format(1, 48_000, ChannelPosition::fallback(1));
        writer.disconnect();
        assert_eq!(writer.shared.fault_epoch.load(Ordering::Relaxed), 2);
    }
}
