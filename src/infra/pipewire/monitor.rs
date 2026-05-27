// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{VIRTUAL_SINK_NAME, registry};
use crate::domain::routing::{CaptureMode, DeviceSelection, RoutingCommand, RoutingConfig};
use async_channel::{Sender, TrySendError};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use tracing::{debug, info, warn};

const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

pub fn init_registry_monitor(
    command_rx: mpsc::Receiver<RoutingCommand>,
    snapshot_tx: Sender<registry::RegistrySnapshot>,
    routing_config: RoutingConfig,
) -> Option<(registry::AudioRegistryHandle, std::thread::JoinHandle<()>)> {
    let handle = registry::spawn_registry()
        .inspect_err(|err| {
            tracing::error!("[registry-monitor] failed to start PipeWire registry: {err:?}");
        })
        .ok()?;

    let handle_for_thread = handle.clone();
    let thread_handle = std::thread::Builder::new()
        .name("openmeters-registry-monitor".into())
        .spawn(move || run_monitor_loop(handle_for_thread, command_rx, snapshot_tx, routing_config))
        .inspect_err(|err| {
            tracing::error!("[registry-monitor] failed to spawn monitor thread: {err}");
        })
        .ok()?;

    Some((handle, thread_handle))
}

// Returns true when the channel closed and the loop should stop.
fn try_send_or_queue(
    tx: &Sender<registry::RegistrySnapshot>,
    snapshot: registry::RegistrySnapshot,
    pending: &mut Option<registry::RegistrySnapshot>,
) -> bool {
    match tx.try_send(snapshot) {
        Ok(()) => {}
        Err(TrySendError::Full(s)) => *pending = Some(s),
        Err(TrySendError::Closed(_)) => return true,
    }
    false
}

