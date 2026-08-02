// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

//! Owned PipeWire graph tap and its two narrow cross-thread interfaces.

mod graph;
mod policy;
mod runtime;
mod stream;
mod transport;

#[cfg(test)]
mod live_tests;

use crate::domain::routing::{CaptureConfig, CaptureMode, StreamIdentity};
use crate::dsp::ChannelPosition;
use crate::util::{audio::DEFAULT_SAMPLE_RATE, unpoison};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock, mpsc};
use std::thread;
use tracing::{error, info};

pub use transport::{AudioReader, CapturedSpan};

type DynError = Box<dyn std::error::Error + Send + Sync>;

#[cfg(test)]
pub(crate) fn test_audio_reader() -> AudioReader {
    transport::channel().1
}

pub(crate) const MAX_CAPTURE_CHANNELS: usize = crate::dsp::MAX_AUDIO_CHANNELS;
const MAX_CAPTURE_SAMPLE_RATE: u32 = crate::util::audio::MAX_SAMPLE_RATE as u32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationView {
    pub identity: StreamIdentity,
    pub label: Arc<str>,
    pub active: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaptureView {
    pub applications: Arc<[ApplicationView]>,
    pub devices: Arc<[Arc<str>]>,
    pub default_sink: Arc<str>,
    pub selected_device: Option<Arc<str>>,
}

enum Command {
    Configure(CaptureConfig),
    Shutdown,
}

#[derive(Default)]
struct PublicState {
    alive: AtomicBool,
    view: RwLock<Arc<CaptureView>>,
}

impl PublicState {
    fn publish(&self, view: CaptureView) {
        let mut current = unpoison(self.view.write());
        if **current != view {
            *current = Arc::new(view);
        }
    }

    fn view(&self) -> Arc<CaptureView> {
        unpoison(self.view.read()).clone()
    }
}

#[derive(Clone)]
pub struct CaptureControl {
    commands: mpsc::Sender<Command>,
    public: Arc<PublicState>,
}

impl CaptureControl {
    pub fn configure(&self, config: CaptureConfig) -> bool {
        self.commands.send(Command::Configure(config)).is_ok()
    }

    pub fn view(&self) -> Arc<CaptureView> {
        self.public.view()
    }

    pub fn is_alive(&self) -> bool {
        self.public.alive.load(Ordering::Acquire)
    }
}

pub struct AudioBackend {
    control: CaptureControl,
    audio: Option<AudioReader>,
    thread: Option<thread::JoinHandle<()>>,
}

impl AudioBackend {
    pub fn start(config: CaptureConfig) -> io::Result<Self> {
        Self::start_with_socket(config, None)
    }

    fn start_with_socket(config: CaptureConfig, socket: Option<PathBuf>) -> io::Result<Self> {
        let (writer, audio) = transport::channel();
        let (channels, positions) = match config.mode {
            CaptureMode::Applications => (MAX_CAPTURE_CHANNELS, ChannelPosition::SURROUND),
            CaptureMode::Device => (2, ChannelPosition::fallback(2)),
        };
        writer.publish_format(channels, DEFAULT_SAMPLE_RATE as u32, positions);
        let (commands, receiver) = mpsc::channel();
        let public = Arc::new(PublicState::default());
        let control = CaptureControl {
            commands,
            public: Arc::clone(&public),
        };
        let thread = thread::Builder::new()
            .name("openmeters-pipewire".into())
            .spawn(move || runtime::run(receiver, config, writer, public, socket))?;

        Ok(Self {
            control,
            audio: Some(audio),
            thread: Some(thread),
        })
    }

    pub fn control(&self) -> CaptureControl {
        self.control.clone()
    }

    pub fn take_audio(&mut self) -> AudioReader {
        self.audio.take().expect("audio reader already taken")
    }

    pub fn shutdown(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        let _ = self.control.commands.send(Command::Shutdown);
        if thread.join().is_err() {
            error!("[pipewire] backend thread panicked");
        }
        info!("[pipewire] backend stopped");
    }
}

impl Drop for AudioBackend {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn unavailable_socket_faults_and_stops_cleanly() {
        let runtime = tempfile::tempdir().unwrap();
        let mut backend = AudioBackend::start_with_socket(
            CaptureConfig::default(),
            Some(runtime.path().join("missing")),
        )
        .unwrap();
        let control = backend.control();
        let mut audio = backend.take_audio();
        let deadline = Instant::now() + Duration::from_secs(1);
        let (mut reset, mut seeded) = (false, false);
        while !(reset && seeded) && Instant::now() < deadline {
            audio.drain(Instant::now(), |span| match span {
                CapturedSpan::Reset => reset = true,
                CapturedSpan::Silence { format, .. } => seeded |= format.channels == 8,
                CapturedSpan::Pcm { .. } => {}
            });
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(reset && seeded);
        assert!(!control.is_alive());
        backend.shutdown();
    }

    #[test]
    fn view_publication_changes_only_with_visible_content() {
        let state = PublicState::default();
        let initial = state.view();
        state.publish(CaptureView::default());
        assert!(Arc::ptr_eq(&initial, &state.view()));
        state.publish(CaptureView {
            default_sink: "sink".into(),
            ..Default::default()
        });
        let changed = state.view();
        assert!(!Arc::ptr_eq(&initial, &changed));
        state.publish((*changed).clone());
        assert!(Arc::ptr_eq(&changed, &state.view()));
    }
}
