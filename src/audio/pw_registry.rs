//! PipeWire registry observer service for OpenMeters.

use crate::util;
pub use crate::util::pipewire::DefaultTarget;
use crate::util::pipewire::{
    DEFAULT_AUDIO_SINK_KEY, DEFAULT_AUDIO_SOURCE_KEY, GraphNode, parse_metadata_name,
};
use anyhow::{Context, Result};
use parking_lot::RwLock;
use pipewire as pw;
use pw::metadata::{Metadata, MetadataListener};
use pw::registry::{GlobalObject, RegistryRc};
use pw::spa::utils::dict::DictRef;
use pw::types::ObjectType;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, OnceLock, mpsc};
use std::thread;

const REGISTRY_THREAD_NAME: &str = "openmeters-pw-registry";

static RUNTIME: OnceLock<RegistryRuntime> = OnceLock::new();
/// Ensure the PipeWire registry observer is running and return a handle to it.
pub fn spawn_registry() -> Result<AudioRegistryHandle> {
    if let Some(runtime) = RUNTIME.get() {
        return Ok(AudioRegistryHandle {
            state: runtime.state.clone(),
            watchers: runtime.watchers.clone(),
        });
    }

    let state = Arc::new(RwLock::new(RegistryState::default()));
    let watchers = Arc::new(RwLock::new(Vec::new()));

    match RUNTIME.set(RegistryRuntime {
        state: state.clone(),
        watchers: watchers.clone(),
    }) {
        Ok(()) => {
            let thread_state = Arc::clone(&state);
            let thread_watchers = Arc::clone(&watchers);

            thread::Builder::new()
                .name(REGISTRY_THREAD_NAME.into())
                .spawn(move || {
                    if let Err(err) = registry_thread_main(thread_state, thread_watchers) {
                        eprintln!("[registry] thread terminated: {err:?}");
                    }
                })
                .context("failed to spawn PipeWire registry thread")?;

            Ok(AudioRegistryHandle { state, watchers })
        }
        Err(_) => {
            let runtime = RUNTIME.get().expect("registry runtime initialized");
            Ok(AudioRegistryHandle {
                state: runtime.state.clone(),
                watchers: runtime.watchers.clone(),
            })
        }
    }
}

/// Shared handle that exposes snapshots and subscriptions to the PipeWire registry.
#[derive(Clone)]
pub struct AudioRegistryHandle {
    state: Arc<RwLock<RegistryState>>,
    watchers: Arc<RwLock<Vec<mpsc::Sender<RegistrySnapshot>>>>,
}

impl AudioRegistryHandle {
    /// Clone a point-in-time view of all known nodes, devices, and defaults.
    pub fn snapshot(&self) -> RegistrySnapshot {
        self.state.read().snapshot()
    }

    /// Subscribe to ongoing registry snapshots; the iterator yields the initial state first.
    pub fn subscribe(&self) -> RegistryUpdates {
        let (tx, rx) = mpsc::channel();
        {
            let mut watchers = self.watchers.write();
            watchers.push(tx);
        }

        RegistryUpdates {
            initial: Some(self.snapshot()),
            receiver: rx,
        }
    }
}

/// Iterator that produces live snapshots of the PipeWire registry.
pub struct RegistryUpdates {
    initial: Option<RegistrySnapshot>,
    receiver: mpsc::Receiver<RegistrySnapshot>,
}

impl RegistryUpdates {
    /// Block until the next snapshot is available; the first call returns the initial snapshot.
    pub fn recv(&mut self) -> Option<RegistrySnapshot> {
        if let Some(snapshot) = self.initial.take() {
            return Some(snapshot);
        }
        self.receiver.recv().ok()
    }
}

impl Iterator for RegistryUpdates {
    type Item = RegistrySnapshot;

    fn next(&mut self) -> Option<Self::Item> {
        self.recv()
    }
}

/// Collection of registry state cloned for thread-safe consumption.
#[derive(Clone, Debug, Default)]
pub struct RegistrySnapshot {
    pub serial: u64,
    pub nodes: Vec<NodeInfo>,
    pub devices: Vec<DeviceInfo>,
    pub defaults: MetadataDefaults,
}

