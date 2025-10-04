use crate::audio::{VIRTUAL_SINK_NAME, pw_registry, pw_router};
use crate::ui::RoutingCommand;
use crate::util::log;
use async_channel::Sender;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::time::Duration;

pub fn init_registry_monitor(
    command_rx: mpsc::Receiver<RoutingCommand>,
    snapshot_tx: Sender<pw_registry::RegistrySnapshot>,
) -> Option<pw_registry::AudioRegistryHandle> {
    match pw_registry::spawn_registry() {
        Ok(handle) => {
            if let Err(err) = spawn_registry_monitor(handle.clone(), command_rx, snapshot_tx) {
                eprintln!("[registry] failed to spawn monitor thread: {err}");
            }
            Some(handle)
        }
        Err(err) => {
            eprintln!("[registry] failed to start PipeWire registry: {err:?}");
            None
        }
    }
}

fn spawn_registry_monitor(
    handle: pw_registry::AudioRegistryHandle,
    command_rx: mpsc::Receiver<RoutingCommand>,
    snapshot_tx: Sender<pw_registry::RegistrySnapshot>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("openmeters-registry-monitor".into())
        .spawn(move || run_registry_monitor(handle, command_rx, snapshot_tx))
        .map(|_| ())
}

fn run_registry_monitor(
    handle: pw_registry::AudioRegistryHandle,
    command_rx: mpsc::Receiver<RoutingCommand>,
    snapshot_tx: Sender<pw_registry::RegistrySnapshot>,
) {
    let mut updates = handle.subscribe();
    let router = match pw_router::Router::new() {
        Ok(router) => Some(router),
        Err(err) => {
            eprintln!("[router] failed to initialise PipeWire router: {err:?}");
            None
        }
    };

    let mut monitor = RegistryMonitor::new(router, command_rx);
    const POLL_INTERVAL: Duration = Duration::from_millis(100);

    loop {
        if monitor.process_pending_commands() {
            continue;
        }

        match updates.recv_timeout(POLL_INTERVAL) {
            Ok(Some(snapshot)) => {
                monitor.process_snapshot(&snapshot);
                if snapshot_tx.send_blocking(snapshot.clone()).is_err() {
                    println!("[registry] UI channel closed; stopping snapshot forwarding");
                    break;
                }
            }
            Ok(None) => continue,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    println!("[registry] update stream ended");
}

struct RegistryMonitor {
    iteration: u64,
    routing: RoutingManager,
}

impl RegistryMonitor {
    fn new(router: Option<pw_router::Router>, command_rx: mpsc::Receiver<RoutingCommand>) -> Self {
        Self {
            iteration: 0,
            routing: RoutingManager::new(router, command_rx),
        }
    }

    fn process_snapshot(&mut self, snapshot: &pw_registry::RegistrySnapshot) {
        let label = if self.iteration == 0 {
            "initial snapshot"
        } else {
            "update"
        };

        log::registry_snapshot(label, snapshot);
        self.iteration += 1;

        self.routing.handle_snapshot(snapshot);
    }

    fn process_pending_commands(&mut self) -> bool {
        self.routing.apply_pending_commands()
    }
}

struct RoutingManager {
    router: Option<pw_router::Router>,
    commands: mpsc::Receiver<RoutingCommand>,
    preferences: HashMap<u32, bool>,
    routed_nodes: HashMap<u32, RouteTarget>,
    last_sink_id: Option<u32>,
    sink_warning_logged: bool,
    last_snapshot: Option<pw_registry::RegistrySnapshot>,
    last_hardware_sink_id: Option<u32>,
    last_hardware_sink_label: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RouteTarget {
    Virtual(u32),
    Hardware(u32),
}

impl RoutingManager {
    fn new(router: Option<pw_router::Router>, commands: mpsc::Receiver<RoutingCommand>) -> Self {
        Self {
            router,
            commands,
            preferences: HashMap::new(),
            routed_nodes: HashMap::new(),
            last_sink_id: None,
            sink_warning_logged: false,
            last_snapshot: None,
            last_hardware_sink_id: None,
            last_hardware_sink_label: None,
        }
    }

    fn handle_snapshot(&mut self, snapshot: &pw_registry::RegistrySnapshot) {
        self.last_snapshot = Some(snapshot.clone());
        self.consume_commands();
        self.apply_snapshot(snapshot, true);
    }

    fn cleanup_removed_nodes(&mut self, observed: &HashSet<u32>) {
        self.preferences
            .retain(|node_id, _| observed.contains(node_id));
        self.routed_nodes
            .retain(|node_id, _| observed.contains(node_id));

        if let Some(id) = self.last_hardware_sink_id {
            if !observed.contains(&id) {
                self.last_hardware_sink_id = None;
            }
        }
    }

    fn consume_commands(&mut self) -> bool {
        let mut changed = false;
        while let Ok(command) = self.commands.try_recv() {
            match command {
                RoutingCommand::SetApplicationEnabled { node_id, enabled } => {
                    self.preferences.insert(node_id, enabled);
                    changed = true;
                }
            }
        }
        changed
    }

    fn apply_pending_commands(&mut self) -> bool {
        let changed = self.consume_commands();
        if !changed {
            return false;
        }

        if let Some(snapshot) = self.last_snapshot.clone() {
            self.apply_snapshot(&snapshot, false);
        }

        true
    }

    fn apply_snapshot(&mut self, snapshot: &pw_registry::RegistrySnapshot, log_sink_missing: bool) {
        let observed: HashSet<u32> = snapshot.nodes.iter().map(|node| node.id).collect();
        self.cleanup_removed_nodes(&observed);

        let Some(sink) = snapshot.find_node_by_label(VIRTUAL_SINK_NAME) else {
            if log_sink_missing && !self.sink_warning_logged {
                println!(
                    "[router] virtual sink '{}' not yet available; will retry on future updates",
                    VIRTUAL_SINK_NAME
                );
                self.sink_warning_logged = true;
            }
            return;
        };

        if self.last_sink_id != Some(sink.id) {
            self.routed_nodes.clear();
            self.last_sink_id = Some(sink.id);
        }

        self.sink_warning_logged = false;

        let mut hardware_sink = snapshot
            .defaults
            .audio_sink
            .as_ref()
            .and_then(|target| snapshot.resolve_default_target(target));

        if let Some(node) = hardware_sink {
            self.last_hardware_sink_id = Some(node.id);
            self.last_hardware_sink_label = Some(node.display_name());
        } else {
            if let Some(id) = self.last_hardware_sink_id {
                hardware_sink = snapshot.nodes.iter().find(|node| node.id == id);
            }
            if hardware_sink.is_none() {
                if let Some(label) = self.last_hardware_sink_label.clone() {
                    hardware_sink = snapshot
                        .nodes
                        .iter()
                        .find(|node| node.matches_label(&label));
                }
            }

            if let Some(node) = hardware_sink {
                self.last_hardware_sink_id = Some(node.id);
                self.last_hardware_sink_label = Some(node.display_name());
            } else {
                self.last_hardware_sink_id = None;
            }
        }

        for node in snapshot.route_candidates(sink) {
            let enabled = self.preferences.get(&node.id).copied().unwrap_or(true);

            if enabled {
                self.route_to_target(node, sink, RouteTarget::Virtual(sink.id));
            } else if let Some(hardware) = hardware_sink {
                self.route_to_target(node, hardware, RouteTarget::Hardware(hardware.id));
            } else if self.routed_nodes.remove(&node.id).is_some() {
                println!(
                    "[router] no hardware sink available to restore '{}' (id={})",
                    node.display_name(),
                    node.id
                );
            }
        }
    }

    fn route_to_target(
        &mut self,
        node: &pw_registry::NodeInfo,
        target: &pw_registry::NodeInfo,
        desired: RouteTarget,
    ) {
        let Some(router) = self.router.as_ref() else {
            return;
        };

        if self.routed_nodes.get(&node.id).copied() == Some(desired) {
            return;
        }

        match router.route_application_to_sink(node, target) {
            Ok(()) => {
                self.routed_nodes.insert(node.id, desired);
                let action = match desired {
                    RouteTarget::Virtual(_) => "routed",
                    RouteTarget::Hardware(_) => "restored",
                };
                println!(
                    "[router] {action} '{}' (id={}) -> '{}' (id={})",
                    node.display_name(),
                    node.id,
                    target.display_name(),
                    target.id
                );
            }
            Err(err) => {
                eprintln!(
                    "[router] failed to route node '{}' (id={}): {err:?}",
                    node.display_name(),
                    node.id
                );
            }
        }
    }
}
