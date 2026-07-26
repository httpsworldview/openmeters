// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

#![forbid(unsafe_code)]

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
use std::cell::RefCell;
use std::process::ExitCode;
use std::rc::Rc;
use ui::UiConfig;
use util::telemetry;

use tracing::{error, info};

fn main() -> ExitCode {
    telemetry::init();
    info!("OpenMeters starting up");

    let settings_handle = SettingsHandle::load_or_default();
    let capture_config = settings_handle.borrow().data.capture_config();
    let mut backend = match AudioBackend::start(capture_config) {
        Ok(backend) => backend,
        Err(err) => {
            error!("[capture] failed to start PipeWire backend: {err}");
            return ExitCode::FAILURE;
        }
    };
    let ui_config = UiConfig {
        capture: backend.control(),
        audio: Rc::new(RefCell::new(Some(backend.take_audio()))),
        settings_handle: settings_handle.clone(),
    };
    let exit_code = match ui::run(ui_config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            error!("[ui] failed: {err}");
            ExitCode::FAILURE
        }
    };
    settings_handle.flush();
    backend.shutdown();
    exit_code
}