/// PipeWire node information extracted from registry announcements.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeInfo {
    pub id: u32,
    pub name: Option<String>,
    pub description: Option<String>,
    pub media_class: Option<String>,
    pub media_role: Option<String>,
    pub direction: NodeDirection,
    pub is_virtual: bool,
    pub parent_device: Option<u32>,
    pub properties: HashMap<String, String>,
}

/// PipeWire device information extracted from registry announcements.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceInfo {
    pub id: u32,
    pub name: Option<String>,
    pub description: Option<String>,
    pub properties: HashMap<String, String>,
}

/// General direction of a node (input/output/unknown) inferred from metadata.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NodeDirection {
    Input,
    Output,
    #[default]
    Unknown,
}

/// Default targets as reported by PipeWire metadata.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetadataDefaults {
    pub audio_sink: Option<DefaultTarget>,
    pub audio_source: Option<DefaultTarget>,
}

#[derive(Debug, Default)]
struct RegistryState {
    serial: u64,
    nodes: HashMap<u32, NodeInfo>,
    devices: HashMap<u32, DeviceInfo>,
    metadata_defaults: MetadataDefaults,
}

impl RegistryState {
    fn snapshot(&self) -> RegistrySnapshot {
        RegistrySnapshot {
            serial: self.serial,
            nodes: self.nodes.values().cloned().collect(),
            devices: self.devices.values().cloned().collect(),
            defaults: self.metadata_defaults.clone(),
        }
    }

    fn upsert_node(&mut self, info: NodeInfo) -> bool {
        let needs_update = match self.nodes.get(&info.id) {
            Some(existing) if existing == &info => false,
            _ => true,
        };

        if needs_update {
            self.nodes.insert(info.id, info);
            self.reconcile_defaults();
            self.bump_serial();
        }

        needs_update
    }

    fn remove_node(&mut self, id: u32) -> bool {
        if let Some(info) = self.nodes.remove(&id) {
            let fallback = info.name.or(info.description);
            let defaults_changed = self.metadata_defaults.clear_node(id, fallback);
            if defaults_changed {
                self.reconcile_defaults();
            }
            self.bump_serial();
            true
        } else {
            false
        }
    }

    fn upsert_device(&mut self, info: DeviceInfo) -> bool {
        let needs_update = match self.devices.get(&info.id) {
            Some(existing) if existing == &info => false,
            _ => true,
        };

        if needs_update {
            self.devices.insert(info.id, info);
            self.bump_serial();
        }

        needs_update
    }

    fn remove_device(&mut self, id: u32) -> bool {
        if self.devices.remove(&id).is_some() {
            self.bump_serial();
            true
        } else {
            false
        }
    }

    fn apply_metadata_property(
        &mut self,
        metadata_id: u32,
        subject: u32,
        key: Option<&str>,
        type_hint: Option<&str>,
        value: Option<&str>,
    ) -> bool {
        let changed = match key {
            Some(key) => {
                self.metadata_defaults
                    .apply_update(metadata_id, subject, key, type_hint, value)
            }
            None => self.metadata_defaults.clear_metadata(metadata_id),
        };

        if changed {
            self.reconcile_defaults();
            self.bump_serial();
        }

        changed
    }

    fn clear_metadata_defaults(&mut self, metadata_id: u32) -> bool {
        if self.metadata_defaults.clear_metadata(metadata_id) {
            self.reconcile_defaults();
            self.bump_serial();
            true
        } else {
            false
        }
    }

    fn bump_serial(&mut self) {
        self.serial = self.serial.wrapping_add(1);
    }

    fn reconcile_defaults(&mut self) {
        self.metadata_defaults.reconcile_with_nodes(&self.nodes);
    }
}

