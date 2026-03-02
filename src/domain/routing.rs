use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub enum RoutingCommand {
    SetApplicationEnabled { node_id: u32, enabled: bool },
    SetCaptureMode(CaptureMode),
    SelectCaptureDevice(DeviceSelection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    #[default]
    Applications,
    Device,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DeviceSelection {
    #[default]
    Default,
    Node(u32),
}

#[derive(Debug, Clone)]
pub struct RoutingConfig {
    pub capture_mode: CaptureMode,
    pub preferred_device: Option<String>,
}
