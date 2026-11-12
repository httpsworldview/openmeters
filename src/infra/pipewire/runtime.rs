// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::graph::{Graph, GraphLink, Node, Port, property};
use super::policy::{self, LinkSpec};
use super::stream::TapStream;
use super::transport::{CaptureWriter, StreamStatus};
use super::{Command, DynError, PublicState};
use crate::domain::routing::CaptureConfig;
use pipewire as pw;
use pw::metadata::Metadata;
use pw::properties::properties;
use pw::proxy::ProxyT;
use pw::registry::{GlobalObject, RegistryRc};
use pw::spa::utils::dict::DictRef;
use pw::types::ObjectType;
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
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

// A sender that is gone reads as a shutdown.
fn queued(commands: &mpsc::Receiver<Command>) -> impl Iterator<Item = Command> {
    std::iter::from_fn(|| match commands.try_recv() {
        Ok(command) => Some(command),
        Err(mpsc::TryRecvError::Disconnected) => Some(Command::Shutdown),
        Err(mpsc::TryRecvError::Empty) => None,
    })
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
    for command in queued(commands) {
        match command {
            Command::Configure(newer) => next = newer,
            Command::Shutdown => return Retry::Stop,
        }
    }
    *config = next;
    Retry::Configured
}

fn take_retry_delay(delay: &mut Duration, maximum: Duration) -> Duration {
    let current = *delay;
    *delay = delay.saturating_mul(2).min(maximum);
    current
}

#[derive(Clone, Copy)]
struct Backoff {
    retry_at: Option<Instant>,
    delay: Duration,
    up_since: Option<Instant>,
}

impl Backoff {
    const IDLE: Self = Self {
        retry_at: None,
        delay: RESOURCE_RETRY_MIN,
        up_since: None,
    };

    fn pending(&self, now: Instant) -> bool {
        self.retry_at.is_some_and(|deadline| now < deadline)
    }

    fn due(&self, now: Instant) -> bool {
        self.retry_at.is_some_and(|deadline| now >= deadline)
    }

    fn defer(&mut self, now: Instant) {
        self.retry_at = Some(now + take_retry_delay(&mut self.delay, RESOURCE_RETRY_MAX));
    }

    fn defer_if_idle(&mut self, now: Instant) -> bool {
        !self.pending(now) && {
            self.defer(now);
            true
        }
    }

    fn restart(&mut self) {
        self.retry_at = None;
        self.delay = RESOURCE_RETRY_MIN;
    }

