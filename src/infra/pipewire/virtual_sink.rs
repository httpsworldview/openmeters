// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::VIRTUAL_SINK_NAME;
use crate::util::audio::DEFAULT_SAMPLE_RATE;
use pipewire as pw;
use pw::{properties::properties, spa};
use spa::pod::Pod;
use std::collections::VecDeque;
use std::error::Error;
use std::io::{self, Cursor};
use std::mem::size_of;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::Duration;
use tracing::{error, info, warn};

const VIRTUAL_SINK_SAMPLE_RATE: u32 = DEFAULT_SAMPLE_RATE as u32;
const CAPTURE_BUFFER_CAPACITY: usize = 64;
const CAPTURE_POOL_INITIAL_SAMPLES: usize = 4_096;
const CAPTURE_POOL_MAX_SAMPLES: usize = 65_536;
const CAPTURE_POOL_SPARE_BUFFERS: usize = 8;
const DESIRED_LATENCY_FRAMES: u32 = 256;

static SINK_THREAD: Mutex<Option<thread::JoinHandle<()>>> = Mutex::new(None);
static CAPTURE_BUFFER: OnceLock<Arc<CaptureBuffer>> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct CapturedAudio {
    pub samples: Vec<f32>,
    pub channels: u32,
    pub sample_rate: u32,
}

pub struct CaptureBuffer {
    inner: Mutex<VecDeque<CapturedAudio>>,
    recycled: Mutex<Vec<Vec<f32>>>,
    capacity: usize,
    pool_capacity: usize,
    available: Condvar,
    dropped_frames: AtomicU64,
    requested_pool_samples: AtomicUsize,
}

impl CaptureBuffer {
    fn new(capacity: usize) -> Self {
        let pool_capacity = capacity.saturating_add(CAPTURE_POOL_SPARE_BUFFERS);
        Self {
            inner: Mutex::new(VecDeque::with_capacity(capacity)),
            recycled: Mutex::new(
                (0..pool_capacity)
                    .map(|_| Vec::with_capacity(CAPTURE_POOL_INITIAL_SAMPLES))
                    .collect(),
            ),
            capacity,
            pool_capacity,
            available: Condvar::new(),
            dropped_frames: AtomicU64::new(0),
            requested_pool_samples: AtomicUsize::new(0),
        }
    }

    pub fn try_push(&self, frame: CapturedAudio) {
        let Ok(mut guard) = self.inner.try_lock() else {
            self.note_dropped_frame();
            self.recycle_samples(frame.samples);
            return;
        };
        if guard.len() >= self.capacity {
            if let Some(old) = guard.pop_front() {
                self.recycle_samples(old.samples);
            }
            self.note_dropped_frame();
        }
        guard.push_back(frame);
        self.available.notify_one();
    }

