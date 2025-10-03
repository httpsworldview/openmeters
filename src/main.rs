mod audio;
mod ui;
mod util;
use audio::{pw_loopback, pw_registry, pw_virtual_sink};
// use std::time::Duration;

fn describe_default_target(
    snapshot: &pw_registry::RegistrySnapshot,
    target: &Option<pw_registry::DefaultTarget>,
) -> (String, String) {
    let raw = target
        .as_ref()
        .and_then(|t| t.name.as_deref())
        .unwrap_or("(none)")
        .to_string();

    let resolved = target
        .as_ref()
        .and_then(|t| resolve_target_node(snapshot, t));
    let display = resolved.map(node_label).unwrap_or_else(|| raw.clone());

    (display, raw)
}

fn resolve_target_node<'a>(
    snapshot: &'a pw_registry::RegistrySnapshot,
    target: &pw_registry::DefaultTarget,
) -> Option<&'a pw_registry::NodeInfo> {
    target
        .node_id
        .and_then(|id| snapshot.nodes.iter().find(|node| node.id == id))
        .or_else(|| {
            target.name.as_ref().and_then(|name| {
                snapshot
                    .nodes
                    .iter()
                    .find(|node| node.name.as_deref() == Some(name.as_str()))
            })
        })
}

fn node_label(node: &pw_registry::NodeInfo) -> String {
    node.name
        .clone()
        .or(node.description.clone())
        .unwrap_or_else(|| format!("node#{}", node.id))
}

fn log_registry_snapshot(kind: &str, snapshot: &pw_registry::RegistrySnapshot) {
    let (default_sink, raw_sink) = describe_default_target(snapshot, &snapshot.defaults.audio_sink);
    let (default_source, raw_source) =
        describe_default_target(snapshot, &snapshot.defaults.audio_source);

    println!(
        "[registry] {kind}: serial={}, nodes={}, devices={}, default_sink={} (raw={}), default_source={} (raw={})",
        snapshot.serial,
        snapshot.nodes.len(),
        snapshot.devices.len(),
        default_sink,
        raw_sink,
        default_source,
        raw_source
    );
}

fn main() {
    println!("Hello, OpenMeters!");

    let registry_handle = match pw_registry::spawn_registry() {
        Ok(handle) => {
            let mut updates = handle.subscribe();
            let initial_snapshot = updates.recv().unwrap_or_else(|| handle.snapshot());
            log_registry_snapshot("initial snapshot", &initial_snapshot);

            if let Err(err) = std::thread::Builder::new()
                .name("openmeters-registry-monitor".into())
                .spawn(move || {
                    let mut updates = updates;
                    while let Some(snapshot) = updates.recv() {
                        log_registry_snapshot("update", &snapshot);
                    }
                    println!("[registry] update stream ended");
                })
            {
                eprintln!("[registry] failed to spawn monitor thread: {err}");
            }
            Some(handle)
        }
        Err(err) => {
            eprintln!("[registry] failed to start PipeWire registry: {err:?}");
            None
        }
    };

    pw_virtual_sink::run();
    pw_loopback::run();

    // // monitor audio buffer
    // let capture_buffer = pw_virtual_sink::capture_buffer_handle();
    // std::thread::Builder::new()
    //     .name("openmeters-buffer-monitor".into())
    //     .spawn(move || {
    //         loop {
    //             std::thread::sleep(Duration::from_millis(500));
    //             if let Ok(buffer) = capture_buffer.lock() {
    //                 let segments = buffer.len();
    //                 let samples: usize = buffer.iter().map(|chunk| chunk.len()).sum();
    //                 // XOR checksum of all samples in the buffer to detect changes
    //                 let checksum: u32 = buffer
    //                     .iter()
    //                     .flat_map(|chunk| chunk.iter())
    //                     .map(|sample| sample.to_bits())
    //                     .fold(0, |acc, x| acc ^ x);
    //                 println!(
    //                     "[monitor] captured segments={}, total_samples={} ({} bytes), checksum={:x}",
    //                     segments,
    //                     samples,
    //                     samples * std::mem::size_of::<f32>(),
    //                     checksum
    //                 );
    //             }
    //         }
    //     })
    //     .expect("failed to spawn buffer monitor thread");

    // Keep the registry handle alive for the lifetime of the process.
    let _registry_handle = registry_handle;

    if let Err(err) = ui::run() {
        eprintln!("[ui] failed to start Qt application: {err:?}");
    }
}
