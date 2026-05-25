// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub enum RoutingCommand {
    SetApplicationEnabled { node_id: u32, enabled: bool },
    SetCaptureState(CaptureMode, DeviceSelection),
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
    pub fn from_token(token: Option<String>) -> Self {
        token.map(Self::Device).unwrap_or_default()
    }

    pub fn token(&self) -> Option<&str> {
        match self {
            Self::Device(token) => Some(token),
            Self::Default => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RoutingConfig {
    pub capture_mode: CaptureMode,
    pub preferred_device: DeviceSelection,
}
