//! PipeWire loopback management for OpenMeters.

use crate::util::pipewire::{
    DEFAULT_AUDIO_SINK_KEY, DefaultTarget, GraphNode, GraphPort, PortDirection,
    pair_ports_by_channel, parse_metadata_name,
};
use anyhow::{Context, Result};
use pipewire as pw;
use pw::metadata::{Metadata, MetadataListener};
use pw::properties::properties;
use pw::registry::{GlobalObject, RegistryRc};
use pw::spa::utils::dict::DictRef;
use pw::types::ObjectType;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::OnceLock;
use std::thread;

const LOOPBACK_THREAD_NAME: &str = "openmeters-pw-loopback";
const LINK_FACTORY_NAME: &str = "link-factory";
const OPENMETERS_SINK_NAME: &str = "openmeters.sink";

static LOOPBACK_THREAD: OnceLock<thread::JoinHandle<()>> = OnceLock::new();

/// Start the loopback controller in a background thread if it is not already running.
pub fn run() {
    if LOOPBACK_THREAD.get().is_some() {
        return;
    }

    match thread::Builder::new()
        .name(LOOPBACK_THREAD_NAME.into())
        .spawn(|| {
            if let Err(err) = run_loopback() {
                eprintln!("[loopback] stopped: {err:?}");
            }
        }) {
        Ok(handle) => {
            let _ = LOOPBACK_THREAD.set(handle);
        }
        Err(err) => eprintln!("[loopback] failed to spawn thread: {err}"),
    }
}

fn run_loopback() -> Result<()> {
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

    let state = Rc::new(RefCell::new(LoopbackState::new(core.clone())));
    let metadata_bindings: Rc<RefCell<HashMap<u32, MetadataBinding>>> =
        Rc::new(RefCell::new(HashMap::new()));

    let registry_for_added = registry.clone();
    let state_for_added = Rc::clone(&state);
    let metadata_for_added = Rc::clone(&metadata_bindings);
    let state_for_removed = Rc::clone(&state);
    let metadata_for_removed = Rc::clone(&metadata_bindings);

    let _registry_listener = registry
        .add_listener_local()
        .global(move |global| {
            handle_global_added(
                &registry_for_added,
                global,
                &state_for_added,
                &metadata_for_added,
            );
        })
        .global_remove(move |id| {
            handle_global_removed(id, &state_for_removed, &metadata_for_removed);
        })
        .register();

    println!("[loopback] PipeWire loopback thread running");
    mainloop.run();
    println!("[loopback] PipeWire loopback loop exited");

    drop(registry);
    drop(context);

    Ok(())
}

fn handle_global_added(
    registry: &RegistryRc,
    global: &GlobalObject<&DictRef>,
    state: &Rc<RefCell<LoopbackState>>,
    metadata_bindings: &Rc<RefCell<HashMap<u32, MetadataBinding>>>,
) {
    match global.type_ {
        ObjectType::Node => {
            if let Some(node) = GraphNode::from_global(global) {
                state.borrow_mut().upsert_node(node);
            }
        }
        ObjectType::Port => {
            if let Some(port) = GraphPort::from_global(global) {
                state.borrow_mut().upsert_port(port);
            }
        }
        ObjectType::Metadata => {
            process_metadata_added(registry, global, state, metadata_bindings);
        }
        _ => {}
    }
}

fn handle_global_removed(
    id: u32,
    state: &Rc<RefCell<LoopbackState>>,
    metadata_bindings: &Rc<RefCell<HashMap<u32, MetadataBinding>>>,
) {
    if state.borrow_mut().remove_port_by_global(id) {
        return;
    }

    if state.borrow_mut().remove_node(id) {
        return;
    }

    if metadata_bindings.borrow_mut().remove(&id).is_some() {
        state.borrow_mut().clear_metadata(id);
    }
}

fn process_metadata_added(
    registry: &RegistryRc,
    global: &GlobalObject<&DictRef>,
    state: &Rc<RefCell<LoopbackState>>,
    metadata_bindings: &Rc<RefCell<HashMap<u32, MetadataBinding>>>,
) {
    let metadata_id = global.id;
    if metadata_bindings.borrow().contains_key(&metadata_id) {
        return;
    }

    let metadata = match registry.bind::<Metadata, _>(global) {
        Ok(metadata) => metadata,
        Err(err) => {
            eprintln!("[loopback] failed to bind metadata {metadata_id}: {err}");
            return;
        }
    };

    let state_for_listener = Rc::clone(state);
    let metadata_listener = metadata
        .add_listener_local()
        .property(move |subject, key, type_hint, value| {
            handle_metadata_property(
                &state_for_listener,
                metadata_id,
                subject,
                key,
                type_hint,
                value,
            );
            0
        })
        .register();

    metadata_bindings.borrow_mut().insert(
        metadata_id,
        MetadataBinding {
            _proxy: metadata,
            _listener: metadata_listener,
        },
    );
}