    // Staying up at least as long as the current delay proves the retry.
    fn settle(&mut self, now: Instant) {
        if self
            .up_since
            .is_some_and(|since| now.duration_since(since) >= self.delay)
        {
            self.delay = RESOURCE_RETRY_MIN;
        }
    }
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
        let Err(err) = run_session(&commands, &mut config, &writer, &public, socket.as_deref())
        else {
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
    writer: &Rc<RefCell<CaptureWriter>>,
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
        updates: GraphUpdates {
            graph: Rc::clone(&graph),
            dirty: Rc::clone(&dirty),
        },
        bound: Rc::default(),
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
                // Requests that race a destroyed object report "unknown resource" on the core,
                // harmlessly; any other core error, such as a lost connection, ends the session.
                let race = std::io::Error::from_raw_os_error(result.wrapping_neg()).kind()
                    == std::io::ErrorKind::NotFound;
                if id == pw::core::PW_ID_CORE && !race && fatal.borrow().is_none() {
                    *fatal.borrow_mut() = Some(detail);
                }
            })
            .register()
    };

    let mut capture = CaptureState::new(core, Rc::clone(writer), dirty);
    let mut errors = 0u32;

    info!("[pipewire] backend session starting");
    loop {
        let result = mainloop
            .loop_()
            .iterate(pw::loop_::Timeout::Finite(ITERATION_TIMEOUT));
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

        for command in queued(commands) {
            match command {
                Command::Configure(next) if *config != next => {
                    *config = next;
                    capture.configuration_changed();
                }
                Command::Configure(_) => {}
                Command::Shutdown => return Ok(()),
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
    stream_backoff: Backoff,
    reported_truncation: usize,
}

impl CaptureState {
    fn new(
        core: pw::core::CoreRc,
        writer: Rc<RefCell<CaptureWriter>>,
        dirty: Rc<Cell<bool>>,
    ) -> Self {
        Self {
            tap: TapStream::new(core.clone(), writer, Rc::clone(&dirty)),
            owned_links: OwnedLinks::new(core, Rc::clone(&dirty)),
            dirty,
            stream_backoff: Backoff::IDLE,
            reported_truncation: 0,
        }
    }

    fn configuration_changed(&mut self) {
        self.dirty.set(true);
        self.stream_backoff.restart();
    }

    fn defer_stream_retry(&mut self, now: Instant) {
        self.tap.clear_failed();
        self.dirty.set(true);
        self.stream_backoff.defer(now);
    }

    fn reconcile(
        &mut self,
        graph: &RefCell<Graph>,
        config: &mut CaptureConfig,
        public: &PublicState,
        now: Instant,
    ) {
        let status = self.tap.status();
        if status != Some(StreamStatus::Paused) {
            self.stream_backoff.up_since = None;
        }
        match status {
            // Paused only means the server accepted the node; the session manager may still
            // reject its target, so it proves the retry only once it outlasts the backoff.
            Some(StreamStatus::Streaming) => self.stream_backoff.delay = RESOURCE_RETRY_MIN,
            Some(StreamStatus::Paused) => {
                self.stream_backoff.up_since.get_or_insert(now);
                self.stream_backoff.settle(now);
            }
            Some(StreamStatus::Failed | StreamStatus::Stopped) => {
                self.owned_links.clear();
                self.defer_stream_retry(now);
            }
            _ => {}
        }

        if !self.dirty.get() && !self.owned_links.backoff.get().due(now) {
            return;
        }
        if self.stream_backoff.pending(now) {
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
            self.stream_backoff.up_since = None;
            if let Err(err) = self.tap.configure(plan.stream) {
                error!("[capture] stream reconfiguration failed: {err}");
                self.defer_stream_retry(now);
                return;
            }
            self.stream_backoff.retry_at = None;
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
    backoff: Rc<Cell<Backoff>>,
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
    _removed: pw::proxy::ProxyListener,
    _proxy: pw::link::Link,
}

impl OwnedLinks {
    fn new(core: pw::core::CoreRc, dirty: Rc<Cell<bool>>) -> Self {
        Self {
            core,
            links: HashMap::new(),
            desired: Vec::new(),
            dirty,
            backoff: Rc::new(Cell::new(Backoff::IDLE)),
        }
    }

    fn clear(&mut self) {
        self.links.clear();
        self.desired.clear();
        self.backoff.set(Backoff::IDLE);
    }

    fn apply(&mut self, desired: Vec<LinkSpec>, now: Instant) {
        if self.desired != desired {
            self.desired = desired;
            let mut retry = self.backoff.get();
            retry.restart();
            self.backoff.set(retry);
        }
        self.links.retain(|spec, link| {
            self.desired.binary_search(spec).is_ok() && link.state.get() != OwnedLinkState::Failed
        });
        let mut retry = self.backoff.get();
        if self.links.len() == self.desired.len()
            && self
                .links
                .values()
                .all(|link| link.state.get() == OwnedLinkState::Established)
        {
            retry.up_since.get_or_insert(now);
        } else {
            retry.up_since = None;
            if retry.pending(now) {
                self.backoff.set(retry);
                return;
            }
        }
        retry.retry_at = None;
        self.backoff.set(retry);
        for spec in self.desired.iter().copied() {
            if self.links.contains_key(&spec) {
                continue;
            }
            match self.create(spec) {
                Ok(link) => {
                    self.links.insert(spec, link);
                }
                Err(err) => {
                    self.dirty.set(true);
                    let mut retry = self.backoff.get();
                    if retry.defer_if_idle(now) {
                        self.backoff.set(retry);
                        error!("[pipewire] could not create link {spec:?}: {err}");
                    }
                }
            }
        }
    }

    fn create(&self, spec: LinkSpec) -> Result<OwnedLink, pw::Error> {
        let props = properties! {
            *pw::keys::LINK_OUTPUT_NODE => spec.output_node.to_string(),
            *pw::keys::LINK_OUTPUT_PORT => spec.output_port.to_string(),
            *pw::keys::LINK_INPUT_NODE => spec.input_node.to_string(),
            *pw::keys::LINK_INPUT_PORT => spec.input_port.to_string(),
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Production",
        };
        let link = (self.core).create_object::<pw::link::Link>(LINK_FACTORY, &props)?;
        let state = Rc::new(Cell::new(OwnedLinkState::Pending));
        let fail = {
            let (state, dirty) = (Rc::clone(&state), Rc::clone(&self.dirty));
            let backoff = Rc::clone(&self.backoff);
            Rc::new(move |reason: Option<&str>| {
                state.set(OwnedLinkState::Failed);
                dirty.set(true);
                let now = Instant::now();
                let mut retry = backoff.get();
                // Links someone keeps removing come back between removals; they
                // prove the retry only once they outlast the backoff.
                retry.settle(now);
                retry.up_since = None;
                let deferred = retry.defer_if_idle(now);
                backoff.set(retry);
                match reason {
                    Some(reason) if deferred => {
                        error!("[pipewire] link failed {spec:?}: {reason}");
                    }
                    // Also what a captured stream's end does to its links.
                    None => debug!("[pipewire] link removed {spec:?}"),
                    Some(_) => {}
                }
            })
        };
        let (listener_state, dirty) = (Rc::clone(&state), Rc::clone(&self.dirty));
        let on_error = Rc::clone(&fail);
        let listener = link
            .add_listener_local()
            .info(move |info| match info.state() {
                pw::link::LinkState::Paused | pw::link::LinkState::Active => {
                    listener_state.set(OwnedLinkState::Established);
                    dirty.set(true);
                }
                pw::link::LinkState::Error(message) => on_error(Some(message)),
                _ => {}
            })
            .register();
        // A link removed by someone else sends no info event, only this.
        let removed = link
            .upcast_ref()
            .add_listener_local()
            .removed(move || fail(None))
            .register();
        Ok(OwnedLink {
            state,
            _listener: listener,
            _removed: removed,
            _proxy: link,
        })
    }
}

#[derive(Clone)]
struct GraphUpdates {
    graph: Rc<RefCell<Graph>>,
    dirty: Rc<Cell<bool>>,
}

impl GraphUpdates {
    fn changed(&self, update: impl FnOnce(&mut Graph)) {
        update(&mut self.graph.borrow_mut());
        self.dirty.set(true);
    }
}

#[derive(Clone)]
struct RegistryContext {
    registry: RegistryRc,
    updates: GraphUpdates,
    // Each bound global's (listener, proxy). Dropping a removed global's proxy frees it, so
    // the listener goes first to prevent UAF.
    bound: Rc<RefCell<HashMap<u32, Box<dyn Any>>>>,
}

impl RegistryContext {
    fn add(&self, global: &GlobalObject<&DictRef>) {
        let updates = &self.updates;
        match global.type_ {
            ObjectType::Node => {
                updates.changed(|graph| graph.upsert_node(Node::from_global(global)));
            }
            ObjectType::Port => {
                if let Some(port) = Port::from_global(global) {
                    updates.changed(|graph| graph.upsert_port(port));
                }
            }
            ObjectType::Client => updates.changed(|graph| graph.add_client(global.id)),
            ObjectType::Link => self.add_link(global),
            ObjectType::Metadata => self.add_metadata(global),
            _ => {}
        }
    }

    fn remove(&self, id: u32) {
        self.updates.changed(|graph| graph.remove_global(id));
        self.bound.borrow_mut().remove(&id);
    }

    fn bind<P: ProxyT + 'static, L: 'static>(
        &self,
        global: &GlobalObject<&DictRef>,
        listen: impl FnOnce(&P, GraphUpdates) -> L,
    ) {
        if self.bound.borrow().contains_key(&global.id) {
            return;
        }
        match self.registry.bind::<P, _>(global) {
            Ok(proxy) => {
                let listener = listen(&proxy, self.updates.clone());
                let bound: Box<dyn Any> = Box::new((listener, proxy));
                self.bound.borrow_mut().insert(global.id, bound);
            }
            Err(err) => warn!(
                "[pipewire] failed to bind {} {}: {err}",
                global.type_, global.id
            ),
        }
    }

    fn add_link(&self, global: &GlobalObject<&DictRef>) {
        let id = global.id;
        self.bind(global, |link: &pw::link::Link, updates| {
            link.add_listener_local()
                .info(move |info| {
                    let link = GraphLink {
                        output_node: info.output_node_id(),
                        input_node: info.input_node_id(),
                        active: matches!(info.state(), pw::link::LinkState::Active),
                    };
                    updates.changed(|graph| graph.upsert_link(id, link));
                })
                .register()
        });
    }

    fn add_metadata(&self, global: &GlobalObject<&DictRef>) {
        let id = global.id;
        let is_default = property(global, "metadata.name")
            .is_some_and(|name| name.eq_ignore_ascii_case(DEFAULT_METADATA));
        if !is_default {
            return;
        }
        self.bind(global, |metadata: &Metadata, updates| {
            metadata
                .add_listener_local()
                .property(move |subject, key, type_hint, value| {
                    updates.changed(|graph| graph.metadata(id, subject, key, type_hint, value));
                    0
                })
                .register()
        });
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
        // An empty graph's view equals the default one, so a marker shows what reconcile publishes.
        let unpublished = || super::super::CaptureView {
            default_sink: Some("unpublished".into()),
            ..Default::default()
        };
        public.publish(unpublished());
        let now = Instant::now();
        let deadline = now + RESOURCE_RETRY_MIN;
        capture.stream_backoff.retry_at = Some(deadline);
        let initial = public.view();
        let early = deadline - Duration::from_nanos(1);
        capture.reconcile(&graph, &mut config, &public, early);
        assert!(capture.dirty.get());
        assert!(capture.tap.config().is_none());
        assert!(Arc::ptr_eq(&initial, &public.view()));
        capture.reconcile(&graph, &mut config, &public, deadline);
        assert!(capture.tap.config().is_some());
        // Synchronous stream callbacks leave fresh work for the next iteration.
        assert!(capture.dirty.get());
        assert_eq!(capture.stream_backoff.retry_at, None);
        assert!(!Arc::ptr_eq(&initial, &public.view()));

        for (status, delay) in [
            (StreamStatus::Paused, Duration::from_secs(8)),
            (StreamStatus::Streaming, RESOURCE_RETRY_MIN),
        ] {
            writer.borrow_mut().set_status(status);
            capture.dirty.set(true);
            capture.stream_backoff.retry_at = Some(deadline);
            capture.stream_backoff.delay = Duration::from_secs(8);
            capture.reconcile(&graph, &mut config, &public, now);
            assert_eq!(capture.stream_backoff.delay, delay);
            assert_eq!(capture.stream_backoff.retry_at, Some(deadline));
            assert!(capture.dirty.get());
        }
        // Staying accepted for longer than the backoff proves the retry.
        writer.borrow_mut().set_status(StreamStatus::Paused);
        capture.stream_backoff.delay = Duration::from_secs(8);
        for (elapsed, delay) in [(1, 8), (8, 8), (9, 1)] {
            let at = now + Duration::from_secs(elapsed);
            capture.reconcile(&graph, &mut config, &public, at);
            assert_eq!(capture.stream_backoff.delay, Duration::from_secs(delay));
        }
        // A replacement stream does not inherit the accepted time of the one it replaces, even
        // when it is accepted before reconcile sees it start.
        let mut stale = capture.tap.config().copied().expect("configured tap");
        stale.passive = !stale.passive;
        capture.tap.configure(stale).expect("stale tap");
        writer.borrow_mut().set_status(StreamStatus::Paused);
        capture.dirty.set(true);
        capture.reconcile(&graph, &mut config, &public, now + Duration::from_secs(10));
        writer.borrow_mut().set_status(StreamStatus::Paused);
        capture.stream_backoff.delay = Duration::from_secs(8);
        capture.reconcile(&graph, &mut config, &public, now + Duration::from_secs(11));
        assert_eq!(capture.stream_backoff.delay, Duration::from_secs(8));
        for status in [StreamStatus::Failed, StreamStatus::Stopped] {
            let published = public.view();
            capture.owned_links.backoff.update(|backoff| Backoff {
                retry_at: Some(now),
                delay: Duration::from_secs(8),
                ..backoff
            });
            capture.stream_backoff.delay = Duration::from_secs(8);
            capture.dirty.set(false);
            writer.borrow_mut().set_status(status);
            capture.reconcile(&graph, &mut config, &public, now);
            assert!(capture.tap.config().is_none());
            assert_eq!(capture.owned_links.backoff.get().retry_at, None);
            assert_eq!(capture.owned_links.backoff.get().delay, RESOURCE_RETRY_MIN);
            assert_eq!(
                capture.stream_backoff.retry_at,
                Some(now + Duration::from_secs(8))
            );
            assert_eq!(capture.stream_backoff.delay, Duration::from_secs(16));
            capture.reconcile(&graph, &mut config, &public, now + Duration::from_secs(1));
            assert_eq!(
                capture.stream_backoff.retry_at,
                Some(now + Duration::from_secs(8))
            );
            assert_eq!(capture.stream_backoff.delay, Duration::from_secs(16));
            assert!(capture.dirty.get());
            assert!(Arc::ptr_eq(&published, &public.view()));
            capture.dirty.set(false);
            capture.configuration_changed();
            assert_eq!(capture.stream_backoff.retry_at, None);
            assert_eq!(capture.stream_backoff.delay, RESOURCE_RETRY_MIN);
            assert!(capture.dirty.get());
            capture.reconcile(&graph, &mut config, &public, now);
            assert!(capture.tap.config().is_some());
        }

        capture.reconcile(&graph, &mut config, &public, now);
        assert!(!capture.dirty.get());
        public.publish(unpublished());
        let published = public.view();
        capture.owned_links.backoff.update(|backoff| Backoff {
            retry_at: Some(deadline),
            ..backoff
        });
        capture.reconcile(&graph, &mut config, &public, now);
        assert!(Arc::ptr_eq(&published, &public.view()));
        capture.reconcile(&graph, &mut config, &public, deadline);
        assert!(!Arc::ptr_eq(&published, &public.view()));
        assert_eq!(capture.owned_links.backoff.get().retry_at, None);
    }
}
