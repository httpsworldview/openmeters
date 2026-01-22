use super::state::RegistryState;
use super::types::{
    GraphPort, LinkSpec, NodeInfo, RegistryCommand, RegistrySnapshot, format_target_metadata,
};
use anyhow::{Context, Result};
use parking_lot::RwLock;
use pipewire as pw;
use pw::metadata::{Metadata, MetadataListener};
use pw::properties::properties;
use pw::registry::{GlobalObject, RegistryRc};
use pw::spa::utils::dict::DictRef;
use pw::types::ObjectType;
use rustc_hash::{FxHashMap, FxHashSet};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, OnceLock, mpsc};
use std::thread;
use std::time::Duration;
use tracing::{debug, error, info, warn};

const REGISTRY_THREAD_NAME: &str = "openmeters-pw-registry";
const TARGET_OBJECT_KEY: &str = "target.object";
const TARGET_NODE_KEY: &str = "target.node";
const LINK_FACTORY_NAME: &str = "link-factory";
const PREFERRED_METADATA_NAMES: &[&str] = &["settings", "default"];

static RUNTIME: OnceLock<RegistryRuntime> = OnceLock::new();

pub fn spawn_registry() -> Result<AudioRegistryHandle> {
    if let Some(runtime) = RUNTIME.get() {
        return Ok(AudioRegistryHandle {
            runtime: runtime.clone(),
        });
    }

    let runtime = RegistryRuntime::default();

    match RUNTIME.set(runtime.clone()) {
        Ok(()) => {
            let thread_runtime = runtime.clone();
            thread::Builder::new()
                .name(REGISTRY_THREAD_NAME.into())
                .spawn(move || {
                    if let Err(err) = registry_thread_main(thread_runtime) {
                        error!("[registry] thread terminated: {err:?}");
                    }
                })
                .context("failed to spawn PipeWire registry thread")?;

            Ok(AudioRegistryHandle { runtime })
        }
        Err(_) => {
            let runtime = RUNTIME.get().expect("registry runtime initialized");
            Ok(AudioRegistryHandle {
                runtime: runtime.clone(),
            })
        }
    }
}

#[derive(Clone)]
pub struct AudioRegistryHandle {
    runtime: RegistryRuntime,
}

impl AudioRegistryHandle {
    pub fn subscribe(&self) -> RegistryUpdates {
        self.runtime.subscribe()
    }

    pub fn send_command(&self, command: RegistryCommand) -> bool {
        self.runtime.send_command(command)
    }

    pub fn set_links(&self, links: Vec<LinkSpec>) -> bool {
        self.send_command(RegistryCommand::SetLinks(links))
    }

    pub fn route_node(&self, application: &NodeInfo, sink: &NodeInfo) -> bool {
        let (target_object, target_node) = format_target_metadata(sink.object_serial(), sink.id);
        self.send_command(RegistryCommand::RouteNode {
            subject: application.id,
            target_object,
            target_node,
        })
    }
}

pub struct RegistryUpdates {
    initial: Option<RegistrySnapshot>,
    receiver: mpsc::Receiver<RegistrySnapshot>,
}

