mod domain;
mod dsp;
mod infra;
mod persistence;
mod ui;
mod util;
mod visuals;
use domain::routing::{RoutingCommand, RoutingConfig};
use infra::pipewire::{monitor, registry, virtual_sink};
use persistence::settings::SettingsManager;
use std::sync::{Arc, mpsc};
use ui::UiConfig;
use util::telemetry;

use tracing::{error, info};

fn main() {
    telemetry::init();
    info!("OpenMeters starting up");

    let (routing_tx, routing_rx) = mpsc::channel::<RoutingCommand>();
    let (snapshot_tx, snapshot_rx) = async_channel::bounded::<registry::RegistrySnapshot>(64);

    let settings = SettingsManager::load_or_default();
    let routing_config = RoutingConfig {
        capture_mode: settings.settings().capture_mode,
        preferred_device: settings.settings().last_device_name.clone(),
    };

    let registry_thread = monitor::init_registry_monitor(routing_rx, snapshot_tx.clone(), routing_config);

    let _sink_thread = virtual_sink::run();

    let audio_stream = infra::pipewire::meter_tap::audio_sample_stream();

    let ui_config =
        UiConfig::new(routing_tx, Some(Arc::new(snapshot_rx))).with_audio_stream(audio_stream);

    drop(snapshot_tx);

    if let Err(err) = ui::run(ui_config) {
        error!("[ui] failed: {err}");
    }

    if let Some((_, handle)) = registry_thread {
        info!("[main] shutdown requested; waiting for registry monitor to exit...");
        let _ = handle.join();
    }
}
