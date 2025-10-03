//! PipeWire virtual sink integration for OpenMeters.

use super::ring_buffer::RingBuffer;
use crate::util::{bytes_per_sample, convert_samples_to_f32};
use pipewire as pw;
use pw::{properties::properties, spa};
use spa::pod::Pod;
use std::error::Error;
use std::io::Cursor;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

/// Default sample rate advertised to PipeWire peers.
const VIRTUAL_SINK_SAMPLE_RATE: u32 = 48_000;
/// Default channel count advertised to PipeWire peers.
const VIRTUAL_SINK_CHANNELS: u32 = 2;
/// Number of audio frames we keep in memory for downstream consumers.
const CAPTURE_BUFFER_CAPACITY: usize = 256;

static SINK_THREAD: OnceLock<thread::JoinHandle<()>> = OnceLock::new();
static CAPTURE_BUFFER: OnceLock<Arc<Mutex<RingBuffer<Vec<f32>>>>> = OnceLock::new();

/// Spawn the Virtual sink in a background thread.
/// Persists for the lifetime of the application.
pub fn run() {
    ensure_capture_buffer();

    if SINK_THREAD.get().is_some() {
        return;
    }

    match thread::Builder::new()
        .name("openmeters-pw-virtual-sink".into())
        .spawn(|| {
            if let Err(err) = run_virtual_sink() {
                eprintln!("[virtual-sink] stopped: {err}");
            }
        }) {
        Ok(handle) => {
            if SINK_THREAD.set(handle).is_err() {
                // Another caller raced us; the thread will keep running but we can drop our handle.
            }
        }
        Err(err) => eprintln!("[virtual-sink] failed to start PipeWire thread: {err}"),
    }
}

/// Accessor for the shared ring buffer that stores captured audio frames.
pub fn capture_buffer_handle() -> Arc<Mutex<RingBuffer<Vec<f32>>>> {
    ensure_capture_buffer().clone()
}

/// Cached parameters describing the negotiated stream format.
struct VirtualSinkState {
    frame_bytes: usize,
    channels: u32,
    sample_rate: u32,
    format: spa::param::audio::AudioFormat,
}

impl VirtualSinkState {
    /// Construct a new state using the requested channel count and sample rate.
    fn new(channels: u32, sample_rate: u32) -> Self {
        let default_format = spa::param::audio::AudioFormat::F32LE;
        let sample_bytes = bytes_per_sample(default_format).unwrap_or(std::mem::size_of::<f32>());
        let frame_bytes = channels.max(1) as usize * sample_bytes;
        Self {
            frame_bytes,
            channels,
            sample_rate,
            format: default_format,
        }
    }

    /// Update the cached values based on the negotiated PipeWire audio info.
    fn update_from_info(&mut self, info: &spa::param::audio::AudioInfoRaw) {
        self.channels = info.channels().max(1);
        self.sample_rate = info.rate();
        self.format = info.format();
        if let Some(sample_bytes) = bytes_per_sample(self.format) {
            self.frame_bytes = self.channels as usize * sample_bytes;
        } else {
            eprintln!(
                "[virtual-sink] unsupported audio format {:?}; falling back to recorded frame size",
                self.format
            );
        }
        println!(
            "[virtual-sink] negotiated format: {:?}, rate {} Hz, channels {}",
            info.format(),
            self.sample_rate,
            self.channels
        );
    }
}

/// PipeWire main loop body that registers and services the virtual sink.
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
            *pw::keys::AUDIO_CHANNELS => VIRTUAL_SINK_CHANNELS.to_string(),
        },
    )?;

    let audio_state = VirtualSinkState::new(VIRTUAL_SINK_CHANNELS, VIRTUAL_SINK_SAMPLE_RATE);
    let capture_buffer = capture_buffer_handle();

    let _listener = stream
        .add_local_listener_with_user_data(audio_state)
        .state_changed(|_, _, previous, current| {
            println!("[virtual-sink] state {previous:?} -> {current:?}");
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
                eprintln!("[virtual-sink] no buffer available to dequeue");
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

                let mut captured = None;
                if let Some(slice) = data.data() {
                    let len = used.min(slice.len());
                    captured = convert_samples_to_f32(&slice[..len], state.format);
                }

                if let Some(samples) = captured {
                    if let Ok(mut ring_buffer) = capture_buffer.try_lock() {
                        let _ = ring_buffer.push(samples);
                    }
                }

                let chunk_mut = data.chunk_mut();
                *chunk_mut.offset_mut() = 0;
                *chunk_mut.size_mut() = used as u32;
                *chunk_mut.stride_mut() = state.frame_bytes as i32;
            }
            // Buffer is returned to PipeWire when it drops; keep scope tight so this happens promptly.
            drop(buffer);
        })
        .register()?;

    let format_bytes = build_format_pod(VIRTUAL_SINK_CHANNELS, VIRTUAL_SINK_SAMPLE_RATE)?;
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
        eprintln!("[virtual-sink] failed to activate stream: {err}");
    }

    println!("[virtual-sink] PipeWire sink active");
    mainloop.run();
    println!("[virtual-sink] main loop exited");

    Ok(())
}

/// Describe the desired raw audio format as a SPA pod for negotiation.
fn build_format_pod(channels: u32, rate: u32) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(rate);
    info.set_channels(channels);

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

fn ensure_capture_buffer() -> &'static Arc<Mutex<RingBuffer<Vec<f32>>>> {
    CAPTURE_BUFFER.get_or_init(|| {
        Arc::new(Mutex::new(RingBuffer::with_capacity(
            CAPTURE_BUFFER_CAPACITY,
        )))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_buffer_is_singleton_with_expected_capacity() {
        let first = ensure_capture_buffer();
        let second = ensure_capture_buffer();
        assert!(Arc::ptr_eq(first, second));

        let guard = first.lock().expect("capture buffer lock poisoned");
        assert_eq!(guard.capacity(), CAPTURE_BUFFER_CAPACITY);
    }

    #[test]
    fn virtual_sink_state_defaults_match_requested_configuration() {
        let state = VirtualSinkState::new(2, 48_000);
        assert_eq!(state.channels, 2);
        assert_eq!(state.sample_rate, 48_000);
        assert_eq!(state.format, spa::param::audio::AudioFormat::F32LE);
        assert_eq!(
            state.frame_bytes,
            2 * bytes_per_sample(state.format).unwrap()
        );
    }

    #[test]
    fn update_from_info_clamps_channels_and_updates_frame_bytes() {
        let mut state = VirtualSinkState::new(4, 96_000);
        let mut info = spa::param::audio::AudioInfoRaw::new();
        info.set_channels(0); // PipeWire may report 0 before negotiation; we clamp to at least 1.
        info.set_rate(44_100);
        info.set_format(spa::param::audio::AudioFormat::S16LE);

        state.update_from_info(&info);

        assert_eq!(
            state.channels, 1,
            "channels should be clamped to at least 1"
        );
        assert_eq!(state.sample_rate, 44_100);
        assert_eq!(state.format, spa::param::audio::AudioFormat::S16LE);
        assert_eq!(state.frame_bytes, bytes_per_sample(state.format).unwrap());
    }
}