fn run_monitor_loop(
    handle: registry::AudioRegistryHandle,
    command_rx: mpsc::Receiver<RoutingCommand>,
    snapshot_tx: Sender<registry::RegistrySnapshot>,
    routing_config: RoutingConfig,
) {
    const CLOSED_MSG: &str = "[registry-monitor] UI channel closed; stopping";
    let mut updates = handle.subscribe();
    let mut routing = RoutingManager::new(handle, command_rx, routing_config);
    let mut last_snapshot: Option<registry::RegistrySnapshot> = None;
    let mut pending_ui_snapshot: Option<registry::RegistrySnapshot> = None;

    let flush_pending = |pending: &mut Option<registry::RegistrySnapshot>| -> bool {
        let Some(snapshot) = pending.take() else {
            return false;
        };
        let closed = try_send_or_queue(&snapshot_tx, snapshot, pending);
        if closed {
            info!("{CLOSED_MSG}");
        }
        closed
    };

    loop {
        if snapshot_tx.is_closed() {
            info!("{CLOSED_MSG}");
            break;
        }

        if routing.process_commands()
            && let Some(snapshot) = last_snapshot.as_ref()
        {
            routing.apply(snapshot);
        }

        if flush_pending(&mut pending_ui_snapshot) {
            break;
        }

        match updates.recv_timeout(POLL_INTERVAL) {
            Ok(Some(snapshot)) => {
                log_registry_snapshot(&snapshot);
                routing.apply(&snapshot);
                last_snapshot = Some(snapshot.clone());
                if try_send_or_queue(&snapshot_tx, snapshot, &mut pending_ui_snapshot) {
                    info!("{CLOSED_MSG}");
                    break;
                }
            }
            Ok(None) => {
                if flush_pending(&mut pending_ui_snapshot) {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    info!("[registry-monitor] update stream ended");
    restore_all_routes(&mut routing, last_snapshot.as_ref());
}

fn restore_all_routes(routing: &mut RoutingManager, snapshot: Option<&registry::RegistrySnapshot>) {
    let Some(snapshot) = snapshot else { return };

    let routed_nodes: Vec<_> = routing.routed_to.keys().copied().collect();
    if !routed_nodes.is_empty() {
        info!(
            "[registry-monitor] restoring {} routed node(s)...",
            routed_nodes.len()
        );

        let hw_sink = routing.hw_sink(snapshot).cloned();
        let find_node = |id: &u32| snapshot.nodes.iter().find(|node| node.id == *id);

        for node in routed_nodes.iter().filter_map(&find_node) {
            if let Some(sink) = hw_sink.as_ref() {
                routing.handle.route_node(node, sink);
            } else {
                // relying on the policy manager to pick a default.
                routing.handle.reset_route(node);
            }
        }

        // Wait for the audio server to process the re-routing messages.
        if !routing.handle.sync() {
            warn!("[registry-monitor] failed to sync with registry thread");
        }

        for node in routed_nodes.iter().filter_map(find_node) {
            routing.handle.reset_route(node);
        }
    }

    routing.handle.destroy();
}

struct RoutingManager {
    handle: registry::AudioRegistryHandle,
    commands: mpsc::Receiver<RoutingCommand>,
    disabled_nodes: HashSet<u32>,
    routed_to: HashMap<u32, u32>,
    capture_mode: CaptureMode,
    device_target: DeviceSelection,
    hw_sink_cache: Option<(u32, String)>,
    current_links: Vec<registry::LinkSpec>,
    warned_sink_missing: bool,
    warned_device_missing: bool,
}

impl RoutingManager {
    fn new(
        handle: registry::AudioRegistryHandle,
        commands: mpsc::Receiver<RoutingCommand>,
        routing_config: RoutingConfig,
    ) -> Self {
        Self {
            handle,
            commands,
            disabled_nodes: HashSet::default(),
            routed_to: HashMap::default(),
            capture_mode: routing_config.capture_mode,
            device_target: routing_config.preferred_device,
            hw_sink_cache: None,
            current_links: Vec::new(),
            warned_sink_missing: false,
            warned_device_missing: false,
        }
    }

    fn process_commands(&mut self) -> bool {
        let mut changed = false;
        while let Ok(cmd) = self.commands.try_recv() {
            changed |= match cmd {
                RoutingCommand::SetApplicationEnabled { node_id, enabled } => {
                    if enabled {
                        self.disabled_nodes.remove(&node_id)
                    } else {
                        self.disabled_nodes.insert(node_id)
                    }
                }
                RoutingCommand::SetCaptureState(mode, device) => {
                    let changed = self.capture_mode != mode || self.device_target != device;
                    self.capture_mode = mode;
                    self.device_target = device;
                    changed
                }
            };
        }
        changed
    }

    fn apply(&mut self, snapshot: &registry::RegistrySnapshot) {
        let node_exists = |id| snapshot.nodes.iter().any(|n| n.id == id);
        self.disabled_nodes.retain(|&id| node_exists(id));
        self.routed_to.retain(|&id, _| node_exists(id));
        if self
            .hw_sink_cache
            .as_ref()
            .is_some_and(|(id, _)| !node_exists(*id))
        {
            self.hw_sink_cache = None;
        }

        let links = self.compute_links(snapshot).unwrap_or_default();
        if self.current_links != links && self.handle.set_links(links.clone()) {
            self.current_links = links;
        }

        self.update_routes(snapshot);
    }

    fn update_routes(&mut self, snapshot: &registry::RegistrySnapshot) {
        let Some(sink) = snapshot.find_node_by_label(VIRTUAL_SINK_NAME) else {
            if !self.warned_sink_missing {
                warn!("[router] virtual sink '{VIRTUAL_SINK_NAME}' not yet available");
                self.warned_sink_missing = true;
            }
            return;
        };
        self.warned_sink_missing = false;
        let hw_sink = self.hw_sink(snapshot);

        for node in snapshot.route_candidates(sink) {
            let target = match self.capture_mode {
                CaptureMode::Applications if !self.disabled_nodes.contains(&node.id) => Some(sink),
                _ => hw_sink,
            };

            let Some(target) = target else {
                if self.routed_to.remove(&node.id).is_some() {
                    warn!(
                        "[router] unable to restore '{}'; no sink available",
                        node.display_name()
                    );
                }
                continue;
            };

            if self.routed_to.get(&node.id) != Some(&target.id)
                && self.handle.route_node(node, target)
            {
                info!(
                    "[router] routed '{}' -> '{}'",
                    node.display_name(),
                    target.display_name()
                );
                self.routed_to.insert(node.id, target.id);
            }
        }
    }

    fn hw_sink<'a>(
        &mut self,
        snapshot: &'a registry::RegistrySnapshot,
    ) -> Option<&'a registry::NodeInfo> {
        let node = snapshot
            .defaults
            .audio_sink
            .as_ref()
            .and_then(|t| snapshot.resolve_default_target(t))
            .or_else(|| {
                let (id, label) = self.hw_sink_cache.as_ref()?;
                snapshot
                    .nodes
                    .iter()
                    .find(|n| n.id == *id || n.matches_label(label))
            });
        self.hw_sink_cache = node.map(|n| (n.id, n.display_name()));
        node
    }

    fn compute_links(
        &mut self,
        snapshot: &registry::RegistrySnapshot,
    ) -> Option<Vec<registry::LinkSpec>> {
        let om_sink = snapshot.find_node_by_label(VIRTUAL_SINK_NAME)?;

        let (source, target) = match self.capture_mode {
            CaptureMode::Applications => (om_sink, self.hw_sink(snapshot)?),
            CaptureMode::Device => (self.device_source(snapshot)?, om_sink),
        };

        let (src_ports, tgt_ports) = (
            source.output_ports_for_loopback(),
            target.input_ports_for_loopback(),
        );
        if src_ports.is_empty() {
            debug!("[loopback] no output ports on '{}'", source.display_name());
            return None;
        }
        if tgt_ports.is_empty() {
            debug!("[loopback] no input ports on '{}'", target.display_name());
            return None;
        }

        Some(
            registry::pair_ports_by_channel(src_ports, tgt_ports)
                .into_iter()
                .map(|(out, inp)| registry::LinkSpec {
                    output_node: source.id,
                    output_port: out.port_id,
                    input_node: target.id,
                    input_port: inp.port_id,
                })
                .collect(),
        )
    }

    fn device_source<'a>(
        &mut self,
        snapshot: &'a registry::RegistrySnapshot,
    ) -> Option<&'a registry::NodeInfo> {
        match &self.device_target {
            DeviceSelection::Default => self.hw_sink(snapshot),
            DeviceSelection::Device(token) => {
                let device = snapshot.find_capture_device_by_token(token);
                if device.is_some() {
                    self.warned_device_missing = false;
                } else if !self.warned_device_missing {
                    warn!("[router] preferred capture device unavailable; waiting");
                    self.warned_device_missing = true;
                }
                device
            }
        }
    }
}

fn log_registry_snapshot(snapshot: &registry::RegistrySnapshot) {
    let sink = snapshot.describe_default_target(snapshot.defaults.audio_sink.as_ref());
    let source = snapshot.describe_default_target(snapshot.defaults.audio_source.as_ref());

    debug!(
        "[registry-monitor] update: serial={}, nodes={}, devices={}, sink={} (raw={}), source={} (raw={})",
        snapshot.serial,
        snapshot.nodes.len(),
        snapshot.device_count,
        sink.display,
        sink.raw,
        source.display,
        source.raw
    );
}