impl MetadataDefaults {
    fn apply_update(
        &mut self,
        metadata_id: u32,
        subject: u32,
        key: &str,
        type_hint: Option<&str>,
        value: Option<&str>,
    ) -> bool {
        let slot = match key {
            DEFAULT_AUDIO_SINK_KEY => &mut self.audio_sink,
            DEFAULT_AUDIO_SOURCE_KEY => &mut self.audio_source,
            _ => return false,
        };

        match value {
            Some(val) => {
                let inserted = slot.is_none();
                let parsed_name = parse_metadata_name(type_hint, Some(val));
                let name_ref = parsed_name.as_deref().or(Some(val));

                let target = slot.get_or_insert_with(DefaultTarget::default);
                let updated = target.update(metadata_id, subject, type_hint, name_ref);
                inserted || updated
            }
            None => {
                if slot
                    .as_ref()
                    .is_some_and(|target| target.metadata_id == Some(metadata_id))
                {
                    *slot = None;
                    true
                } else {
                    false
                }
            }
        }
    }

    fn reconcile_with_nodes(&mut self, nodes: &HashMap<u32, NodeInfo>) {
        for target in [&mut self.audio_sink, &mut self.audio_source] {
            if let Some(target) = target {
                if let Some(node_id) = target.node_id {
                    if !nodes.contains_key(&node_id) {
                        target.node_id = None;
                    }
                }
                if target.node_id.is_none() {
                    if let Some(name) = &target.name {
                        if let Some((id, _)) = nodes
                            .iter()
                            .find(|(_, node)| node.name.as_deref() == Some(name))
                        {
                            target.node_id = Some(*id);
                        }
                    }
                }
            }
        }
    }

    fn clear_metadata(&mut self, metadata_id: u32) -> bool {
        let mut changed = false;
        for slot in [&mut self.audio_sink, &mut self.audio_source] {
            if slot
                .as_ref()
                .is_some_and(|target| target.metadata_id == Some(metadata_id))
            {
                *slot = None;
                changed = true;
            }
        }
        changed
    }

    fn clear_node(&mut self, node_id: u32, fallback_name: Option<String>) -> bool {
        let mut changed = false;
        for slot in [&mut self.audio_sink, &mut self.audio_source] {
            if let Some(target) = slot {
                if target.node_id == Some(node_id) {
                    target.node_id = None;
                    if target.name.is_none() {
                        target.name = fallback_name.clone();
                    }
                    changed = true;
                }
            }
        }
        changed
    }
}
#[derive(Clone)]
struct RegistryRuntime {
    state: Arc<RwLock<RegistryState>>,
    watchers: Arc<RwLock<Vec<mpsc::Sender<RegistrySnapshot>>>>,
}

fn registry_thread_main(
    shared_state: Arc<RwLock<RegistryState>>,
    watchers: Arc<RwLock<Vec<mpsc::Sender<RegistrySnapshot>>>>,
) -> Result<()> {
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

    let metadata_bindings: Rc<RefCell<HashMap<u32, MetadataBinding>>> =
        Rc::new(RefCell::new(HashMap::new()));

    let registry_for_added = registry.clone();
    let state_for_added = Arc::clone(&shared_state);
    let metadata_for_added = Rc::clone(&metadata_bindings);
    let state_for_removed = Arc::clone(&shared_state);
    let metadata_for_removed = Rc::clone(&metadata_bindings);
    let watchers_for_added = Arc::clone(&watchers);
    let watchers_for_removed = Arc::clone(&watchers);

    let _registry_listener = registry
        .add_listener_local()
        .global(move |global| {
            handle_global_added(
                &registry_for_added,
                global,
                &state_for_added,
                &metadata_for_added,
                &watchers_for_added,
            );
        })
        .global_remove(move |id| {
            handle_global_removed(
                id,
                &state_for_removed,
                &metadata_for_removed,
                &watchers_for_removed,
            );
        })
        .register();

    if let Err(err) = core.sync(0) {
        eprintln!("[registry] failed to sync core: {err}");
    }

    println!("[registry] PipeWire registry thread running");
    mainloop.run();
    println!("[registry] PipeWire registry loop exited");

    // Drop resources tied to the loop before returning.
    drop(registry);
    drop(context);

    Ok(())
}