impl RegistryUpdates {
    pub fn recv_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<RegistrySnapshot>, mpsc::RecvTimeoutError> {
        if let Some(snapshot) = self.initial.take() {
            return Ok(Some(snapshot));
        }

        match self.receiver.recv_timeout(timeout) {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

#[derive(Clone, Default)]
struct RegistryRuntime {
    state: Arc<RwLock<RegistryState>>,
    watchers: Arc<RwLock<Vec<mpsc::Sender<RegistrySnapshot>>>>,
    commands: Arc<RwLock<Option<mpsc::Sender<RegistryCommand>>>>,
}

impl RegistryRuntime {
    fn set_command_sender(&self, sender: mpsc::Sender<RegistryCommand>) {
        *self.commands.write() = Some(sender);
    }

    fn send_command(&self, command: RegistryCommand) -> bool {
        match self.commands.read().as_ref() {
            Some(sender) => sender
                .send(command)
                .inspect_err(|_| {
                    warn!("[registry] failed to send command; channel closed");
                })
                .is_ok(),
            None => {
                warn!("[registry] command channel not initialised");
                false
            }
        }
    }

    fn snapshot(&self) -> RegistrySnapshot {
        self.state.read().snapshot()
    }

    fn subscribe(&self) -> RegistryUpdates {
        let (tx, rx) = mpsc::channel();
        self.watchers.write().push(tx);
        RegistryUpdates {
            initial: Some(self.snapshot()),
            receiver: rx,
        }
    }

    fn mutate<F: FnOnce(&mut RegistryState) -> bool>(&self, f: F) -> bool {
        let changed = f(&mut self.state.write());
        if changed {
            self.notify_watchers();
        }
        changed
    }

    fn notify_watchers(&self) {
        let snapshot = self.state.read().snapshot();
        self.watchers
            .write()
            .retain(|tx| tx.send(snapshot.clone()).is_ok());
    }
}

fn registry_thread_main(runtime: RegistryRuntime) -> Result<()> {
    pw::init();

    let mainloop =
        pw::main_loop::MainLoopRc::new(None).context("failed to create PipeWire main loop")?;
    let context = pw::context::ContextRc::new(&mainloop, None)
        .context("failed to create PipeWire context")?;
    let core = context
        .connect_rc(None)
        .context("failed to connect to PipeWire core")?;
    let registry = core
        .get_registry_rc()
        .context("failed to obtain PipeWire registry")?;

    let (command_tx, command_rx) = mpsc::channel::<RegistryCommand>();
    runtime.set_command_sender(command_tx);

    let mut link_state = LinkState::new(core.clone());
    let routing_metadata: Rc<RefCell<Option<Metadata>>> = Rc::new(RefCell::new(None));

    let metadata_bindings: Rc<RefCell<HashMap<u32, MetadataBinding>>> = Default::default();

    let _registry_listener = {
        let registry_added = registry.clone();
        let metadata_added = Rc::clone(&metadata_bindings);
        let metadata_removed = Rc::clone(&metadata_bindings);
        let routing_metadata = Rc::clone(&routing_metadata);
        let runtime_added = runtime.clone();
        let runtime_removed = runtime.clone();

        registry
            .add_listener_local()
            .global(move |global| {
                handle_global_added(
                    &registry_added,
                    global,
                    &runtime_added,
                    &metadata_added,
                    &routing_metadata,
                );
            })
            .global_remove(move |id| {
                handle_global_removed(id, &runtime_removed, &metadata_removed);
            })
            .register()
    };

    if let Err(err) = core.sync(0) {
        error!("[registry] failed to sync core: {err}");
    }

    info!("[registry] PipeWire registry thread running");

    let loop_ref = mainloop.loop_();
    let mut commands_disconnected = false;
    let mut consecutive_errors = 0u32;
    const MAX_CONSECUTIVE_ERRORS: u32 = 10;

    loop {
        if !commands_disconnected {
            loop {
                match command_rx.try_recv() {
                    Ok(command) => {
                        handle_command(command, &mut link_state, &routing_metadata, &mainloop);
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        commands_disconnected = true;
                        break;
                    }
                }
            }
        }

        let result = loop_ref.iterate(Duration::from_millis(50));
        if result >= 0 {
            if consecutive_errors > 0 {
                info!(
                    "[registry] PipeWire loop recovered after {} error(s)",
                    consecutive_errors
                );
                consecutive_errors = 0;
            }
            continue;
        }

        consecutive_errors += 1;
        if consecutive_errors == 1 {
            warn!(
                "[registry] PipeWire loop iteration failed (errno={}); retrying",
                -result
            );
        }
        if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
            error!(
                "[registry] PipeWire loop failed {} consecutive times; exiting",
                consecutive_errors
            );
            break;
        }
        let backoff = Duration::from_millis(50 * (1 << consecutive_errors.min(4)));
        thread::sleep(backoff);
    }

    *runtime.commands.write() = None;
    info!("[registry] PipeWire registry loop exited");

    drop(registry);
    drop(context);

    Ok(())
}

struct LinkState {
    core: pw::core::CoreRc,
    active_links: FxHashMap<LinkSpec, pw::link::Link>,
}

impl LinkState {
    fn new(core: pw::core::CoreRc) -> Self {
        Self {
            core,
            active_links: FxHashMap::default(),
        }
    }

    fn apply_links(&mut self, desired: Vec<LinkSpec>) {
        let desired_set: FxHashSet<_> = desired.iter().copied().collect();

        self.active_links.retain(|spec, _| {
            let keep = desired_set.contains(spec);
            if !keep {
                debug!("[registry] removed link {:?}", spec);
            }
            keep
        });

        for spec in desired {
            if self.active_links.contains_key(&spec) {
                continue;
            }
            match create_passive_audio_link(
                &self.core,
                spec.output_node,
                spec.output_port,
                spec.input_node,
                spec.input_port,
            ) {
                Ok(link) => {
                    debug!("[registry] linked {:?}", spec);
                    self.active_links.insert(spec, link);
                }
                Err(err) => error!("[registry] link failed {:?}: {err}", spec),
            }
        }
    }
}

fn create_passive_audio_link(
    core: &pw::core::CoreRc,
    output_node: u32,
    output_port: u32,
    input_node: u32,
    input_port: u32,
) -> std::result::Result<pw::link::Link, pw::Error> {
    let props = properties! {
        *pw::keys::LINK_OUTPUT_NODE => output_node.to_string(),
        *pw::keys::LINK_OUTPUT_PORT => output_port.to_string(),
        *pw::keys::LINK_INPUT_NODE => input_node.to_string(),
        *pw::keys::LINK_INPUT_PORT => input_port.to_string(),
        *pw::keys::LINK_PASSIVE => "true",
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Playback",
        *pw::keys::MEDIA_ROLE => "Playback",
    };
    core.create_object::<pw::link::Link>(LINK_FACTORY_NAME, &props)
}

fn handle_command(
    command: RegistryCommand,
    link_state: &mut LinkState,
    routing_metadata: &Rc<RefCell<Option<Metadata>>>,
    mainloop: &pw::main_loop::MainLoopRc,
) {
    match command {
        RegistryCommand::SetLinks(desired) => link_state.apply_links(desired),
        RegistryCommand::RouteNode {
            subject,
            target_object,
            target_node,
        } => {
            let borrowed = routing_metadata.borrow();
            let Some(metadata) = borrowed.as_ref() else {
                warn!(
                    "[registry] cannot route node {}; no metadata bound",
                    subject
                );
                return;
            };
            metadata.set_property(
                subject,
                TARGET_OBJECT_KEY,
                Some("Spa:Id"),
                Some(&target_object),
            );
            metadata.set_property(subject, TARGET_NODE_KEY, Some("Spa:Id"), Some(&target_node));
            mainloop.loop_().iterate(Duration::from_millis(10));
            debug!(
                "[registry] routed node {} -> object={}, node={}",
                subject, target_object, target_node
            );
        }
    }
}

fn handle_global_added(
    registry: &RegistryRc,
    global: &GlobalObject<&DictRef>,
    runtime: &RegistryRuntime,
    metadata_bindings: &Rc<RefCell<HashMap<u32, MetadataBinding>>>,
    routing_metadata: &Rc<RefCell<Option<Metadata>>>,
) {
    match global.type_ {
        ObjectType::Node => {
            runtime.mutate(|s| s.upsert_node(NodeInfo::from_global(global)));
        }
        ObjectType::Device => {
            runtime.mutate(|s| {
                s.add_device();
                true
            });
        }
        ObjectType::Port => {
            if let Some(p) = GraphPort::from_global(global) {
                runtime.mutate(|s| s.upsert_port(p));
            }
        }
        ObjectType::Metadata => process_metadata_added(
            registry,
            global,
            runtime,
            metadata_bindings,
            routing_metadata,
        ),
        _ => {}
    }
}

fn handle_global_removed(
    id: u32,
    runtime: &RegistryRuntime,
    metadata_bindings: &Rc<RefCell<HashMap<u32, MetadataBinding>>>,
) {
    if runtime.mutate(|s| s.remove_port(id) || s.remove_node(id)) {
        return;
    }
    if metadata_bindings.borrow_mut().remove(&id).is_some() {
        runtime.mutate(|s| s.apply_metadata_property(id, 0, None, None, None));
    }
}

fn process_metadata_added(
    registry: &RegistryRc,
    global: &GlobalObject<&DictRef>,
    runtime: &RegistryRuntime,
    metadata_bindings: &Rc<RefCell<HashMap<u32, MetadataBinding>>>,
    routing_metadata: &Rc<RefCell<Option<Metadata>>>,
) {
    let metadata_id = global.id;
    if metadata_bindings.borrow().contains_key(&metadata_id) {
        return;
    }

    let props = super::types::dict_to_map(global.props.as_ref().copied());
    let metadata_name = props.get("metadata.name").cloned();

    let Ok(metadata) = registry.bind::<Metadata, _>(global) else {
        warn!("[registry] failed to bind metadata {metadata_id}");
        return;
    };

    let is_preferred = metadata_name.as_deref().is_some_and(|n| {
        PREFERRED_METADATA_NAMES
            .iter()
            .any(|p| p.eq_ignore_ascii_case(n))
    });

    {
        let mut routing_ref = routing_metadata.borrow_mut();
        if (is_preferred || routing_ref.is_none())
            && let Ok(copy) = registry.bind::<Metadata, _>(global)
        {
            *routing_ref = Some(copy);
            info!(
                "[registry] using metadata '{}' for routing",
                metadata_name.as_deref().unwrap_or("unnamed")
            );
        }
    }

    let runtime_for_listener = runtime.clone();
    let listener = metadata
        .add_listener_local()
        .property(move |subject, key, type_, value| {
            runtime_for_listener
                .mutate(|s| s.apply_metadata_property(metadata_id, subject, key, type_, value));
            0
        })
        .register();

    metadata_bindings.borrow_mut().insert(
        metadata_id,
        MetadataBinding {
            _proxy: metadata,
            _listener: listener,
        },
    );
}

struct MetadataBinding {
    _proxy: Metadata,
    _listener: MetadataListener,
}
