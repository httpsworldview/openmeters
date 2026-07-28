// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::graph::{CaptureLayout, Channel};
use super::transport::{CaptureWriter, PcmChunk, StreamStatus};
use super::{MAX_CAPTURE_CHANNELS, MAX_CAPTURE_SAMPLE_RATE};
use crate::util::audio::DEFAULT_SAMPLE_RATE;
use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use spa::buffer::ChunkFlags;
use spa::buffer::meta::{MetaHeader, MetaHeaderFlags};
use spa::pod::Pod;
use std::cell::{Cell, RefCell};
use std::error::Error;
use std::io::{self, Cursor};
use std::mem::size_of;
use std::rc::Rc;
use tracing::{error, info};

const DESCRIPTION: &str = "OpenMeters Audio Tap";
const LATENCY_FRAMES: u32 = 256;
const EMPTY: i32 = spa::sys::SPA_CHUNK_FLAG_EMPTY as i32;

type DynError = Box<dyn Error + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StreamConfig {
    pub layout: CaptureLayout,
    pub target: Option<String>,
    pub passive: bool,
}

impl StreamConfig {
    pub(super) fn idle() -> Self {
        Self {
            layout: CaptureLayout::stereo(),
            target: None,
            passive: true,
        }
    }
}

pub(super) struct TapStream {
    core: pw::core::CoreRc,
    writer: Rc<RefCell<CaptureWriter>>,
    node_name: String,
    dirty: Rc<Cell<bool>>,
    session: Option<Session>,
    config: Option<StreamConfig>,
}

impl TapStream {
    pub(super) fn new(
        core: pw::core::CoreRc,
        writer: Rc<RefCell<CaptureWriter>>,
        node_name: String,
        dirty: Rc<Cell<bool>>,
    ) -> Self {
        Self {
            core,
            writer,
            node_name,
            dirty,
            session: None,
            config: None,
        }
    }

    pub(super) fn configure(&mut self, config: StreamConfig) -> Result<(), DynError> {
        self.session = None;
        self.config = None;
        let mut writer = self.writer.borrow_mut();
        writer.clear_format();
        let layout = &config.layout.channels;
        let positions = std::array::from_fn(|index| layout.get(index).copied().unwrap_or_default());
        writer.publish_format(layout.len(), DEFAULT_SAMPLE_RATE as u32, positions);
        writer.set_status(StreamStatus::Starting);
        drop(writer);
        self.session = Some(Session::new(
            self.core.clone(),
            Rc::clone(&self.writer),
            &self.node_name,
            &config,
            Rc::clone(&self.dirty),
        )?);
        self.writer.borrow().mark_reconnect();
        self.config = Some(config);
        Ok(())
    }

    pub(super) fn node_id(&self) -> Option<u32> {
        let id = self.session.as_ref()?.stream.node_id();
        (id != pw::constants::ID_ANY).then_some(id)
    }

    pub(super) fn config(&self) -> Option<&StreamConfig> {
        self.config.as_ref()
    }

    pub(super) fn status(&self) -> Option<StreamStatus> {
        self.session.as_ref().map(|_| self.writer.borrow().status())
    }

    pub(super) fn clear_failed(&mut self) {
        self.session = None;
        self.config = None;
        self.writer.borrow_mut().disconnect();
    }
}

impl Drop for TapStream {
    fn drop(&mut self) {
        self.writer.borrow_mut().set_status(StreamStatus::Stopped);
    }
}

struct Session {
    _listener: pw::stream::StreamListener<Rc<RefCell<CaptureWriter>>>,
    stream: pw::stream::StreamRc,
}

impl Session {
    fn new(
        core: pw::core::CoreRc,
        writer: Rc<RefCell<CaptureWriter>>,
        node_name: &str,
        config: &StreamConfig,
        dirty: Rc<Cell<bool>>,
    ) -> Result<Self, DynError> {
        let mut props = properties! {
            *pw::keys::MEDIA_CLASS => "Stream/Input/Audio",
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Production",
            *pw::keys::NODE_NAME => node_name,
            *pw::keys::NODE_DESCRIPTION => DESCRIPTION,
            *pw::keys::NODE_VIRTUAL => "true",
            *pw::keys::NODE_LATENCY => format!("{LATENCY_FRAMES}/{}", DEFAULT_SAMPLE_RATE as u32),
            *pw::keys::NODE_ALWAYS_PROCESS => if !config.passive && config.target.is_none() { "true" } else { "false" },
            *pw::keys::NODE_PASSIVE => if config.passive { "in" } else { "false" },
            *pw::keys::NODE_AUTOCONNECT => if config.target.is_some() { "true" } else { "false" },
            *pw::keys::APP_NAME => "OpenMeters",
        };
        if let Some(target) = &config.target {
            props.insert(*pw::keys::TARGET_OBJECT, target.clone());
            props.insert(*pw::keys::NODE_DONT_RECONNECT, "true");
            props.insert("node.dont-fallback", "true");
            props.insert("node.dont-move", "true");
            props.insert(
                *pw::keys::STREAM_CAPTURE_SINK,
                if config.passive { "true" } else { "false" },
            );
        }

        let stream = pw::stream::StreamRc::new(core, DESCRIPTION, props)?;
        let state_dirty = Rc::clone(&dirty);
        let format_dirty = dirty;
        let listener = stream
            .add_local_listener_with_user_data(writer)
            .state_changed(move |_, writer, old, new| {
                let status = match &new {
                    pw::stream::StreamState::Streaming => StreamStatus::Streaming,
                    pw::stream::StreamState::Paused => StreamStatus::Paused,
                    pw::stream::StreamState::Error(_) => StreamStatus::Failed,
                    pw::stream::StreamState::Unconnected => StreamStatus::Stopped,
                    pw::stream::StreamState::Connecting => StreamStatus::Starting,
                };
                writer.borrow_mut().set_status(status);
                state_dirty.set(true);
                if let pw::stream::StreamState::Error(message) = &new {
                    error!("[capture] stream error after {old:?}: {message}");
                } else {
                    info!("[capture] stream state {old:?} -> {new:?}");
                }
            })
            .param_changed(move |_, writer, id, param| {
                if id != spa::param::ParamType::Format.as_raw() {
                    return;
                }
                format_dirty.set(true);
                handle_format_change(writer, param);
            })
            .process(|stream, writer| process(stream, &mut writer.borrow_mut()))
            .register()?;

        let format = format_pod(&config.layout)?;
        let mut params =
            [Pod::from_bytes(&format).ok_or_else(|| io::Error::other("invalid format pod"))?];
        let flags = pw::stream::StreamFlags::MAP_BUFFERS
            | config
                .target
                .as_ref()
                .map_or(pw::stream::StreamFlags::empty(), |_| {
                    pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::DONT_RECONNECT
                });
        stream.connect(spa::utils::Direction::Input, None, flags, &mut params)?;
        stream.set_active(true)?;
        Ok(Self {
            _listener: listener,
            stream,
        })
    }
}