fn handle_metadata_property(
    state: &Rc<RefCell<LoopbackState>>,
    metadata_id: u32,
    subject: u32,
    key: Option<&str>,
    type_hint: Option<&str>,
    value: Option<&str>,
) {
    if key != Some(DEFAULT_AUDIO_SINK_KEY) {
        return;
    }

    state
        .borrow_mut()
        .update_default_sink(metadata_id, subject, type_hint, value);
}

struct MetadataBinding {
    #[allow(dead_code)]
    _proxy: Metadata,
    #[allow(dead_code)]
    _listener: MetadataListener,
}

struct LoopbackState {
    core: pw::core::CoreRc,
    nodes: HashMap<u32, TrackedNode>,
    port_index: HashMap<u32, (u32, u32)>,
    default_sink: DefaultTarget,
    openmeters_node_id: Option<u32>,
    active_links: HashMap<LinkKey, pw::link::Link>,
}

impl LoopbackState {
    fn new(core: pw::core::CoreRc) -> Self {
        Self {
            core,
            nodes: HashMap::new(),
            port_index: HashMap::new(),
            default_sink: DefaultTarget::default(),
            openmeters_node_id: None,
            active_links: HashMap::new(),
        }
    }

    fn upsert_node(&mut self, node: GraphNode) {
        let node_id = node.id();
        let entry = self
            .nodes
            .entry(node_id)
            .or_insert_with(TrackedNode::default);
        entry.set_info(node);

        let is_openmeters = entry.has_name(OPENMETERS_SINK_NAME);
        if is_openmeters {
            if self.openmeters_node_id != Some(node_id) {
                println!("[loopback] detected OpenMeters sink node #{node_id}");
            }
            self.openmeters_node_id = Some(node_id);
        } else if self.openmeters_node_id == Some(node_id) {
            println!("[loopback] OpenMeters sink node removed");
            self.openmeters_node_id = None;
        }

        self.resolve_default_sink_node();
        self.refresh_links();
    }

    fn remove_node(&mut self, node_id: u32) -> bool {
        let existed = self.nodes.remove(&node_id).is_some();
        if !existed {
            return false;
        }

        self.port_index.retain(|_, (owner, _)| *owner != node_id);

        if self.openmeters_node_id == Some(node_id) {
            println!("[loopback] OpenMeters sink node removed");
            self.openmeters_node_id = None;
        }

        if self.default_sink.node_id == Some(node_id) {
            self.default_sink.node_id = None;
        }

        self.resolve_default_sink_node();
        self.refresh_links();
        true
    }

    fn upsert_port(&mut self, port: GraphPort) {
        let node_id = port.node_id;
        let port_id = port.port_id;
        let node = self
            .nodes
            .entry(node_id)
            .or_insert_with(TrackedNode::default);
        node.upsert_port(port.clone());
        self.port_index.insert(port.global_id, (node_id, port_id));

        if self.is_tracked_node(node_id) {
            println!(
                "[loopback] tracked port discovered: node={} port={} dir={:?} monitor={} channel={} name={}",
                node_id,
                port_id,
                port.direction,
                port.is_monitor,
                port.channel.as_deref().unwrap_or("unknown"),
                port.name.as_deref().unwrap_or("unnamed")
            );
            self.refresh_links();
        }
    }

    fn remove_port_by_global(&mut self, global_id: u32) -> bool {
        let Some((node_id, port_id)) = self.port_index.remove(&global_id) else {
            return false;
        };

        if let Some(node) = self.nodes.get_mut(&node_id) {
            node.remove_port(port_id);
        }

        if self.is_tracked_node(node_id) {
            println!(
                "[loopback] tracked port removed: node={} port={}",
                node_id, port_id
            );
            self.refresh_links();
        }

        true
    }

