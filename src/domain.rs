// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

pub mod routing {
    use serde::{Deserialize, Serialize};
    use std::{collections::HashSet, sync::Arc};

    /// Stable key for one application's capture policy.
    #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
    #[serde(transparent)]
    pub struct StreamIdentity(pub(crate) Arc<str>);

    crate::macros::choice_enum!(all pub enum CaptureMode { #[default] Applications => "Applications", Device => "Devices" });

    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct CaptureConfig {
        pub mode: CaptureMode,
        pub device: Option<Arc<str>>,
        pub disabled_streams: HashSet<StreamIdentity>,
    }
}

pub mod visuals {
    crate::macros::choice_enum!(no_default
        #[derive(PartialOrd, Ord)]
        pub enum VisualKind {
            Loudness => "Loudness",
            Oscilloscope => "Oscilloscope",
            Waveform => "Waveform",
            Spectrogram => "Spectrogram",
            Spectrum => "Spectrum analyzer",
            Stereometer => "Stereometer",
        }
    );
}