fn handle_global_added(
    registry: &RegistryRc,
    global: &GlobalObject<&DictRef>,
    shared_state: &Arc<RwLock<RegistryState>>,
    metadata_bindings: &Rc<RefCell<HashMap<u32, MetadataBinding>>>,
    watchers: &Arc<RwLock<Vec<mpsc::Sender<RegistrySnapshot>>>>,
) {
    match global.type_ {
        ObjectType::Node => {
            let info = build_node_info(global);
            mutate_state(shared_state, watchers, move |state| state.upsert_node(info));
        }
        ObjectType::Device => {
            let info = build_device_info(global);
            mutate_state(shared_state, watchers, move |state| {
                state.upsert_device(info)
            });
        }
        ObjectType::Metadata => {
            process_metadata_added(registry, global, shared_state, metadata_bindings, watchers);
        }
        _ => {}
    }
}

fn handle_global_removed(
    id: u32,
    shared_state: &Arc<RwLock<RegistryState>>,
    metadata_bindings: &Rc<RefCell<HashMap<u32, MetadataBinding>>>,
    watchers: &Arc<RwLock<Vec<mpsc::Sender<RegistrySnapshot>>>>,
) {
    if mutate_state(shared_state, watchers, |state| state.remove_node(id)) {
        return;
    }

    if mutate_state(shared_state, watchers, |state| state.remove_device(id)) {
        return;
    }

    if metadata_bindings.borrow_mut().remove(&id).is_some() {
        mutate_state(shared_state, watchers, |state| {
            state.clear_metadata_defaults(id)
        });
    }
}

fn process_metadata_added(
    registry: &RegistryRc,
    global: &GlobalObject<&DictRef>,
    shared_state: &Arc<RwLock<RegistryState>>,
    metadata_bindings: &Rc<RefCell<HashMap<u32, MetadataBinding>>>,
    watchers: &Arc<RwLock<Vec<mpsc::Sender<RegistrySnapshot>>>>,
) {
    let metadata_id = global.id;
    if metadata_bindings.borrow().contains_key(&metadata_id) {
        return;
    }

    let props = util::dict_to_map(global.props.as_ref().copied());
    let metadata_name = props.get("metadata.name").cloned();

    let metadata = match registry.bind::<Metadata, _>(global) {
        Ok(metadata) => metadata,
        Err(err) => {
            eprintln!("[registry] failed to bind metadata {metadata_id}: {err}");
            return;
        }
    };

    let state_for_listener = Arc::clone(shared_state);
    let watchers_for_listener = Arc::clone(watchers);
    let listener = metadata
        .add_listener_local()
        .property(move |subject, key, type_, value| {
            handle_metadata_property(
                &state_for_listener,
                &watchers_for_listener,
                metadata_id,
                subject,
                key,
                type_,
                value,
            );
            0
        })
        .register();

    metadata_bindings.borrow_mut().insert(
        metadata_id,
        MetadataBinding {
            _proxy: metadata,
            _listener: listener,
            name: metadata_name,
        },
    );
}

fn handle_metadata_property(
    shared_state: &Arc<RwLock<RegistryState>>,
    watchers: &Arc<RwLock<Vec<mpsc::Sender<RegistrySnapshot>>>>,
    metadata_id: u32,
    subject: u32,
    key: Option<&str>,
    type_hint: Option<&str>,
    value: Option<&str>,
) {
    mutate_state(shared_state, watchers, |state| {
        state.apply_metadata_property(metadata_id, subject, key, type_hint, value)
    });
}

fn mutate_state<F>(
    shared_state: &Arc<RwLock<RegistryState>>,
    watchers: &Arc<RwLock<Vec<mpsc::Sender<RegistrySnapshot>>>>,
    mutate: F,
) -> bool
where
    F: FnOnce(&mut RegistryState) -> bool,
{
    let changed = {
        let mut state = shared_state.write();
        mutate(&mut state)
    };

    if changed {
        notify_watchers(shared_state, watchers);
    }

    changed
}

