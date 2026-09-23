// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::graph::{Graph, GraphLink, Node, Port};
use super::policy::{self, LinkSpec};
use super::stream::TapStream;
use super::transport::{CaptureWriter, StreamStatus};
use super::{Command, DynError, PublicState};
use crate::domain::routing::CaptureConfig;
use pipewire as pw;
use pw::metadata::{Metadata, MetadataListener};
use pw::properties::properties;
use pw::registry::{GlobalObject, RegistryRc};
use pw::spa::utils::dict::DictRef;
use pw::types::ObjectType;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, hash_map::Entry};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, atomic::Ordering, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

const DEFAULT_METADATA: &str = "default";
const LINK_FACTORY: &str = "link-factory";
const ITERATION_TIMEOUT: Duration = Duration::from_millis(20);
const SESSION_RETRY_MIN: Duration = Duration::from_millis(250);
const SESSION_RETRY_MAX: Duration = Duration::from_secs(8);
const RESOURCE_RETRY_MIN: Duration = Duration::from_secs(1);
const RESOURCE_RETRY_MAX: Duration = Duration::from_secs(30);
const MAX_LOOP_ERRORS: u32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Retry {
    Timeout,
    Configured,
    Stop,
}

fn wait_for_retry(
    commands: &mpsc::Receiver<Command>,
    config: &mut CaptureConfig,
    timeout: Duration,
) -> Retry {
    let mut next = match commands.recv_timeout(timeout) {
        Ok(Command::Configure(next)) => next,
        Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => return Retry::Stop,
        Err(mpsc::RecvTimeoutError::Timeout) => return Retry::Timeout,
    };
    loop {
        match commands.try_recv() {
            Ok(Command::Configure(newer)) => next = newer,
            Ok(Command::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => return Retry::Stop,
            Err(mpsc::TryRecvError::Empty) => {
                *config = next;
                return Retry::Configured;
            }
        }
    }
}

fn take_retry_delay(delay: &mut Duration, maximum: Duration) -> Duration {
    let current = *delay;
    *delay = delay.saturating_mul(2).min(maximum);
    current
}

fn retry_deadline(now: Instant, delay: &mut Duration) -> Instant {
    now + take_retry_delay(delay, RESOURCE_RETRY_MAX)
}

fn defer_retry(at: &Cell<Option<Instant>>, delay: &Cell<Duration>, now: Instant) -> bool {
    if at.get().is_some_and(|deadline| deadline > now) {
        return false;
    }
    let mut current = delay.get();
    at.set(Some(retry_deadline(now, &mut current)));
    delay.set(current);
    true
}

pub(super) fn run(
    commands: mpsc::Receiver<Command>,
    mut config: CaptureConfig,
    writer: CaptureWriter,
    public: Arc<PublicState>,
    socket: Option<PathBuf>,
) {
    pw::init();
    let writer = Rc::new(RefCell::new(writer));
    let mut retry_delay = SESSION_RETRY_MIN;
    let mut outage = false;
    loop {
        writer.borrow_mut().set_status(StreamStatus::Starting);
        let Err(err) = run_session(
            &commands,
            &mut config,
            Rc::clone(&writer),
            &public,
            socket.as_deref(),
        ) else {
            break;
        };

        if public.alive.load(Ordering::Acquire) {
            retry_delay = SESSION_RETRY_MIN;
            outage = false;
        }
        if outage {
            debug!("[pipewire] reconnect attempt failed: {err}");
        } else {
            error!("[pipewire] backend disconnected: {err}");
            public.publish(Default::default());
            outage = true;
        }
        writer.borrow_mut().disconnect();
        public.alive.store(false, Ordering::Release);

        let wait = take_retry_delay(&mut retry_delay, SESSION_RETRY_MAX);
        match wait_for_retry(&commands, &mut config, wait) {
            Retry::Configured => retry_delay = SESSION_RETRY_MIN,
            Retry::Timeout => {}
            Retry::Stop => break,
        }
    }
    writer.borrow_mut().set_status(StreamStatus::Stopped);
    public.alive.store(false, Ordering::Release);
    info!("[pipewire] backend loop exited");
}

fn run_session(
    commands: &mpsc::Receiver<Command>,
    config: &mut CaptureConfig,
    writer: Rc<RefCell<CaptureWriter>>,
    public: &PublicState,
    socket: Option<&Path>,
) -> Result<(), DynError> {
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = if let Some(socket) = socket {
        context.connect_fd_rc(UnixStream::connect(socket)?.into(), None)?
    } else {
        context.connect_rc(None)?
    };
    let registry = core.get_registry_rc()?;
    let graph = Rc::new(RefCell::new(Graph::default()));
    let dirty = Rc::new(Cell::new(true));

    let registry_context = RegistryContext {
        registry: registry.clone(),
        graph: Rc::clone(&graph),
        dirty: Rc::clone(&dirty),
        links: Rc::default(),
        metadata: Rc::default(),
    };
    let _registry_listener = {
        let added = registry_context.clone();
        let removed = registry_context;
        registry
            .add_listener_local()
            .global(move |global| added.add(global))
            .global_remove(move |id| removed.remove(id))
            .register()
    };

    let synced = Rc::new(Cell::new(false));
    let fatal: Rc<RefCell<Option<String>>> = Rc::default();
    let sequence = core.sync(0)?;
    let _core_listener = {
        let synced = Rc::clone(&synced);
        let fatal = Rc::clone(&fatal);
        core.add_listener_local()
            .done(move |id, done| {
                if id == pw::core::PW_ID_CORE && done == sequence {
                    synced.set(true);
                }
            })
            .error(move |id, sequence, result, message| {
                let detail = format!(
                    "PipeWire core error: object={id}, sequence={sequence}, result={result}, message={message}"
                );
                error!("[pipewire] {detail}");
                if id == pw::core::PW_ID_CORE && fatal.borrow().is_none() {
                    *fatal.borrow_mut() = Some(detail);
                }
            })
            .register()
    };

    let mut capture = CaptureState::new(core, Rc::clone(&writer), dirty);
    let mut errors = 0u32;

    info!("[pipewire] backend session starting");
    loop {
        let result = mainloop
            .loop_()
            .iterate(pw::loop_::Timeout::Finite(ITERATION_TIMEOUT));
        writer.borrow_mut().reclaim_buffers();
        if result < 0 {
            errors += 1;
            warn!("[pipewire] loop iteration failed (errno={})", -result);
            if errors >= MAX_LOOP_ERRORS {
                return Err(std::io::Error::other("PipeWire server stopped responding").into());
            }
            thread::sleep(Duration::from_millis(25 << errors.min(5)));
        } else {
            errors = 0;
        }

        if let Some(message) = fatal.borrow_mut().take() {
            return Err(std::io::Error::new(std::io::ErrorKind::ConnectionAborted, message).into());
        }

        loop {
            match commands.try_recv() {
                Ok(Command::Configure(next)) => {
                    if *config != next {
                        *config = next;
                        capture.configuration_changed();
                    }
                }
                Ok(Command::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        if !synced.get() {
            continue;
        }
        public.alive.store(true, Ordering::Release);

        capture.reconcile(&graph, config, public, Instant::now());
    }
}

struct CaptureState {
    // Links must be dropped before the tap they reference.
    owned_links: OwnedLinks,
    tap: TapStream,
    dirty: Rc<Cell<bool>>,
    stream_retry_at: Option<Instant>,
    stream_retry_delay: Duration,
    reported_truncation: usize,
}

impl CaptureState {
    fn new(
        core: pw::core::CoreRc,
        writer: Rc<RefCell<CaptureWriter>>,
        dirty: Rc<Cell<bool>>,
    ) -> Self {
        Self {
            tap: TapStream::new(
                core.clone(),
                writer,
                format!("openmeters.tap.{}", std::process::id()),
                Rc::clone(&dirty),
            ),
            owned_links: OwnedLinks::new(core, Rc::clone(&dirty)),
            dirty,
            stream_retry_at: None,
            stream_retry_delay: RESOURCE_RETRY_MIN,
            reported_truncation: 0,
        }
    }

    fn configuration_changed(&mut self) {
        self.dirty.set(true);
        self.stream_retry_at = None;
        self.stream_retry_delay = RESOURCE_RETRY_MIN;
    }

    fn defer_stream_retry(&mut self, now: Instant) {
        self.tap.clear_failed();
        self.dirty.set(true);
        self.stream_retry_at = Some(retry_deadline(now, &mut self.stream_retry_delay));
    }

    fn reconcile(
        &mut self,
        graph: &RefCell<Graph>,
        config: &mut CaptureConfig,
        public: &PublicState,
        now: Instant,
    ) {
        match self.tap.status() {
            Some(StreamStatus::Paused | StreamStatus::Streaming) => {
                self.stream_retry_delay = RESOURCE_RETRY_MIN;
            }
            Some(StreamStatus::Failed | StreamStatus::Stopped) => {
                self.owned_links.clear();
                self.defer_stream_retry(now);
            }
            _ => {}
        }

        if !self.dirty.get() && !self.owned_links.retry_due(now) {
            return;
        }
        if self.stream_retry_at.is_some_and(|deadline| now < deadline) {
            return;
        }
        self.dirty.set(false);
        let plan = policy::plan(&graph.borrow(), config, self.tap.node_id());
        if plan.truncated > 0 && plan.truncated != self.reported_truncation {
            warn!(
                "[capture] source layout truncated {} channel(s)",
                plan.truncated
            );
        }
        self.reported_truncation = plan.truncated;

        if self.tap.config() != Some(&plan.stream) {
            self.owned_links.clear();
            if let Err(err) = self.tap.configure(plan.stream.clone()) {
                error!("[capture] stream reconfiguration failed: {err}");
                self.defer_stream_retry(now);
                return;
            }
            self.stream_retry_at = None;
        }

        let graph = graph.borrow();
        let desired = self
            .tap
            .node_id()
            .and_then(|id| graph.node(id))
            .map_or_else(Vec::new, |tap| policy::desired_links(&graph, &plan, tap));
        self.owned_links.apply(desired, now);
        let view = graph.view(self.tap.node_id(), config.device.as_deref());
        if let Some(selected) = &view.selected_device
            && config.device.as_deref() != Some(selected)
        {
            config.device = Some(Arc::clone(selected));
        }
        public.publish(view);
    }
}

struct OwnedLinks {
    core: pw::core::CoreRc,
    links: HashMap<LinkSpec, OwnedLink>,
    desired: Vec<LinkSpec>,
    dirty: Rc<Cell<bool>>,
    retry_at: Rc<Cell<Option<Instant>>>,
    retry_delay: Rc<Cell<Duration>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OwnedLinkState {
    Pending,
    Established,
    Failed,
}

struct OwnedLink {
    state: Rc<Cell<OwnedLinkState>>,
    _listener: pw::link::LinkListener,
    _proxy: pw::link::Link,
}

impl OwnedLinks {
    fn new(core: pw::core::CoreRc, dirty: Rc<Cell<bool>>) -> Self {
        Self {
            core,
            links: HashMap::new(),
            desired: Vec::new(),
            dirty,
            retry_at: Rc::default(),
            retry_delay: Rc::new(Cell::new(RESOURCE_RETRY_MIN)),
        }
    }

    fn clear(&mut self) {
        self.links.clear();
        self.desired.clear();
        self.retry_at.set(None);
        self.retry_delay.set(RESOURCE_RETRY_MIN);
    }

    fn retry_due(&self, now: Instant) -> bool {
        self.retry_at.get().is_some_and(|deadline| now >= deadline)
    }

    fn apply(&mut self, desired: Vec<LinkSpec>, now: Instant) {
        if self.desired != desired {
            self.desired = desired;
            self.retry_at.set(None);
            self.retry_delay.set(RESOURCE_RETRY_MIN);
        }
        self.links.retain(|spec, link| {
            self.desired.binary_search(spec).is_ok() && link.state.get() != OwnedLinkState::Failed
        });
        if self.links.len() == self.desired.len()
            && self
                .links
                .values()
                .all(|link| link.state.get() == OwnedLinkState::Established)
        {
            self.retry_at.set(None);
            self.retry_delay.set(RESOURCE_RETRY_MIN);
        } else if self.retry_at.get().is_some_and(|deadline| now < deadline) {
            return;
        }
        self.retry_at.set(None);
        for spec in self.desired.iter().copied() {
            let Entry::Vacant(entry) = self.links.entry(spec) else {
                continue;
            };
            let props = properties! {
                *pw::keys::LINK_OUTPUT_NODE => spec.output_node.to_string(),
                *pw::keys::LINK_OUTPUT_PORT => spec.output_port.to_string(),
                *pw::keys::LINK_INPUT_NODE => spec.input_node.to_string(),
                *pw::keys::LINK_INPUT_PORT => spec.input_port.to_string(),
                *pw::keys::MEDIA_TYPE => "Audio",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_ROLE => "Production",
            };
            match self
                .core
                .create_object::<pw::link::Link>(LINK_FACTORY, &props)
            {
                Ok(link) => {
                    let state = Rc::new(Cell::new(OwnedLinkState::Pending));
                    let listener_state = Rc::clone(&state);
                    let dirty = Rc::clone(&self.dirty);
                    let retry_at = Rc::clone(&self.retry_at);
                    let retry_delay = Rc::clone(&self.retry_delay);
                    let listener = link
                        .add_listener_local()
                        .info(move |info| {
                            let message = match info.state() {
                                pw::link::LinkState::Paused | pw::link::LinkState::Active => {
                                    listener_state.set(OwnedLinkState::Established);
                                    dirty.set(true);
                                    return;
                                }
                                pw::link::LinkState::Error(message) => Some(message),
                                pw::link::LinkState::Unlinked => None,
                                _ => return,
                            };
                            listener_state.set(OwnedLinkState::Failed);
                            dirty.set(true);
                            let report = defer_retry(&retry_at, &retry_delay, Instant::now());
                            if let Some(message) = message
                                && report
                            {
                                error!("[pipewire] link failed {spec:?}: {message}");
                            }
                        })
                        .register();
                    entry.insert(OwnedLink {
                        state,
                        _listener: listener,
                        _proxy: link,
                    });
                }
                Err(err) => {
                    self.dirty.set(true);
                    if defer_retry(&self.retry_at, &self.retry_delay, now) {
                        error!("[pipewire] could not create link {spec:?}: {err}");
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
struct RegistryContext {
    registry: RegistryRc,
    graph: Rc<RefCell<Graph>>,
    dirty: Rc<Cell<bool>>,
    links: Rc<RefCell<HashMap<u32, (pw::link::Link, pw::link::LinkListener)>>>,
    metadata: Rc<RefCell<HashMap<u32, (Metadata, MetadataListener)>>>,
}

impl RegistryContext {
    fn changed(&self, update: impl FnOnce(&mut Graph)) {
        update(&mut self.graph.borrow_mut());
        self.dirty.set(true);
    }

    fn add(&self, global: &GlobalObject<&DictRef>) {
        match global.type_ {
            ObjectType::Node => self.changed(|graph| graph.upsert_node(Node::from_global(global))),
            ObjectType::Port => {
                if let Some(port) = Port::from_global(global) {
                    self.changed(|graph| graph.upsert_port(port));
                }
            }
            ObjectType::Client => self.changed(|graph| graph.add_client(global.id)),
            ObjectType::Link => self.add_link(global),
            ObjectType::Metadata => self.add_metadata(global),
            _ => {}
        }
    }

    fn remove(&self, id: u32) {
        self.changed(|graph| graph.remove_global(id));
        self.links.borrow_mut().remove(&id);
        self.metadata.borrow_mut().remove(&id);
    }

    fn add_link(&self, global: &GlobalObject<&DictRef>) {
        let id = global.id;
        if self.links.borrow().contains_key(&id) {
            return;
        }
        let Ok(proxy) = self.registry.bind::<pw::link::Link, _>(global) else {
            warn!("[pipewire] failed to bind link {id}");
            return;
        };
        let graph = Rc::clone(&self.graph);
        let dirty = Rc::clone(&self.dirty);
        let listener = proxy
            .add_listener_local()
            .info(move |info| {
                graph.borrow_mut().upsert_link(
                    id,
                    GraphLink {
                        output_node: info.output_node_id(),
                        input_node: info.input_node_id(),
                        active: matches!(info.state(), pw::link::LinkState::Active),
                    },
                );
                dirty.set(true);
            })
            .register();
        self.links.borrow_mut().insert(id, (proxy, listener));
    }

    fn add_metadata(&self, global: &GlobalObject<&DictRef>) {
        let id = global.id;
        let is_default = global
            .props
            .as_ref()
            .and_then(|props| props.get("metadata.name"))
            .is_some_and(|name| name.eq_ignore_ascii_case(DEFAULT_METADATA));
        if !is_default || self.metadata.borrow().contains_key(&id) {
            return;
        }
        let Ok(proxy) = self.registry.bind::<Metadata, _>(global) else {
            warn!("[pipewire] failed to bind default metadata {id}");
            return;
        };
        let graph = Rc::clone(&self.graph);
        let dirty = Rc::clone(&self.dirty);
        let listener = proxy
            .add_listener_local()
            .property(move |subject, key, type_hint, value| {
                graph
                    .borrow_mut()
                    .metadata(id, subject, key, type_hint, value);
                dirty.set(true);
                0
            })
            .register();
        self.metadata.borrow_mut().insert(id, (proxy, listener));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_reconciliation_preserves_retry_and_publication_boundaries() {
        pw::init();
        let mainloop = pw::main_loop::MainLoopRc::new(None).unwrap();
        let context = pw::context::ContextRc::new(&mainloop, None).unwrap();
        // A local socket keeps real PipeWire objects independent of the user's daemon.
        let (_peer, socket) = UnixStream::pair().unwrap();
        let core = context.connect_fd_rc(socket.into(), None).unwrap();
        let (writer, _audio) = super::super::transport::channel();
        let writer = Rc::new(RefCell::new(writer));
        let dirty = Rc::new(Cell::new(true));
        let mut capture = CaptureState::new(core, Rc::clone(&writer), dirty);
        let graph = RefCell::new(Graph::default());
        let mut config = CaptureConfig::default();
        let public = PublicState::default();
        let now = Instant::now();
        let deadline = now + RESOURCE_RETRY_MIN;
        capture.stream_retry_at = Some(deadline);
        let initial = public.view();
        capture.reconcile(
            &graph,
            &mut config,
            &public,
            deadline - Duration::from_nanos(1),
        );
        assert!(capture.dirty.get());
        assert!(capture.tap.config().is_none());
        assert!(Arc::ptr_eq(&initial, &public.view()));
        capture.reconcile(&graph, &mut config, &public, deadline);
        assert!(capture.tap.config().is_some());
        // Synchronous stream callbacks leave fresh work for the next iteration.
        assert!(capture.dirty.get());
        assert_eq!(capture.stream_retry_at, None);
        assert!(!Arc::ptr_eq(&initial, &public.view()));

        for status in [StreamStatus::Paused, StreamStatus::Streaming] {
            writer.borrow_mut().set_status(status);
            capture.dirty.set(true);
            capture.stream_retry_at = Some(deadline);
            capture.stream_retry_delay = Duration::from_secs(8);
            capture.reconcile(&graph, &mut config, &public, now);
            assert_eq!(capture.stream_retry_delay, RESOURCE_RETRY_MIN);
            assert_eq!(capture.stream_retry_at, Some(deadline));
            assert!(capture.dirty.get());
        }
        for status in [StreamStatus::Failed, StreamStatus::Stopped] {
            let published = public.view();
            capture.owned_links.retry_at.set(Some(now));
            capture.owned_links.retry_delay.set(Duration::from_secs(8));
            capture.stream_retry_delay = Duration::from_secs(8);
            capture.dirty.set(false);
            writer.borrow_mut().set_status(status);
            capture.reconcile(&graph, &mut config, &public, now);
            assert!(capture.tap.config().is_none());
            assert_eq!(capture.owned_links.retry_at.get(), None);
            assert_eq!(capture.owned_links.retry_delay.get(), RESOURCE_RETRY_MIN);
            assert_eq!(capture.stream_retry_at, Some(now + Duration::from_secs(8)));
            assert_eq!(capture.stream_retry_delay, Duration::from_secs(16));
            capture.reconcile(&graph, &mut config, &public, now + Duration::from_secs(1));
            assert_eq!(capture.stream_retry_at, Some(now + Duration::from_secs(8)));
            assert_eq!(capture.stream_retry_delay, Duration::from_secs(16));
            assert!(capture.dirty.get());
            assert!(Arc::ptr_eq(&published, &public.view()));
            capture.dirty.set(false);
            capture.configuration_changed();
            assert_eq!(capture.stream_retry_at, None);
            assert_eq!(capture.stream_retry_delay, RESOURCE_RETRY_MIN);
            assert!(capture.dirty.get());
            capture.reconcile(&graph, &mut config, &public, now);
            assert!(capture.tap.config().is_some());
        }

        capture.reconcile(&graph, &mut config, &public, now);
        assert!(!capture.dirty.get());
        public.publish(super::super::CaptureView {
            default_sink: "unpublished".into(),
            ..Default::default()
        });
        let published = public.view();
        capture.owned_links.retry_at.set(Some(deadline));
        capture.reconcile(&graph, &mut config, &public, now);
        assert!(Arc::ptr_eq(&published, &public.view()));
        capture.reconcile(&graph, &mut config, &public, deadline);
        assert!(!Arc::ptr_eq(&published, &public.view()));
        assert_eq!(capture.owned_links.retry_at.get(), None);
    }
}
