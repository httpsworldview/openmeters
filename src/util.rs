// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo
pub mod audio;
pub mod color;

pub fn finite_positive(value: f32) -> Option<f32> {
    (value.is_finite() && value > 0.0).then_some(value)
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub mod telemetry {
    use std::sync::OnceLock;
    use tracing::Level;
    use tracing_subscriber::{EnvFilter, fmt};

    static TELEMETRY_INIT: OnceLock<()> = OnceLock::new();

    pub fn init() {
        TELEMETRY_INIT.get_or_init(|| {
            let env_filter = EnvFilter::try_from_default_env()
                .or_else(|_| EnvFilter::try_new("openmeters=info"))
                .unwrap_or_else(|_| EnvFilter::default().add_directive(Level::INFO.into()));

            if let Err(err) = fmt()
                .with_env_filter(env_filter)
                .with_target(false)
                .compact()
                .try_init()
            {
                eprintln!("[telemetry] failed to initialise tracing subscriber: {err}");
            }
        });
    }
}
