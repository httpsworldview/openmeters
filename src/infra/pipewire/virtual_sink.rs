// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::util::audio::DEFAULT_SAMPLE_RATE;
use pipewire as pw;
use pw::{properties::properties, spa};
use spa::pod::Pod;
use std::collections::VecDeque;
use std::convert::TryInto;
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

static SINK_THREAD: OnceLock<thread::JoinHandle<()>> = OnceLock::new();
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
        match self.inner.try_lock() {
            Ok(mut guard) => {
                if guard.len() >= self.capacity {
                    guard.pop_front();
                    self.dropped_frames.fetch_add(1, Ordering::Relaxed);
                }
                guard.push_back(frame);
                self.available.notify_one();
            }
            Err(_) => {
                self.dropped_frames.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn pop_wait_timeout(&self, timeout: Duration) -> Result<Option<CapturedAudio>, ()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| {
                error!("[virtual-sink] capture buffer lock poisoned");
                e.into_inner()
            })
            .map_err(|_| ())?;

        if let Some(frame) = guard.pop_front() {
            return Ok(Some(frame));
        }
        if timeout.is_zero() {
            return Ok(None);
        }

        loop {
            let (new_guard, result) = self
                .available
                .wait_timeout(guard, timeout)
                .map_err(|_| ())?;
            guard = new_guard;

            if let Some(frame) = guard.pop_front() {
                return Ok(Some(frame));
            }
            if result.timed_out() {
                return Ok(None);
            }
        }
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

    let sample_count = bytes.len() / sample_bytes;
    let mut samples = Vec::with_capacity(sample_count);

    macro_rules! convert {
        ($ty:ty, $from:ident, $to_f32:expr) => {{
            for chunk in bytes.chunks_exact(std::mem::size_of::<$ty>()) {
                samples.push($to_f32(<$ty>::$from(chunk.try_into().unwrap())));
            }
        }};
    }
    const I16_DIV: f32 = i16::MAX as f32;
    const I32_DIV: f32 = i32::MAX as f32;

    match format {
        Fmt::F32LE => convert!(f32, from_le_bytes, |v: f32| v),
        Fmt::F32BE => convert!(f32, from_be_bytes, |v: f32| v),
        Fmt::F64LE => convert!(f64, from_le_bytes, |v: f64| v as f32),
        Fmt::F64BE => convert!(f64, from_be_bytes, |v: f64| v as f32),
        Fmt::S16LE => convert!(i16, from_le_bytes, |v: i16| v as f32 / I16_DIV),
        Fmt::S16BE => convert!(i16, from_be_bytes, |v: i16| v as f32 / I16_DIV),
        Fmt::S32LE | Fmt::S24_32LE => convert!(i32, from_le_bytes, |v: i32| v as f32 / I32_DIV),
        Fmt::S32BE | Fmt::S24_32BE => convert!(i32, from_be_bytes, |v: i32| v as f32 / I32_DIV),
        Fmt::U16LE => convert!(u16, from_le_bytes, |v: u16| (v as f32 - 32_768.0)
            / 32_768.0),
        Fmt::U16BE => convert!(u16, from_be_bytes, |v: u16| (v as f32 - 32_768.0)
            / 32_768.0),
        Fmt::U8 => samples.extend(bytes.iter().map(|&b| (b as f32 - 128.0) / 128.0)),
        Fmt::S8 => samples.extend(bytes.iter().map(|&b| (b as i8) as f32 / i8::MAX as f32)),
        _ => return None,
    }

    Some(samples)
}

pub fn run() -> Option<std::thread::JoinHandle<()>> {
    ensure_capture_buffer();

    if SINK_THREAD.get().is_some() {
        return None;
    }

    match thread::Builder::new()
        .name("openmeters-pw-virtual-sink".into())
        .spawn(|| {
            if let Err(err) = run_virtual_sink() {
                error!("[virtual-sink] stopped: {err}");
            }
        }) {
        Ok(handle) => Some(handle),
        Err(err) => {
            error!("[virtual-sink] failed to start PipeWire thread: {err}");
            None
        }
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
        let format = spa::param::audio::AudioFormat::F32LE;
        Self {
            frame_bytes: 2 * bytes_per_sample(format).unwrap_or(size_of::<f32>()),
            channels: 2,
            sample_rate: VIRTUAL_SINK_SAMPLE_RATE,
            format,
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
            *pw::keys::NODE_NAME => "openmeters.sink",
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
                let used = {
                    let chunk = data.chunk();
                    chunk.size() as usize
                };

                if used == 0 {
                    continue;
                }

                if let Some(slice) = data.data()
                    && let Some(samples) =
                        convert_samples_to_f32(&slice[..used.min(slice.len())], state.format)
                {
                    capture_buffer.try_push(CapturedAudio {
                        samples,
                        channels: state.channels,
                        sample_rate: state.sample_rate,
                    });
                }

                let chunk_mut = data.chunk_mut();
                *chunk_mut.offset_mut() = 0;
                *chunk_mut.size_mut() = used as u32;
                *chunk_mut.stride_mut() = state.frame_bytes as i32;
            }
            drop(buffer);
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

    #[test]
    fn sample_format_conversion() {
        // S16LE: i16::MIN -> -1.0, i16::MAX -> +1.0
        let s16_out = convert_samples_to_f32(&[0x00_u8, 0x80, 0xFF, 0x7F], Fmt::S16LE).unwrap();
        assert_eq!(s16_out.len(), 2);
        let expected_min = i16::MIN as f32 / i16::MAX as f32;
        let s16_min = s16_out[0];
        let s16_max = s16_out[1];
        assert!(
            (s16_min - expected_min).abs() < 1e-5,
            "S16LE min: {s16_min} vs {expected_min}"
        );
        assert!((s16_max - 1.0).abs() < f32::EPSILON, "S16LE max: {s16_max}");

        // F32LE: passthrough preserves exact values
        let val: f32 = 0.123_456_78;
        let f32_out = convert_samples_to_f32(&val.to_le_bytes(), Fmt::F32LE).unwrap();
        assert_eq!(f32_out.len(), 1);
        let f32_result = f32_out[0];
        assert!(
            (f32_result - val).abs() < f32::EPSILON,
            "F32LE passthrough: {f32_result} vs {val}"
        );

        // S32LE: verify normalization range
        let s32_out = convert_samples_to_f32(&i32::MAX.to_le_bytes(), Fmt::S32LE).unwrap();
        let s32_result = s32_out[0];
        assert!(
            (s32_result - 1.0).abs() < f32::EPSILON,
            "S32LE max: {s32_result}"
        );

        // Unsupported format returns None; misaligned buffer returns None
        assert!(convert_samples_to_f32(&[0u8; 4], Fmt::Unknown).is_none());
        assert!(convert_samples_to_f32(&[0u8; 3], Fmt::S16LE).is_none());
    }
}
