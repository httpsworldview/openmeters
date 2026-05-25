// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

mod domain;
mod dsp;
mod infra;
mod macros;
mod persistence;
mod ui;
mod util;
mod visuals;
use domain::routing::{DeviceSelection, RoutingCommand, RoutingConfig};
use infra::pipewire::{monitor, registry, virtual_sink};
use persistence::settings::SettingsHandle;
use std::sync::{Arc, mpsc};
use ui::UiConfig;
use util::telemetry;

use tracing::{error, info};

fn main() {
    telemetry::init();
    info!("OpenMeters starting up");

    let (routing_tx, routing_rx) = mpsc::channel::<RoutingCommand>();
    let (snapshot_tx, snapshot_rx) = async_channel::bounded::<registry::RegistrySnapshot>(64);

    let settings_handle = SettingsHandle::load_or_default();
    let settings_for_shutdown = settings_handle.clone();
    let routing_config = {
        let guard = settings_handle.borrow();
        let settings = &guard.data;
        RoutingConfig {
            capture_mode: settings.capture_mode,
            preferred_device: DeviceSelection::from_token(settings.last_device_name.clone()),
        }
    };

    let registry_thread = monitor::init_registry_monitor(routing_rx, snapshot_tx, routing_config);

    virtual_sink::run();

    let audio_stream = infra::pipewire::meter_tap::audio_sample_stream();

    let ui_config = UiConfig::new(routing_tx, Some(Arc::new(snapshot_rx)), settings_handle)
        .with_audio_stream(audio_stream);

    if let Err(err) = ui::run(ui_config) {
        error!("[ui] failed: {err}");
    }
    settings_for_shutdown.flush();

    if let Some((_, handle)) = registry_thread {
        info!("[main] shutdown requested; waiting for registry monitor to exit...");
        let _ = handle.join();
    }
}
