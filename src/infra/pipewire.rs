// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

//! Threaded PipeWire capture with ordered audio delivery.

mod graph;
mod policy;
mod runtime;
mod stream;
mod transport;

#[cfg(test)]
mod live_tests;

use crate::domain::routing::{CaptureConfig, StreamIdentity};
use crate::util::{Notifier, unpoison};
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationView {
    pub identity: StreamIdentity,
    pub label: Arc<str>,
    pub active: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaptureView {
    pub applications: Vec<ApplicationView>,
    pub devices: Vec<Arc<str>>,
    pub default_sink: Option<Arc<str>>,
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
    changes: Notifier,
}

impl PublicState {
    fn set_alive(&self, alive: bool) {
        if self.alive.swap(alive, Ordering::AcqRel) != alive {
            self.changes.notify();
        }
    }

    fn publish(&self, view: CaptureView) {
        let mut current = unpoison(self.view.write());
        if **current != view {
            *current = Arc::new(view);
            self.changes.notify();
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
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self {
            commands: mpsc::channel().0,
            public: Arc::default(),
        }
    }

    pub fn configure(&self, config: CaptureConfig) -> bool {
        self.commands.send(Command::Configure(config)).is_ok()
    }

    pub fn view(&self) -> Arc<CaptureView> {
        self.public.view()
    }

    pub fn is_alive(&self) -> bool {
        self.public.alive.load(Ordering::Acquire)
    }

    pub fn changes(&self) -> Notifier {
        self.public.changes.clone()
    }
}

pub struct AudioBackend {
    commands: mpsc::Sender<Command>,
    thread: Option<thread::JoinHandle<()>>,
}

impl AudioBackend {
    pub fn start(config: CaptureConfig) -> io::Result<(Self, CaptureControl, AudioReader)> {
        Self::start_with_socket(config, None)
    }

    fn start_with_socket(
        config: CaptureConfig,
        socket: Option<PathBuf>,
    ) -> io::Result<(Self, CaptureControl, AudioReader)> {
        let (writer, audio) = transport::channel();
        let initial = policy::plan(&Default::default(), &config, None);
        writer.publish_layout(initial.stream.layout);
        let (commands, receiver) = mpsc::channel();
        let public = Arc::new(PublicState::default());
        let control = CaptureControl {
            commands: commands.clone(),
            public: Arc::clone(&public),
        };
        let thread = thread::Builder::new()
            .name("openmeters-pipewire".into())
            .spawn(move || runtime::run(receiver, config, writer, public, socket))?;

        Ok((
            Self {
                commands,
                thread: Some(thread),
            },
            control,
            audio,
        ))
    }

    pub fn shutdown(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        let _ = self.commands.send(Command::Shutdown);
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
    use std::time::Instant;

    #[test]
    fn unavailable_socket_faults_and_stops_cleanly() {
        let runtime = tempfile::tempdir().unwrap();
        let (mut backend, control, mut audio) = AudioBackend::start_with_socket(
            CaptureConfig::default(),
            Some(runtime.path().join("missing")),
        )
        .unwrap();
        let (mut reset, mut seeded) = (false, false);
        live_tests::wait_for("the backend to fault and seed silence", || {
            audio.drain(Instant::now(), |span| match span {
                CapturedSpan::Reset => reset = true,
                CapturedSpan::Silence { format, .. } => seeded |= format.channels() == 8,
                CapturedSpan::Pcm { .. } => {}
            });
            (reset && seeded).then_some(())
        });
        assert!(!control.is_alive());
        backend.shutdown();
    }

    #[test]
    fn publication_and_liveness_notify_only_on_change() {
        let state = PublicState::default();
        let notified = || state.changes.receiver().try_recv().is_ok();
        let initial = state.view();
        state.publish(CaptureView::default());
        assert!(Arc::ptr_eq(&initial, &state.view()));
        assert!(!notified());
        state.publish(CaptureView {
            default_sink: Some("sink".into()),
            ..Default::default()
        });
        let changed = state.view();
        assert!(!Arc::ptr_eq(&initial, &changed));
        assert!(notified());
        state.publish((*changed).clone());
        assert!(Arc::ptr_eq(&changed, &state.view()));
        for (alive, change) in [(true, true), (true, false), (false, true)] {
            state.set_alive(alive);
            assert_eq!(notified(), change);
        }
    }
}
