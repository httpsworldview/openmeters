mod audio;
mod ui;
mod util;
use audio::{pw_loopback, pw_registry, pw_router, pw_virtual_sink};
use std::collections::{HashMap, HashSet};

const VIRTUAL_SINK_NAME: &str = "openmeters.sink";

fn main() {
    println!("Hello, OpenMeters!");

    let registry_handle = init_registry_monitor();

    pw_virtual_sink::run();
    pw_loopback::run();

    let _registry_handle = registry_handle;

    if let Err(err) = ui::bootstrap::run() {
        eprintln!("[ui] failed to start Qt application: {err:?}");
    }
}

fn init_registry_monitor() -> Option<pw_registry::AudioRegistryHandle> {
    match pw_registry::spawn_registry() {
        Ok(handle) => {
            if let Err(err) = spawn_registry_monitor(handle.clone()) {
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

fn spawn_registry_monitor(handle: pw_registry::AudioRegistryHandle) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("openmeters-registry-monitor".into())
        .spawn(move || run_registry_monitor(handle))
        .map(|_| ())
}

fn run_registry_monitor(handle: pw_registry::AudioRegistryHandle) {
    let mut updates = handle.subscribe();
    let router = match pw_router::Router::new() {
        Ok(router) => Some(router),
        Err(err) => {
            eprintln!("[router] failed to initialise PipeWire router: {err:?}");
            None
        }
    };

    let mut monitor = RegistryMonitor::new(router);

    while let Some(snapshot) = updates.recv() {
        monitor.process_snapshot(&snapshot);
    }

    println!("[registry] update stream ended");
}

fn log_registry_snapshot(kind: &str, snapshot: &pw_registry::RegistrySnapshot) {
    let sink_summary = snapshot.describe_default_target(snapshot.defaults.audio_sink.as_ref());
    let source_summary = snapshot.describe_default_target(snapshot.defaults.audio_source.as_ref());

    println!(
        "[registry] {kind}: serial={}, nodes={}, devices={}, default_sink={} (raw={}), default_source={} (raw={})",
        snapshot.serial,
        snapshot.nodes.len(),
        snapshot.devices.len(),
        sink_summary.display,
        sink_summary.raw,
        source_summary.display,
        source_summary.raw
    );
}

struct RegistryMonitor {
    iteration: u64,
    routing: RoutingManager,
}

impl RegistryMonitor {
    fn new(router: Option<pw_router::Router>) -> Self {
        Self {
            iteration: 0,
            routing: RoutingManager::new(router),
        }
    }

    fn process_snapshot(&mut self, snapshot: &pw_registry::RegistrySnapshot) {
        let label = if self.iteration == 0 {
            "initial snapshot"
        } else {
            "update"
        };

        log_registry_snapshot(label, snapshot);
        self.iteration += 1;

        self.routing.handle_snapshot(snapshot);
    }
}

struct RoutingManager {
    router: Option<pw_router::Router>,
    routed_nodes: HashMap<u32, u32>,
    last_sink_id: Option<u32>,
    sink_warning_logged: bool,
}

impl RoutingManager {
    fn new(router: Option<pw_router::Router>) -> Self {
        Self {
            router,
            routed_nodes: HashMap::new(),
            last_sink_id: None,
            sink_warning_logged: false,
        }
    }

    fn handle_snapshot(&mut self, snapshot: &pw_registry::RegistrySnapshot) {
        if self.router.is_none() {
            return;
        }

        if let Some(sink) = snapshot.find_node_by_label(VIRTUAL_SINK_NAME) {
            if self.last_sink_id != Some(sink.id) {
                self.routed_nodes.clear();
                self.last_sink_id = Some(sink.id);
            }

            self.route_applications(snapshot, sink);
            self.sink_warning_logged = false;
        } else if !self.sink_warning_logged {
            println!(
                "[router] virtual sink '{}' not yet available; will retry on future updates",
                VIRTUAL_SINK_NAME
            );
            self.sink_warning_logged = true;
        }
    }

    fn route_applications(
        &mut self,
        snapshot: &pw_registry::RegistrySnapshot,
        sink: &pw_registry::NodeInfo,
    ) {
        let Some(router) = self.router.as_ref() else {
            return;
        };

        let observed: HashSet<u32> = snapshot.nodes.iter().map(|node| node.id).collect();
        self.routed_nodes
            .retain(|node_id, _| observed.contains(node_id));

        let sink_id = sink.id;
        let sink_label = sink.display_name();

        for node in snapshot.route_candidates(sink) {
            let was_same_target = self
                .routed_nodes
                .get(&node.id)
                .is_some_and(|&target| target == sink_id);

            match router.route_application_to_sink(node, sink) {
                Ok(()) => {
                    self.routed_nodes.insert(node.id, sink_id);
                    if !was_same_target {
                        println!(
                            "[router] routed '{}' (id={}) -> '{}' (id={sink_id})",
                            node.display_name(),
                            node.id,
                            sink_label
                        );
                    }
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
}