    fn update_default_sink(
        &mut self,
        metadata_id: u32,
        subject: u32,
        type_hint: Option<&str>,
        value: Option<&str>,
    ) {
        let parsed_name = parse_metadata_name(type_hint, value);
        let changed =
            self.default_sink
                .update(metadata_id, subject, type_hint, parsed_name.as_deref());

        self.resolve_default_sink_node();

        if changed {
            if let Some(node_id) = self.default_sink.node_id {
                println!("[loopback] default audio sink is node #{node_id}");
            } else if let Some(name) = self.default_sink.name.as_deref() {
                println!("[loopback] default audio sink set to '{name}' (node unresolved)");
            } else {
                println!("[loopback] default audio sink cleared");
            }
        }

        self.refresh_links();
    }

    fn clear_metadata(&mut self, metadata_id: u32) {
        if self.default_sink.metadata_id == Some(metadata_id) {
            self.default_sink.clear();
            self.refresh_links();
        }
    }

    fn resolve_default_sink_node(&mut self) {
        if let Some(node_id) = self.default_sink.node_id {
            if self.nodes.contains_key(&node_id) {
                return;
            }
        }

        if let Some(name) = self.default_sink.name.clone() {
            if let Some(node_id) = self
                .nodes
                .iter()
                .find_map(|(&id, node)| node.matches_name(&name).then_some(id))
            {
                self.default_sink.node_id = Some(node_id);
            }
        }
    }

    fn is_tracked_node(&self, node_id: u32) -> bool {
        self.openmeters_node_id == Some(node_id) || self.default_sink.node_id == Some(node_id)
    }

    fn refresh_links(&mut self) {
        let Some(source_id) = self.openmeters_node_id else {
            self.clear_links();
            return;
        };

        let Some(target_id) = self.default_sink.node_id else {
            self.clear_links();
            return;
        };

        let Some(source_ports) = self.select_ports(
            source_id,
            "source",
            |node| node.output_ports_for_loopback(),
            |id| {
                println!(
                    "[loopback] no output ports available on node {} for loopback",
                    id
                )
            },
        ) else {
            return;
        };

        let Some(target_ports) = self.select_ports(
            target_id,
            "target",
            |node| node.input_ports_for_loopback(),
            |id| {
                println!(
                    "[loopback] no input ports available on node {} for loopback",
                    id
                )
            },
        ) else {
            return;
        };

        let plans = pair_ports_by_channel(source_ports, target_ports);
        let desired_keys: HashSet<LinkKey> = plans
            .iter()
            .map(|(out_port, in_port)| LinkKey {
                output_node: source_id,
                output_port: out_port.port_id,
                input_node: target_id,
                input_port: in_port.port_id,
            })
            .collect();

        let existing_keys: Vec<LinkKey> = self.active_links.keys().copied().collect();
        for key in existing_keys {
            if !desired_keys.contains(&key) {
                if let Some(link) = self.active_links.remove(&key) {
                    drop(link);
                }
                println!(
                    "[loopback] removed link {}:{} -> {}:{}",
                    key.output_node, key.output_port, key.input_node, key.input_port
                );
            }
        }

        for (output_port, input_port) in plans {
            let key = LinkKey {
                output_node: source_id,
                output_port: output_port.port_id,
                input_node: target_id,
                input_port: input_port.port_id,
            };
            if self.active_links.contains_key(&key) {
                continue;
            }

            match self.create_link(source_id, target_id, &output_port, &input_port) {
                Ok(link) => {
                    println!(
                        "[loopback] linking {}:{}({}/{}) -> {}:{}({}/{})",
                        source_id,
                        output_port.port_id,
                        output_port.name.as_deref().unwrap_or("unnamed"),
                        output_port.channel.as_deref().unwrap_or("unknown"),
                        target_id,
                        input_port.port_id,
                        input_port.name.as_deref().unwrap_or("unnamed"),
                        input_port.channel.as_deref().unwrap_or("unknown")
                    );
                    self.active_links.insert(key, link);
                }
                Err(err) => {
                    eprintln!(
                        "[loopback] failed to create link {}:{} -> {}:{}: {err}",
                        source_id, output_port.port_id, target_id, input_port.port_id
                    );
                }
            }
        }
    }