fn handle_format_change(writer: &RefCell<CaptureWriter>, param: Option<&Pod>) {
    let mut writer = writer.borrow_mut();
    let Some(param) = param else {
        writer.clear_format();
        return;
    };
    let mut info = spa::param::audio::AudioInfoRaw::new();
    let valid = info.parse(param).is_ok()
        && info.format() == native_f32()
        && (1..=MAX_CAPTURE_SAMPLE_RATE).contains(&info.rate())
        && (1..=MAX_CAPTURE_CHANNELS as u32).contains(&info.channels());
    if !valid {
        writer.clear_format();
        writer.set_status(StreamStatus::Failed);
        error!("[capture] rejected negotiated format: {info:?}");
        return;
    }
    let negotiated = info.position();
    let positions = std::array::from_fn(|index| Channel::from_spa_id(negotiated[index]));
    let format = writer.set_format(info.channels() as usize, info.rate(), positions);
    info!(
        "[capture] F32NE {} Hz, {} channel(s), generation {}",
        format.sample_rate, format.channels, format.generation
    );
}

fn process(stream: &pw::stream::Stream, writer: &mut CaptureWriter) {
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let header = buffer
        .find_meta::<MetaHeader>()
        .map_or_else(MetaHeaderFlags::empty, MetaHeader::flags);
    let Some(data) = buffer.datas_mut().first_mut() else {
        return;
    };
    let (offset, size, stride, flags) = {
        let chunk = data.chunk();
        (chunk.offset(), chunk.size(), chunk.stride(), chunk.flags())
    };
    let Some(channels) = writer.channels() else {
        return;
    };
    let frame_bytes = channels * size_of::<f32>();
    if stride != 0 && usize::try_from(stride).ok() != Some(frame_bytes) {
        writer.push_fault(size as u64 / frame_bytes as u64);
        return;
    }
    let Some(chunk) = PcmChunk::new(data.as_raw().maxsize as usize, offset, size, frame_bytes)
    else {
        return;
    };
    let frames = (chunk.len() / frame_bytes) as u64;
    match classify(flags, header) {
        BufferKind::Silence => writer.push_silence(frames),
        BufferKind::Fault => writer.push_fault(frames),
        BufferKind::Pcm => match data.data() {
            Some(bytes) => writer.push_pcm(bytes, &chunk, frames),
            None => writer.push_fault(frames),
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BufferKind {
    Pcm,
    Silence,
    Fault,
}

fn classify(chunk: ChunkFlags, header: MetaHeaderFlags) -> BufferKind {
    if chunk.contains(ChunkFlags::CORRUPTED)
        || header.intersects(MetaHeaderFlags::CORRUPTED | MetaHeaderFlags::DISCONT)
    {
        BufferKind::Fault
    } else if chunk.bits() & EMPTY != 0 || header.contains(MetaHeaderFlags::GAP) {
        BufferKind::Silence
    } else {
        BufferKind::Pcm
    }
}

fn format_pod(layout: &CaptureLayout) -> Result<Vec<u8>, DynError> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(native_f32());
    info.set_channels(layout.channels.len() as u32);
    info.set_position(layout.spa_positions());
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    Ok(spa::pod::serialize::PodSerializer::serialize(
        Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )?
    .0
    .into_inner())
}

const fn native_f32() -> spa::param::audio::AudioFormat {
    #[cfg(target_endian = "little")]
    {
        spa::param::audio::AudioFormat::F32LE
    }
    #[cfg(target_endian = "big")]
    {
        spa::param::audio::AudioFormat::F32BE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_metadata_distinguishes_pcm_silence_and_faults() {
        assert_eq!(
            classify(ChunkFlags::empty(), MetaHeaderFlags::empty()),
            BufferKind::Pcm
        );
        assert_eq!(
            classify(ChunkFlags::empty(), MetaHeaderFlags::GAP),
            BufferKind::Silence
        );
        assert_eq!(
            classify(ChunkFlags::empty(), MetaHeaderFlags::DISCONT),
            BufferKind::Fault
        );
        assert_eq!(
            classify(ChunkFlags::CORRUPTED, MetaHeaderFlags::GAP),
            BufferKind::Fault
        );
    }
}
