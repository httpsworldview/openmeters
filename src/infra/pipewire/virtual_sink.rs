// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::VIRTUAL_SINK_NAME;
use crate::util::audio::DEFAULT_SAMPLE_RATE;
use pipewire as pw;
use pw::{properties::properties, spa};
use spa::pod::Pod;
use std::collections::VecDeque;
use std::error::Error;
use std::io::Cursor;
use std::mem::size_of;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tracing::{error, info, warn};

const VIRTUAL_SINK_SAMPLE_RATE: u32 = DEFAULT_SAMPLE_RATE as u32;
const CAPTURE_BUFFER_CAPACITY: usize = 256;
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
    capacity: usize,
    available: Condvar,
    dropped_frames: AtomicU64,
}

impl CaptureBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
            available: Condvar::new(),
            dropped_frames: AtomicU64::new(0),
        }
    }

    pub fn try_push(&self, frame: CapturedAudio) {
        let Ok(mut guard) = self.inner.try_lock() else {
            self.dropped_frames.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if guard.len() >= self.capacity {
            guard.pop_front();
            self.dropped_frames.fetch_add(1, Ordering::Relaxed);
        }
        guard.push_back(frame);
        self.available.notify_one();
    }

    pub fn pop_wait_timeout(&self, timeout: Duration) -> Result<Option<CapturedAudio>, ()> {
        let mut guard = self.inner.lock().map_err(|_| {
            error!("[virtual-sink] capture buffer lock poisoned");
        })?;

        if guard.is_empty() && !timeout.is_zero() {
            let (new_guard, _) = self
                .available
                .wait_timeout_while(guard, timeout, |queue| queue.is_empty())
                .map_err(|_| ())?;
            guard = new_guard;
        }

        Ok(guard.pop_front())
    }

    pub fn dropped_frames(&self) -> u64 {
        self.dropped_frames.load(Ordering::Relaxed)
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
    let start = offset as usize;
    let end = start.saturating_add(size as usize).min(bytes.len());
    let len = end.checked_sub(start)? / frame_bytes * frame_bytes;
    (len > 0).then(|| &bytes[start..start + len])
}

fn convert_samples_to_f32(
    bytes: &[u8],
    format: spa::param::audio::AudioFormat,
) -> Option<Vec<f32>> {
    use spa::param::audio::AudioFormat as Fmt;

    let sample_bytes = bytes_per_sample(format)?;
    if !bytes.len().is_multiple_of(sample_bytes) {
        warn!(
            "[virtual-sink] buffer length {} is not aligned to {:?}",
            bytes.len(),
            format
        );
        return None;
    }

    let little_endian = matches!(
        format,
        Fmt::F32LE | Fmt::F64LE | Fmt::S16LE | Fmt::S24_32LE | Fmt::S32LE | Fmt::U16LE | Fmt::U32LE
    );
    macro_rules! convert {
        ($ty:ty, $to_f32:expr) => {{
            bytes
                .chunks_exact(size_of::<$ty>())
                .map(|chunk| {
                    let mut bytes = [0; size_of::<$ty>()];
                    bytes.copy_from_slice(chunk);
                    let raw = if little_endian {
                        <$ty>::from_le_bytes(bytes)
                    } else {
                        <$ty>::from_be_bytes(bytes)
                    };
                    $to_f32(raw)
                })
                .collect()
        }};
    }
    const I16_DIV: f32 = i16::MAX as f32;
    const I32_DIV: f32 = i32::MAX as f32;

    Some(match format {
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
        Fmt::U8 => bytes.iter().map(|&b| (b as f32 - 128.0) / 128.0).collect(),
        Fmt::S8 => bytes
            .iter()
            .map(|&b| (b as i8) as f32 / i8::MAX as f32)
            .collect(),
        _ => return None,
    })
}

pub fn run() {
    ensure_capture_buffer();

    let mut sink_thread = SINK_THREAD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
                warn!("[virtual-sink] no buffer available to dequeue");
                return;
            };

            for data in buffer.datas_mut() {
                let chunk = data.chunk();
                let (offset, size) = (chunk.offset(), chunk.size());

                if size == 0 {
                    continue;
                }

                if let Some(samples) = data
                    .data()
                    .and_then(|bytes| audio_chunk(bytes, offset, size, state.frame_bytes))
                    .and_then(|bytes| convert_samples_to_f32(bytes, state.format))
                {
                    capture_buffer.try_push(CapturedAudio {
                        samples,
                        channels: state.channels,
                        sample_rate: state.sample_rate,
                    });
                }

                let chunk_mut = data.chunk_mut();
                *chunk_mut.offset_mut() = 0;
                *chunk_mut.size_mut() = size;
                *chunk_mut.stride_mut() = state.frame_bytes as i32;
            }
        })
        .register()?;

    let format_bytes = build_format_pod(VIRTUAL_SINK_SAMPLE_RATE)?;
    let mut params = [Pod::from_bytes(&format_bytes).ok_or(pw::Error::CreationFailed)?];

    stream.connect(
        spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;

    if let Err(err) = stream.set_active(true) {
        error!("[virtual-sink] failed to activate stream: {err}");
    }

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
        let out = convert_samples_to_f32(bytes, fmt).unwrap();
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
    fn sample_format_conversion() {
        let s16 = [0x00_u8, 0x80, 0xFF, 0x7F];
        assert_sample(
            &s16,
            Fmt::S16LE,
            2,
            0,
            i16::MIN as f32 / i16::MAX as f32,
            1e-5,
        );
        assert_sample(&s16, Fmt::S16LE, 2, 1, 1.0, f32::EPSILON);
        assert_sample(
            &[0x80, 0x00, 0x7F, 0xFF],
            Fmt::S16BE,
            2,
            0,
            i16::MIN as f32 / i16::MAX as f32,
            1e-5,
        );
        assert_sample(
            &[0x00, 0x00, 0xFF, 0xFF],
            Fmt::U16BE,
            2,
            1,
            (u16::MAX as f32 - 32_768.0) / 32_768.0,
            f32::EPSILON,
        );

        let val: f32 = 0.123_456_78;
        assert_sample(&val.to_le_bytes(), Fmt::F32LE, 1, 0, val, f32::EPSILON);
        assert_sample(&i32::MAX.to_le_bytes(), Fmt::S32LE, 1, 0, 1.0, f32::EPSILON);

        assert!(convert_samples_to_f32(&[0u8; 4], Fmt::Unknown).is_none());
        assert!(convert_samples_to_f32(&[0u8; 3], Fmt::S16LE).is_none());
    }
}