    fn select_ports<F, L>(
        &mut self,
        node_id: u32,
        label: &str,
        selector: F,
        on_empty: L,
    ) -> Option<Vec<GraphPort>>
    where
        F: Fn(&TrackedNode) -> Vec<GraphPort>,
        L: Fn(u32),
    {
        let node = match self.nodes.get(&node_id) {
            Some(node) => node,
            None => {
                self.clear_links();
                return None;
            }
        };

        let ports = selector(node);
        if ports.is_empty() {
            let snapshot = node.clone_ports();
            self.clear_links();
            Self::dump_ports_snapshot(label, node_id, &snapshot);
            on_empty(node_id);
            return None;
        }

        Some(ports)
    }

    fn clear_links(&mut self) {
        if self.active_links.is_empty() {
            return;
        }

        self.active_links.clear();
        println!("[loopback] cleared all active links");
    }

    fn dump_ports_snapshot(label: &str, node_id: u32, ports: &[GraphPort]) {
        if ports.is_empty() {
            println!("[loopback] {label} node {} has no known ports", node_id);
            return;
        }

        println!(
            "[loopback] {label} node {} port inventory ({} ports):",
            node_id,
            ports.len()
        );
        for port in ports {
            println!(
                "[loopback]   port={} dir={:?} monitor={} channel={} name={}",
                port.port_id,
                port.direction,
                port.is_monitor,
                port.channel.as_deref().unwrap_or("unknown"),
                port.name.as_deref().unwrap_or("unnamed")
            );
        }
    }

    fn create_link(
        &self,
        output_node: u32,
        input_node: u32,
        output_port: &GraphPort,
        input_port: &GraphPort,
    ) -> Result<pw::link::Link, pw::Error> {
        let props = properties! {
            *pw::keys::LINK_OUTPUT_NODE => output_node.to_string(),
            *pw::keys::LINK_OUTPUT_PORT => output_port.port_id.to_string(),
            *pw::keys::LINK_INPUT_NODE => input_node.to_string(),
            *pw::keys::LINK_INPUT_PORT => input_port.port_id.to_string(),
            *pw::keys::LINK_PASSIVE => "true",
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Playback",
            *pw::keys::MEDIA_ROLE => "Playback",
        };

        self.core
            .create_object::<pw::link::Link>(LINK_FACTORY_NAME, &props)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct LinkKey {
    output_node: u32,
    output_port: u32,
    input_node: u32,
    input_port: u32,
}

struct TrackedNode {
    info: Option<GraphNode>,
    ports: HashMap<u32, GraphPort>,
}

impl Default for TrackedNode {
    fn default() -> Self {
        Self {
            info: None,
            ports: HashMap::new(),
        }
    }
}

impl TrackedNode {
    fn set_info(&mut self, info: GraphNode) {
        self.info = Some(info);
    }

    fn upsert_port(&mut self, port: GraphPort) {
        self.ports.insert(port.port_id, port);
    }

    fn remove_port(&mut self, port_id: u32) {
        self.ports.remove(&port_id);
    }

    fn matches_name(&self, candidate: &str) -> bool {
        self.info
            .as_ref()
            .is_some_and(|info| info.matches_name(candidate))
    }

    fn has_name(&self, name: &str) -> bool {
        self.info.as_ref().is_some_and(|info| info.has_name(name))
    }

    fn output_ports_for_loopback(&self) -> Vec<GraphPort> {
        let monitor_ports: Vec<_> = self
            .ports
            .values()
            .filter(|port| port.direction == PortDirection::Output && port.is_monitor)
            .cloned()
            .collect();

        if !monitor_ports.is_empty() {
            return monitor_ports;
        }

        let output_ports: Vec<_> = self
            .ports
            .values()
            .filter(|port| port.direction == PortDirection::Output)
            .cloned()
            .collect();
        if !output_ports.is_empty() {
            return output_ports;
        }

        self.ports.values().cloned().collect()
    }

    fn input_ports_for_loopback(&self) -> Vec<GraphPort> {
        let playback_ports: Vec<_> = self
            .ports
            .values()
            .filter(|port| port.direction == PortDirection::Input && !port.is_monitor)
            .cloned()
            .collect();

        if !playback_ports.is_empty() {
            return playback_ports;
        }

        let input_ports: Vec<_> = self
            .ports
            .values()
            .filter(|port| port.direction == PortDirection::Input)
            .cloned()
            .collect();
        if !input_ports.is_empty() {
            return input_ports;
        }

        self.ports.values().cloned().collect()
    }

    fn clone_ports(&self) -> Vec<GraphPort> {
        self.ports.values().cloned().collect()
    }
}