    pub fn pop_wait_timeout(&self, timeout: Duration) -> Option<CapturedAudio> {
        let mut guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.is_empty() && !timeout.is_zero() {
            guard = self
                .available
                .wait_timeout_while(guard, timeout, |queue| queue.is_empty())
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        guard.pop_front()
    }

    pub fn dropped_frames(&self) -> u64 {
        self.dropped_frames.load(Ordering::Relaxed)
    }

    pub fn try_acquire_samples(&self, needed: usize) -> Option<Vec<f32>> {
        if !(1..=CAPTURE_POOL_MAX_SAMPLES).contains(&needed) {
            self.note_dropped_frame();
            return None;
        }

        let Ok(mut recycled) = self.recycled.try_lock() else {
            self.note_dropped_frame();
            return None;
        };
        let Some(index) = recycled.iter().rposition(|buf| buf.capacity() >= needed) else {
            self.request_pool_growth(needed);
            self.note_dropped_frame();
            return None;
        };
        let mut samples = recycled.swap_remove(index);
        samples.clear();
        Some(samples)
    }

    pub fn recycle_samples(&self, samples: Vec<f32>) {
        if let Ok(mut recycled) = self.recycled.try_lock() {
            self.recycle_samples_locked(samples, &mut recycled);
        }
    }

    pub fn recycle_samples_blocking(&self, samples: Vec<f32>) {
        let mut recycled = self.recycled.lock().unwrap_or_else(PoisonError::into_inner);
        self.recycle_samples_locked(samples, &mut recycled);
    }

    fn recycle_samples_locked(&self, mut samples: Vec<f32>, recycled: &mut Vec<Vec<f32>>) {
        if samples.capacity() <= CAPTURE_POOL_MAX_SAMPLES && recycled.len() < self.pool_capacity {
            samples.clear();
            recycled.push(samples);
        }
    }

    pub fn grow_recycle_pool(&self) {
        let Some(requested) = self.requested_pool_capacity() else {
            return;
        };
        let mut recycled = self.recycled.lock().unwrap_or_else(PoisonError::into_inner);
        if recycled.iter().all(|b| b.capacity() >= requested) {
            return;
        }
        for buffer in recycled.iter_mut().filter(|b| b.capacity() < requested) {
            if let Err(err) = buffer.try_reserve_exact(requested) {
                warn!("[virtual-sink] failed to grow capture buffer pool: {err}");
                return;
            }
        }
        drop(recycled);
        info!("[virtual-sink] grew capture buffer pool to {requested} samples per packet");
    }

    fn request_pool_growth(&self, needed: usize) {
        self.requested_pool_samples
            .fetch_max(needed, Ordering::Relaxed);
    }

    fn requested_pool_capacity(&self) -> Option<usize> {
        let requested = self.requested_pool_samples.swap(0, Ordering::Relaxed);
        (requested > 0).then(|| {
            requested
                .next_power_of_two()
                .clamp(CAPTURE_POOL_INITIAL_SAMPLES, CAPTURE_POOL_MAX_SAMPLES)
        })
    }

    fn note_dropped_frame(&self) {
        self.dropped_frames.fetch_add(1, Ordering::Relaxed);
    }
}

fn bytes_per_sample(format: spa::param::audio::AudioFormat) -> Option<usize> {
    use spa::param::audio::AudioFormat as Fmt;

    match format {
        Fmt::F32LE
        | Fmt::F32BE
        | Fmt::S24_32LE
        | Fmt::S24_32BE
        | Fmt::S32LE
        | Fmt::S32BE
        | Fmt::U32LE
        | Fmt::U32BE => Some(4),
        Fmt::F64LE | Fmt::F64BE => Some(8),
        Fmt::S16LE | Fmt::S16BE | Fmt::U16LE | Fmt::U16BE => Some(2),
        Fmt::S8 | Fmt::U8 => Some(1),
        _ => None,
    }
}

fn audio_chunk(bytes: &[u8], offset: u32, size: u32, frame_bytes: usize) -> Option<&[u8]> {
    let frame_bytes = frame_bytes.max(1);
    let start = usize::try_from(offset).ok()?;
    let end = start
        .saturating_add(usize::try_from(size).ok()?)
        .min(bytes.len());
    let len = end.checked_sub(start)? / frame_bytes * frame_bytes;
    (len > 0).then(|| &bytes[start..start + len])
}

fn converted_sample_count(bytes: &[u8], format: spa::param::audio::AudioFormat) -> Option<usize> {
    let sample_bytes = bytes_per_sample(format)?;
    bytes
        .len()
        .is_multiple_of(sample_bytes)
        .then_some(bytes.len() / sample_bytes)
}

fn convert_samples_to_f32_into(
    bytes: &[u8],
    format: spa::param::audio::AudioFormat,
    out: &mut Vec<f32>,
) -> Option<()> {
    use spa::param::audio::AudioFormat as Fmt;

    let sample_count = converted_sample_count(bytes, format)?;
    if out.capacity() < sample_count {
        return None;
    }
    out.clear();

    let little_endian = matches!(
        format,
        Fmt::F32LE | Fmt::F64LE | Fmt::S16LE | Fmt::S24_32LE | Fmt::S32LE | Fmt::U16LE | Fmt::U32LE
    );
    macro_rules! convert {
        ($ty:ty, $to_f32:expr) => {{
            out.extend(bytes.chunks_exact(size_of::<$ty>()).map(|chunk| {
                let mut bytes = [0; size_of::<$ty>()];
                bytes.copy_from_slice(chunk);
                let raw = if little_endian {
                    <$ty>::from_le_bytes(bytes)
                } else {
                    <$ty>::from_be_bytes(bytes)
                };
                $to_f32(raw)
            }));
        }};
    }
    const I8_DIV: f32 = i8::MAX as f32;
    const I16_DIV: f32 = i16::MAX as f32;
    const I32_DIV: f32 = i32::MAX as f32;

    match format {
        Fmt::F32LE | Fmt::F32BE => convert!(f32, |v| v),
        Fmt::F64LE | Fmt::F64BE => convert!(f64, |v| v as f32),
        Fmt::S16LE | Fmt::S16BE => convert!(i16, |v| v as f32 / I16_DIV),
        Fmt::S32LE | Fmt::S24_32LE | Fmt::S32BE | Fmt::S24_32BE => {
            convert!(i32, |v| v as f32 / I32_DIV)
        }
        Fmt::U16LE | Fmt::U16BE => convert!(u16, |v| (v as f32 - 32_768.0) / 32_768.0),
        Fmt::U32LE | Fmt::U32BE => {
            convert!(u32, |v| (v as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32)
        }
        Fmt::U8 => out.extend(bytes.iter().map(|&b| (b as f32 - 128.0) / 128.0)),
        Fmt::S8 => convert!(i8, |v| f32::from(v) / I8_DIV),
        _ => return None,
    }
    Some(())
}

pub fn run() {
    ensure_capture_buffer();

    let mut sink_thread = SINK_THREAD.lock().unwrap_or_else(PoisonError::into_inner);
    if sink_thread.is_none() {
        *sink_thread = thread::Builder::new()
            .name("openmeters-pw-virtual-sink".into())
            .spawn(|| {
                if let Err(err) = run_virtual_sink() {
                    error!("[virtual-sink] stopped: {err}");
                }
            })
            .inspect_err(|err| error!("[virtual-sink] failed to start PipeWire thread: {err}"))
            .ok();
    }
}

pub fn capture_buffer_handle() -> Arc<CaptureBuffer> {
    ensure_capture_buffer().clone()
}

struct VirtualSinkState {
    frame_bytes: usize,
    channels: u32,
    sample_rate: u32,
    format: spa::param::audio::AudioFormat,
}

impl Default for VirtualSinkState {
    fn default() -> Self {
        Self {
            frame_bytes: 2 * size_of::<f32>(),
            channels: 2,
            sample_rate: VIRTUAL_SINK_SAMPLE_RATE,
            format: spa::param::audio::AudioFormat::F32LE,
        }
    }
}

impl VirtualSinkState {
    fn update_from_info(&mut self, info: &spa::param::audio::AudioInfoRaw) {
        self.channels = info.channels().max(1);
        self.sample_rate = info.rate();
        self.format = info.format();
        if let Some(sample_bytes) = bytes_per_sample(self.format) {
            self.frame_bytes = self.channels as usize * sample_bytes;
        } else {
            warn!(
                "[virtual-sink] unsupported audio format {:?}; falling back to recorded frame size",
                self.format
            );
        }
        info!(
            "[virtual-sink] negotiated format: {:?}, rate {} Hz, channels {}",
            info.format(),
            self.sample_rate,
            self.channels
        );
    }
}

fn capture_audio_chunk(capture_buffer: &CaptureBuffer, bytes: &[u8], state: &VirtualSinkState) {
    let Some(sample_count) = converted_sample_count(bytes, state.format) else {
        capture_buffer.note_dropped_frame();
        return;
    };
    let Some(mut samples) = capture_buffer.try_acquire_samples(sample_count) else {
        return;
    };
    if convert_samples_to_f32_into(bytes, state.format, &mut samples).is_none() {
        capture_buffer.recycle_samples(samples);
        capture_buffer.note_dropped_frame();
        return;
    }
    capture_buffer.try_push(CapturedAudio {
        samples,
        channels: state.channels,
        sample_rate: state.sample_rate,
    });
}

fn run_virtual_sink() -> Result<(), Box<dyn Error + Send + Sync>> {
    pw::init();

    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;

    let stream = pw::stream::StreamBox::new(
        &core,
        "OpenMeters Sink",
        properties! {
            *pw::keys::MEDIA_CLASS => "Audio/Sink",
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_ROLE => "Playback",
            *pw::keys::MEDIA_CATEGORY => "Playback",
            *pw::keys::NODE_DESCRIPTION => "OpenMeters Sink",
            *pw::keys::NODE_NAME => VIRTUAL_SINK_NAME,
            *pw::keys::APP_NAME => "OpenMeters",
            *pw::keys::NODE_LATENCY => format!("{}/{}", DESIRED_LATENCY_FRAMES, VIRTUAL_SINK_SAMPLE_RATE),
        },
    )?;

    let audio_state = VirtualSinkState::default();
    let capture_buffer = capture_buffer_handle();

    let _listener = stream
        .add_local_listener_with_user_data(audio_state)
        .state_changed(|_, _, previous, current| {
            info!("[virtual-sink] state {previous:?} -> {current:?}");
        })
        .param_changed(|_, state, id, param| {
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }

            if let Some(pod) = param {
                let mut info = spa::param::audio::AudioInfoRaw::new();
                if info.parse(pod).is_ok() {
                    state.update_from_info(&info);
                }
            }
        })
        .process(move |stream, state| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };

            for data in buffer.datas_mut() {
                let chunk = data.chunk();
                let (offset, size) = (chunk.offset(), chunk.size());

                if size == 0 {
                    continue;
                }

                if let Some(bytes) = data
                    .data()
                    .and_then(|bytes| audio_chunk(bytes, offset, size, state.frame_bytes))
                {
                    capture_audio_chunk(&capture_buffer, bytes, state);
                }

                let chunk_mut = data.chunk_mut();
                *chunk_mut.offset_mut() = 0;
                *chunk_mut.size_mut() = size;
                *chunk_mut.stride_mut() = state.frame_bytes as i32;
            }
        })
        .register()?;

    let format_bytes = build_format_pod(VIRTUAL_SINK_SAMPLE_RATE)?;
    let mut params = [Pod::from_bytes(&format_bytes)
        .ok_or_else(|| io::Error::other("serialized PipeWire format pod was invalid"))?];

    stream.connect(
        spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;

    stream.set_active(true)?;

    info!("[virtual-sink] PipeWire sink active");
    mainloop.run();
    info!("[virtual-sink] main loop exited");

    Ok(())
}

