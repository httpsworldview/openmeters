// OpenMeters - an audio analysis and visualization tool
// Copyright (C) 2026  Maika Namuo
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

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

    let registry_thread =
        monitor::init_registry_monitor(routing_rx, snapshot_tx.clone(), routing_config);

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
