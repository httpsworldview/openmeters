// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

pub mod routing {
    use serde::{Deserialize, Serialize};
    use std::{collections::HashSet, sync::Arc};

    /// Stable key for one application's capture policy.
    #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub struct StreamIdentity(Arc<str>);

    impl StreamIdentity {
        pub fn new(value: impl Into<Arc<str>>) -> Self {
            Self(value.into())
        }

        pub fn as_str(&self) -> &str {
            &self.0
        }
    }

    crate::macros::choice_enum!(all pub enum CaptureMode { #[default] Applications => "Applications", Device => "Devices" });

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
    #[serde(rename_all = "snake_case")]
    pub enum DeviceSelection {
        #[default]
        Default,
        Device(String),
    }

    impl DeviceSelection {
        pub fn from_token(token: Option<&str>) -> Self {
            token
                .filter(|token| !token.is_empty())
                .map_or(Self::Default, |token| Self::Device(token.to_owned()))
        }

        pub fn token(&self) -> Option<&str> {
            match self {
                Self::Device(token) => Some(token),
                Self::Default => None,
            }
        }
    }

    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct CaptureConfig {
        pub mode: CaptureMode,
        pub device: DeviceSelection,
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
