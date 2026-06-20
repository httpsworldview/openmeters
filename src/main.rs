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
use infra::pipewire::{meter_tap, monitor, registry, virtual_sink};
use persistence::settings::SettingsHandle;
use std::{
    process::ExitCode,
    sync::{Arc, mpsc},
};
use ui::UiConfig;
use util::telemetry;

use tracing::{error, info};

fn main() -> ExitCode {
    telemetry::init();
    info!("OpenMeters starting up");

    let (routing_tx, routing_rx) = mpsc::channel::<RoutingCommand>();
    let (snapshot_tx, snapshot_rx) = async_channel::bounded::<registry::RegistrySnapshot>(64);

    let settings_handle = SettingsHandle::load_or_default();
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

    let ui_config = UiConfig::new(
        routing_tx,
        registry_thread.as_ref().map(|_| Arc::new(snapshot_rx)),
        meter_tap::audio_sample_stream(),
        settings_handle.clone(),
    );

    let exit_code = match ui::run(ui_config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            error!("[ui] failed: {err}");
            ExitCode::FAILURE
        }
    };
    settings_handle.flush();

    if let Some(handle) = registry_thread {
        info!("[main] shutdown requested; waiting for registry monitor to exit...");
        let _ = handle.join();
    }

    exit_code
}