fn notify_watchers(
    shared_state: &Arc<RwLock<RegistryState>>,
    watchers: &Arc<RwLock<Vec<mpsc::Sender<RegistrySnapshot>>>>,
) {
    let snapshot = {
        let state = shared_state.read();
        state.snapshot()
    };

    let mut guard = watchers.write();
    guard.retain(|sender| sender.send(snapshot.clone()).is_ok());
}

fn build_node_info(global: &GlobalObject<&DictRef>) -> NodeInfo {
    let props = util::dict_to_map(global.props.as_ref().copied());

    let summary = GraphNode::from_props(global.id, &props);
    let name = summary.name().map(|value| value.to_string());
    let description = summary.description().map(|value| value.to_string());

    let media_class = props.get(*pw::keys::MEDIA_CLASS).cloned();
    let media_role = props.get(*pw::keys::MEDIA_ROLE).cloned();

    let direction = derive_direction(media_class.as_deref(), &props);
    let parent_device = props.get("device.id").and_then(|id| id.parse::<u32>().ok());
    let is_virtual = props
        .get("node.virtual")
        .map(|value| value == "true")
        .unwrap_or_else(|| name.as_deref() == Some("openmeters.sink"));

    NodeInfo {
        id: global.id,
        name,
        description,
        media_class,
        media_role,
        direction,
        is_virtual,
        parent_device,
        properties: props,
    }
}

fn build_device_info(global: &GlobalObject<&DictRef>) -> DeviceInfo {
    let props = util::dict_to_map(global.props.as_ref().copied());
    let name = props.get("device.name").cloned();
    let description = props
        .get(*pw::keys::DEVICE_DESCRIPTION)
        .cloned()
        .or_else(|| props.get("device.product.name").cloned())
        .or_else(|| name.clone());

    DeviceInfo {
        id: global.id,
        name,
        description,
        properties: props,
    }
}

fn derive_direction(media_class: Option<&str>, props: &HashMap<String, String>) -> NodeDirection {
    if let Some(class) = media_class {
        let lowered = class.to_ascii_lowercase();
        if lowered.contains("sink") || lowered.contains("output") {
            return NodeDirection::Output;
        }
        if lowered.contains("source") || lowered.contains("input") {
            return NodeDirection::Input;
        }
    }

    if let Some(direction) = props.get(*pw::keys::PORT_DIRECTION) {
        match direction.to_ascii_lowercase().as_str() {
            "in" => return NodeDirection::Input,
            "out" => return NodeDirection::Output,
            _ => {}
        }
    }

    NodeDirection::Unknown
}

struct MetadataBinding {
    _proxy: Metadata,
    _listener: MetadataListener,
    #[allow(dead_code)]
    name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_direction_prefers_media_class_keywords() {
        let props = HashMap::new();
        assert_eq!(
            derive_direction(Some("Audio/Sink"), &props),
            NodeDirection::Output
        );
        assert_eq!(
            derive_direction(Some("Stream/Input/Audio"), &props),
            NodeDirection::Input
        );
    }

    #[test]
    fn derive_direction_falls_back_to_port_direction() {
        let mut props = HashMap::new();
        props.insert((*pw::keys::PORT_DIRECTION).to_string(), "in".into());
        assert_eq!(derive_direction(None, &props), NodeDirection::Input);
        props.insert((*pw::keys::PORT_DIRECTION).to_string(), "out".into());
        assert_eq!(derive_direction(None, &props), NodeDirection::Output);
    }

    #[test]
    fn metadata_defaults_reconcile_matches_by_name() {
        let mut defaults = MetadataDefaults {
            audio_sink: Some(DefaultTarget {
                metadata_id: Some(7),
                node_id: None,
                name: Some("node.main".into()),
                type_hint: None,
            }),
            audio_source: None,
        };

        let mut nodes = HashMap::new();
        nodes.insert(
            42,
            NodeInfo {
                id: 42,
                name: Some("node.main".into()),
                ..Default::default()
            },
        );

        defaults.reconcile_with_nodes(&nodes);
        assert_eq!(
            defaults.audio_sink.as_ref().and_then(|t| t.node_id),
            Some(42)
        );
    }
}
