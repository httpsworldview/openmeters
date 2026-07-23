// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{MAX_CAPTURE_CHANNELS, MAX_CAPTURE_SAMPLE_RATE};
use crate::dsp::{AudioFormat, ChannelPosition};
use crate::util::{audio::DEFAULT_SAMPLE_RATE, unpoison};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::mem::size_of;
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

#[derive(Debug)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum StreamStatus {
    Starting,
    Paused,
    Streaming,
    Failed,
    Stopped,
}

struct Shared {
    epoch: Instant,
    status: AtomicU8,
    format: RwLock<AudioFormat>,
    fault_epoch: AtomicU64,
    activity_epoch: AtomicU64,
    accepting: AtomicBool,
    reconnects: AtomicU64,
}

impl Shared {
    fn new() -> Self {
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
            accepting: AtomicBool::new(true),
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
        self.fault_epoch.fetch_add(1, Ordering::AcqRel);
    }
}

fn frames_ns(frames: u64, rate: u64) -> u64 {
    (u128::from(frames) * 1_000_000_000 / u128::from(rate.max(1))).min(u128::from(u64::MAX)) as u64
}

fn ns_frames(ns: u64, rate: u64) -> u64 {
    (u128::from(ns) * u128::from(rate) / 1_000_000_000).min(u128::from(u64::MAX)) as u64
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
    (u128::from(value) * u128::from(numerator) / u128::from(denominator.max(1))) as u64
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PcmChunk {
    first: Range<usize>,
    second: Range<usize>,
}

impl PcmChunk {
    pub(super) fn new(max: usize, offset: u32, size: u32, frame_bytes: usize) -> Option<Self> {
        if max == 0 || frame_bytes == 0 {
            return None;
        }
        let len = (size as usize).min(max) / frame_bytes * frame_bytes;
        if len == 0 {
            return None;
        }
        let start = offset as usize % max;
        let first = len.min(max - start);
        Some(Self {
            first: start..start + first,
            second: 0..len - first,
        })
    }

    pub(super) fn len(&self) -> usize {
        self.first.len() + self.second.len()
    }

    fn byte(&self, bytes: &[u8], index: usize) -> u8 {
        if index < self.first.len() {
            bytes[self.first.start + index]
        } else {
            bytes[self.second.start + index - self.first.len()]
        }
    }
}

#[derive(Debug)]
struct Packet {
    samples: Option<Box<[f32]>>,
    frames: u64,
    format: AudioFormat,
    epoch: u64,
    start: u64,
    end: u64,
}

impl Packet {
    fn new(format: AudioFormat, epoch: u64, start: u64, samples: Option<Box<[f32]>>) -> Self {
        Self {
            samples,
            frames: 0,
            format,
            epoch,
            start,
            end: start,
        }
    }
}

pub(super) struct CaptureWriter {
    producer: Producer<Packet>,
    recycled: Consumer<Box<[f32]>>,
    shared: Arc<Shared>,
    format: Option<AudioFormat>,
    pending: Option<Packet>,
    pool: Vec<Box<[f32]>>,
    retired: Vec<Box<[f32]>>,
    pool_samples: usize,
    pool_limit: usize,
    generation: u64,
    activity_epoch: u64,
    previous_end: u64,
    previous_callback: u64,
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
            if samples.len() == self.pool_samples && self.pool.len() < self.pool_limit {
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

    pub(super) fn channels(&self) -> Option<usize> {
        self.format.map(|format| format.channels)
    }

    pub(super) fn set_format(
        &mut self,
        channels: usize,
        rate: u32,
        positions: [ChannelPosition; MAX_CAPTURE_CHANNELS],
    ) -> AudioFormat {
        self.flush_pending();
        let format = self.publish_format(channels, rate, positions);
        self.configure_pool(format);
        self.format = Some(format);
        self.disconnected = false;
        format
    }

    pub(super) fn publish_format(
        &mut self,
        channels: usize,
        rate: u32,
        positions: [ChannelPosition; MAX_CAPTURE_CHANNELS],
    ) -> AudioFormat {
        let current = self.shared.format();
        let candidate = AudioFormat::new(channels, rate, current.generation, positions);
        if current.generation != 0 && candidate == current {
            return current;
        }
        self.generation = current.generation.max(self.generation).saturating_add(1);
        let format = AudioFormat::new(channels, rate, self.generation, positions);
        *unpoison(self.shared.format.write()) = format;
        format
    }

    pub(super) fn mark_reconnect(&self) {
        self.shared.reconnects.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn push_pcm(&mut self, bytes: &[u8], chunk: &PcmChunk, frames: u64) {
        let Some(format) = self.format else { return };
        self.push_frames(format, frames, true, |samples, source| {
            for (index, sample) in samples.iter_mut().enumerate() {
                let logical = (source + index) * size_of::<f32>();
                let raw = std::array::from_fn(|byte| chunk.byte(bytes, logical + byte));
                let value = f32::from_ne_bytes(raw);
                *sample = if value.is_finite() { value } else { 0.0 };
            }
        });
    }

    pub(super) fn push_silence(&mut self, frames: u64) {
        let Some(format) = self.format.filter(|_| frames > 0) else {
            return;
        };
        self.push_frames(format, frames, false, |samples, _| samples.fill(0.0));
    }

    fn push_frames(
        &mut self,
        format: AudioFormat,
        frames: u64,
        pcm: bool,
        mut write: impl FnMut(&mut [f32], usize),
    ) {
        if !self.accepting() {
            self.timing(frames, format);
            return;
        }
        let (start, end) = self.timing(frames, format);
        let packet_frames = packet_frame_limit(format.rate()) as u64;
        let mut offset = 0;
        while offset < frames {
            let block_start = start + scale(end - start, offset, frames);
            if !self.start_packet(pcm, format, block_start) {
                self.overflow(frames - offset);
                return;
            }
            let packet = self.pending.as_mut().expect("pending packet");
            let count = (frames - offset).min(packet_frames - packet.frames);
            if let Some(samples) = &mut packet.samples {
                let from = packet.frames as usize * format.channels;
                let to = (packet.frames + count) as usize * format.channels;
                write(&mut samples[from..to], offset as usize * format.channels);
            }
            offset += count;
            packet.frames += count;
            packet.end = start + scale(end - start, offset, frames);
            if packet.frames == packet_frames && !self.flush_pending() {
                self.overflow(frames - offset);
                return;
            }
        }
    }

    pub(super) fn push_fault(&mut self, frames: u64) {
        let Some(format) = self.format else {
            return;
        };
        self.timing(frames, format);
        self.discard_pending();
        if self.accepting() {
            self.shared.fault();
        }
    }

    fn accepting(&mut self) -> bool {
        let epoch = self.shared.activity_epoch.load(Ordering::Acquire);
        if self.activity_epoch != epoch {
            self.discard_pending();
            self.activity_epoch = epoch;
        }
        let accepting = self.shared.accepting.load(Ordering::Acquire);
        if !accepting {
            self.discard_pending();
        }
        accepting
    }

    fn start_packet(&mut self, pcm: bool, format: AudioFormat, start: u64) -> bool {
        if self
            .pending
            .as_ref()
            .is_some_and(|packet| packet.format != format || packet.end != start)
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
            self.pending = Some(Packet::new(format, self.activity_epoch, start, samples));
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

    fn take_samples(&mut self) -> Option<Box<[f32]>> {
        while let Ok(samples) = self.recycled.pop() {
            if samples.len() == self.pool_samples && self.pool.len() < self.pool_limit {
                return Some(samples);
            }
            self.retired.push(samples);
        }
        self.pool.pop()
    }

    fn reclaim_samples(&mut self, samples: Option<Box<[f32]>>) {
        if let Some(samples) = samples {
            if samples.len() == self.pool_samples && self.pool.len() < self.pool_limit {
                self.pool.push(samples);
            } else {
                self.retired.push(samples);
            }
        }
    }

    fn discard_pending(&mut self) {
        let samples = self.pending.take().and_then(|packet| packet.samples);
        self.reclaim_samples(samples);
    }

    fn flush_pending(&mut self) -> bool {
        let Some(packet) = self.pending.take().filter(|packet| packet.frames > 0) else {
            return true;
        };
        let frames = packet.frames;
        if let Err(PushError::Full(packet)) = self.producer.push(packet) {
            self.reclaim_samples(packet.samples);
            self.overflow(frames);
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
        let continuous = self.previous_end != 0
            && now.saturating_sub(self.previous_callback) <= watchdog
            && self.previous_end.abs_diff(now) <= watchdog;
        let start = if continuous {
            self.previous_end
        } else {
            now.saturating_sub(duration)
        };
        self.previous_end = start.saturating_add(duration);
        self.previous_callback = now;
        (start, self.previous_end)
    }

    fn overflow(&mut self, _frames: u64) {
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
}

impl AudioReader {
    pub fn drain<F>(&mut self, now: Instant, mut consume: F)
    where
        F: for<'a> FnMut(CapturedSpan<'a>),
    {
        if !self.shared.accepting.load(Ordering::Acquire) {
            self.discard(now);
            return;
        }
        let now_ns = nanos(now.saturating_duration_since(self.shared.epoch));
        if self.consumer.peek().is_ok_and(|packet| {
            packet.epoch == self.shared.activity_epoch.load(Ordering::Acquire)
                && now_ns.saturating_sub(packet.end) > nanos(MAX_BACKLOG)
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
        self.shared.accepting.load(Ordering::Acquire)
    }

    pub fn set_active(&mut self, active: bool) -> bool {
        if self.shared.accepting.load(Ordering::Acquire) == active {
            return false;
        }
        if !active {
            self.shared.accepting.store(false, Ordering::Release);
        }
        self.shared.activity_epoch.fetch_add(1, Ordering::AcqRel);
        self.reset_timeline(self.shared.now_ns());
        if active {
            self.shared.accepting.store(true, Ordering::Release);
        }
        true
    }

    pub fn discard(&mut self, now: Instant) {
        self.shared.activity_epoch.fetch_add(1, Ordering::AcqRel);
        self.reset_timeline(nanos(now.saturating_duration_since(self.shared.epoch)));
    }

    #[cfg(test)]
    pub(super) fn reconnects(&self) -> u64 {
        self.shared.reconnects.load(Ordering::Relaxed)
    }

    fn synchronize_fault<F>(&mut self, consume: &mut F) -> bool
    where
        F: for<'a> FnMut(CapturedSpan<'a>),
    {
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

    fn accept<F>(&mut self, packet: Packet, consume: &mut F)
    where
        F: for<'a> FnMut(CapturedSpan<'a>),
    {
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
            start,
            end,
            ..
        } = packet;
        self.switch(format, consume);
        if std::mem::take(&mut self.align_next_packet) {
            self.cursor = start;
        }
        let gap = (start > self.cursor).then(|| ns_frames(start - self.cursor, format.rate()));
        let skip = if self.cursor > start {
            ns_frames_ceil(self.cursor.min(end) - start, format.rate()).min(frames)
        } else {
            0
        };
        self.cursor = self.cursor.max(end);

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

    fn switch<F>(&mut self, format: AudioFormat, consume: &mut F)
    where
        F: for<'a> FnMut(CapturedSpan<'a>),
    {
        if self.format != format {
            self.flush(consume);
            self.format = format;
        }
    }

    fn flush<F>(&mut self, consume: &mut F)
    where
        F: for<'a> FnMut(CapturedSpan<'a>),
    {
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
        self.clear_queue();
        self.scratch.clear();
        self.cursor = cursor;
        self.align_next_packet = true;
        self.fault_epoch = self.shared.fault_epoch.load(Ordering::Acquire);
    }

    fn clear_queue(&mut self) {
        while let Ok(packet) = self.consumer.pop() {
            if let Some(samples) = packet.samples {
                let _ = self.recycler.push(samples);
            }
        }
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
            generation: 0,
            activity_epoch: 0,
            previous_end: 0,
            previous_callback: 0,
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
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mono(capacity: usize, rate: u32) -> (CaptureWriter, AudioReader, AudioFormat) {
        let (mut writer, reader) = channel_with_capacity(capacity);
        let format = writer.set_format(1, rate, ChannelPosition::fallback(1));
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
            start,
            end: start + frames_ns(frames, format.rate()),
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
        assert_eq!(writer.channels(), None);
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
    fn capture_faults_reset_instead_of_replaying_audio() {
        let (mut writer, mut reader, _) = mono(1, 48_000);
        writer.pool.clear();
        let bytes = bytemuck::cast_slice(&[0.25_f32]);
        let chunk = PcmChunk::new(bytes.len(), 0, bytes.len() as u32, size_of::<f32>()).unwrap();
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