fn build_format_pod(rate: u32) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(rate);

    let (cursor, _) = pw::spa::pod::serialize::PodSerializer::serialize(
        Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(pw::spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: spa::param::ParamType::EnumFormat.as_raw(),
            properties: info.into(),
        }),
    )?;

    Ok(cursor.into_inner())
}

fn ensure_capture_buffer() -> &'static Arc<CaptureBuffer> {
    CAPTURE_BUFFER.get_or_init(|| Arc::new(CaptureBuffer::new(CAPTURE_BUFFER_CAPACITY)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use spa::param::audio::AudioFormat as Fmt;

    fn assert_sample(
        bytes: &[u8],
        fmt: Fmt,
        expected_len: usize,
        index: usize,
        expected: f32,
        eps: f32,
    ) {
        let mut out = Vec::with_capacity(converted_sample_count(bytes, fmt).unwrap());
        convert_samples_to_f32_into(bytes, fmt, &mut out).unwrap();
        assert_eq!(out.len(), expected_len);
        assert!(
            (out[index] - expected).abs() < eps,
            "{fmt:?}[{index}] = {}, expected {expected}",
            out[index]
        );
    }

    #[test]
    fn audio_chunk_respects_offset_and_frame_alignment() {
        let bytes: Vec<_> = (0_u8..16).collect();

        assert_eq!(audio_chunk(&bytes, 4, 8, 4), Some(&bytes[4..12]));
        assert_eq!(audio_chunk(&bytes, 4, 10, 4), Some(&bytes[4..12]));
        assert_eq!(audio_chunk(&bytes, 14, 4, 4), None);
        assert_eq!(audio_chunk(&bytes, 20, 4, 4), None);
    }

    #[test]
    fn capture_buffer_recycles_and_grows_sample_storage() {
        let buffer = CaptureBuffer::new(1);
        let mut samples = buffer.try_acquire_samples(4).unwrap();
        let capacity = samples.capacity();
        samples.extend_from_slice(&[0.0, 1.0, 2.0, 3.0]);

        buffer.try_push(CapturedAudio {
            samples,
            channels: 2,
            sample_rate: 48_000,
        });
        let packet = buffer.pop_wait_timeout(Duration::ZERO).expect("packet");
        assert_eq!(packet.samples, [0.0, 1.0, 2.0, 3.0]);

        buffer.recycle_samples(packet.samples);
        assert_eq!(buffer.try_acquire_samples(4).unwrap().capacity(), capacity);

        let requested = CAPTURE_POOL_INITIAL_SAMPLES + 1;
        assert!(buffer.try_acquire_samples(requested).is_none());
        buffer.grow_recycle_pool();
        assert!(buffer.try_acquire_samples(requested).is_some());
    }

    #[test]
    fn sample_format_conversion() {
        let s16le = [0x00_u8, 0x80, 0xFF, 0x7F];
        let s16be = [0x80, 0x00, 0x7F, 0xFF];
        let u16be = [0x00, 0x00, 0xFF, 0xFF];
        let f32le = 0.123_456_78f32.to_le_bytes();
        let s32le = i32::MAX.to_le_bytes();
        let s8 = [0x80, 0x7F];
        for (bytes, fmt, len, index, expected, eps) in [
            (
                &s16le[..],
                Fmt::S16LE,
                2,
                0,
                i16::MIN as f32 / i16::MAX as f32,
                1e-5,
            ),
            (&s16le, Fmt::S16LE, 2, 1, 1.0, f32::EPSILON),
            (
                &s16be,
                Fmt::S16BE,
                2,
                0,
                i16::MIN as f32 / i16::MAX as f32,
                1e-5,
            ),
            (
                &u16be,
                Fmt::U16BE,
                2,
                1,
                (u16::MAX as f32 - 32_768.0) / 32_768.0,
                f32::EPSILON,
            ),
            (&f32le, Fmt::F32LE, 1, 0, 0.123_456_78, f32::EPSILON),
            (&s32le, Fmt::S32LE, 1, 0, 1.0, f32::EPSILON),
            (&s8, Fmt::S8, 2, 0, i8::MIN as f32 / i8::MAX as f32, 1e-5),
            (&s8, Fmt::S8, 2, 1, 1.0, f32::EPSILON),
        ] {
            assert_sample(bytes, fmt, len, index, expected, eps);
        }
        assert!(converted_sample_count(&[0u8; 4], Fmt::Unknown).is_none());
        assert!(converted_sample_count(&[0u8; 3], Fmt::S16LE).is_none());
    }
}
