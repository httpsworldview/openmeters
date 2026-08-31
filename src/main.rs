// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

mod domain;
mod dsp;
mod infra;
mod macros;
mod meter;
mod persistence;
mod ui;
mod util;
mod visuals;

use infra::pipewire::AudioBackend;
use persistence::settings::SettingsHandle;
use std::{cell::RefCell, process::ExitCode, rc::Rc};
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, fmt};
use ui::UiConfig;

fn main() -> ExitCode {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("openmeters=info"));
    if let Err(err) = fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact()
        .try_init()
    {
        eprintln!("[telemetry] failed to initialise tracing subscriber: {err}");
    }
    info!("OpenMeters starting up");

    let settings_handle = SettingsHandle::load_or_default();
    let capture_config = settings_handle.borrow().data.capture_config();
    let (mut backend, capture, audio) = match AudioBackend::start(capture_config) {
        Ok(backend) => backend,
        Err(err) => {
            error!("[capture] failed to start PipeWire backend: {err}");
            return ExitCode::FAILURE;
        }
    };
    let ui_config = UiConfig {
        capture,
        audio: Rc::new(RefCell::new(Some(audio))),
        settings_handle,
    };
    let exit_code = match ui::run(ui_config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            error!("[ui] failed: {err}");
            ExitCode::FAILURE
        }
    };
    SettingsHandle::flush();
    backend.shutdown();
    exit_code
}
