//! page for configuring apps and visuals.
//! to be ingested into main app as a tab.

use crate::audio::VIRTUAL_SINK_NAME;
use crate::audio::pw_registry::RegistrySnapshot;
use crate::ui::application_row::ApplicationRow;
use crate::ui::channel_subscription::channel_subscription;
use crate::ui::hardware_sink::HardwareSinkCache;
use crate::ui::settings::SettingsHandle;
use crate::ui::theme;
use crate::ui::visualization::visual_manager::{VisualKind, VisualManagerHandle};
use async_channel::Receiver as AsyncReceiver;
use iced::alignment;
use iced::widget::text::Style as TextStyle;
use iced::widget::{Column, Row, Space, button, container, pick_list, radio, scrollable, text};
use iced::{Element, Length, Subscription, Task};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, mpsc};

const GRID_COLUMNS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceOption(String, DeviceSelection);

impl std::fmt::Display for DeviceOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
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
pub enum RoutingCommand {
    SetApplicationEnabled { node_id: u32, enabled: bool },
    SetCaptureMode(CaptureMode),
    SelectCaptureDevice(DeviceSelection),
}

#[derive(Debug, Clone)]
pub enum ConfigMessage {
    RegistryUpdated(RegistrySnapshot),
    ToggleChanged { node_id: u32, enabled: bool },
    ToggleApplicationsVisibility,
    VisualToggled { kind: VisualKind, enabled: bool },
    CaptureModeChanged(CaptureMode),
    CaptureDeviceChanged(DeviceSelection),
}

#[derive(Debug)]
pub struct ConfigPage {
    routing_sender: mpsc::Sender<RoutingCommand>,
    registry_updates: Option<Arc<AsyncReceiver<RegistrySnapshot>>>,
    visual_manager: VisualManagerHandle,
    settings: SettingsHandle,
    preferences: HashMap<u32, bool>,
    applications: Vec<ApplicationRow>,
    hardware_sink: HardwareSinkCache,
    registry_ready: bool,
    applications_expanded: bool,
    capture_mode: CaptureMode,
    device_choices: Vec<DeviceOption>,
    selected_device: DeviceSelection,
}

impl ConfigPage {
    pub fn new(
        routing_sender: mpsc::Sender<RoutingCommand>,
        registry_updates: Option<Arc<AsyncReceiver<RegistrySnapshot>>>,
        visual_manager: VisualManagerHandle,
        settings: SettingsHandle,
    ) -> Self {
        let ret = Self {
            routing_sender,
            registry_updates,
            visual_manager,
            settings,
            preferences: HashMap::new(),
            applications: Vec::new(),
            hardware_sink: HardwareSinkCache::new(),
            registry_ready: false,
            applications_expanded: false,
            capture_mode: CaptureMode::Applications,
            device_choices: Vec::new(),
            selected_device: DeviceSelection::Default,
        };
        ret.dispatch_capture_state();
        ret
    }

    pub fn subscription(&self) -> Subscription<ConfigMessage> {
        self.registry_updates
            .as_ref()
            .map(|receiver| {
                channel_subscription(Arc::clone(receiver)).map(ConfigMessage::RegistryUpdated)
            })
            .unwrap_or_else(Subscription::none)
    }

    pub fn update(&mut self, message: ConfigMessage) -> Task<ConfigMessage> {
        match message {
            ConfigMessage::RegistryUpdated(snapshot) => {
                self.registry_ready = true;
                self.apply_snapshot(snapshot);
            }
            ConfigMessage::ToggleChanged { node_id, enabled } => {
                self.preferences.insert(node_id, enabled);

                if let Some(entry) = self
                    .applications
                    .iter_mut()
                    .find(|entry| entry.node_id == node_id)
                {
                    entry.enabled = enabled;
                }

                if let Err(err) = self
                    .routing_sender
                    .send(RoutingCommand::SetApplicationEnabled { node_id, enabled })
                {
                    tracing::error!("[ui] failed to send routing command: {err}");
                }
            }
            ConfigMessage::ToggleApplicationsVisibility => {
                self.applications_expanded = !self.applications_expanded;
            }
            ConfigMessage::VisualToggled { kind, enabled } => {
                self.visual_manager
                    .borrow_mut()
                    .set_enabled_by_kind(kind, enabled);
                self.settings
                    .update(|settings| settings.set_visual_enabled(kind, enabled));
            }
            ConfigMessage::CaptureModeChanged(mode) => {
                if self.capture_mode != mode {
                    self.capture_mode = mode;
                    self.dispatch_capture_state();
                }
            }
            ConfigMessage::CaptureDeviceChanged(selection) => {
                if self.selected_device != selection {
                    self.selected_device = selection;
                    self.dispatch_capture_state();
                }
            }
        }

        Task::none()
    }

    pub fn view(&self) -> Element<'_, ConfigMessage> {
        let visuals_snapshot = self.visual_manager.snapshot();
        let status_label = self.capture_status_label();

        let capture_controls = self.render_capture_mode_controls();
        let primary_section: Element<'_, ConfigMessage> = match self.capture_mode {
            CaptureMode::Applications => self.render_applications_section().into(),
            CaptureMode::Device => self.render_device_section().into(),
        };

        let visuals_section = self.render_visuals_section(&visuals_snapshot);

        let content = Column::new()
            .spacing(16)
            .push(text(status_label).size(14))
            .push(capture_controls)
            .push(primary_section)
            .push(visuals_section);

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn render_applications_section(&self) -> Column<'_, ConfigMessage> {
        let status_suffix = if self.applications.is_empty() {
            if self.registry_updates.is_some() {
                if self.registry_ready {
                    " - none detected"
                } else {
                    " - waiting..."
                }
            } else {
                " - unavailable"
            }
        } else {
            &format!(" - {} total", self.applications.len())
        };

        let indicator = if self.applications_expanded {
            "▾"
        } else {
            "▸"
        };
        let summary_label = format!("{indicator} Applications{status_suffix}");

        let summary_button = button(
            text(summary_label)
                .width(Length::Fill)
                .align_x(alignment::Horizontal::Left),
        )
        .padding(8)
        .width(Length::Fill)
        .style(header_button_style)
        .on_press(ConfigMessage::ToggleApplicationsVisibility);

        let mut section = Column::new().spacing(8).push(summary_button);

        if self.applications_expanded {
            let content = if self.applications.is_empty() {
                let msg = if self.registry_updates.is_none() {
                    "Registry unavailable; routing controls disabled."
                } else if self.registry_ready {
                    "No audio applications detected. Launch something to see it here."
                } else {
                    "Waiting for PipeWire registry snapshots..."
                };
                text(msg).into()
            } else {
                self.render_applications_grid()
            };
            section = section.push(scrollable(content).height(Length::Shrink));
        }

        section
    }

    fn render_applications_grid(&self) -> Element<'_, ConfigMessage> {
        render_toggle_grid(&self.applications, |entry| {
            (
                format!(
                    "{} ({})",
                    entry.display_label(),
                    if entry.enabled { "enabled" } else { "disabled" }
                ),
                entry.enabled,
                ConfigMessage::ToggleChanged {
                    node_id: entry.node_id,
                    enabled: !entry.enabled,
                },
            )
        })
        .into()
    }

    fn capture_status_label(&self) -> String {
        match self.capture_mode {
            CaptureMode::Applications => {
                format!("Default hardware sink: {}", self.hardware_sink.label())
            }
            CaptureMode::Device => format!("Capturing from: {}", self.selected_device_label()),
        }
    }

    fn render_capture_mode_controls(&self) -> Row<'_, ConfigMessage> {
        Row::new()
            .spacing(12)
            .push(radio(
                "Applications",
                CaptureMode::Applications,
                Some(self.capture_mode),
                ConfigMessage::CaptureModeChanged,
            ))
            .push(radio(
                "Devices",
                CaptureMode::Device,
                Some(self.capture_mode),
                ConfigMessage::CaptureModeChanged,
            ))
    }

    fn render_device_section(&self) -> Column<'_, ConfigMessage> {
        let selected = self
            .device_choices
            .iter()
            .find(|opt| opt.1 == self.selected_device)
            .cloned();
        let mut picker = pick_list(
            self.device_choices.clone(),
            selected,
            |opt: DeviceOption| ConfigMessage::CaptureDeviceChanged(opt.1),
        );
        if self.device_choices.len() <= 1 {
            picker = picker.placeholder("No devices available");
        }

        Column::new()
            .spacing(8)
            .push(text("Device capture").size(14))
            .push(picker)
            .push(
                text("Direct device capture. Application routing disabled.")
                    .size(12)
                    .style(|_| TextStyle {
                        color: Some(theme::text_secondary()),
                    }),
            )
    }

    fn selected_device_label(&self) -> String {
        self.device_choices
            .iter()
            .find(|opt| opt.1 == self.selected_device)
            .map(|opt| opt.0.clone())
            .unwrap_or_else(|| match self.selected_device {
                DeviceSelection::Default => format!("Default ({})", self.hardware_sink.label()),
                DeviceSelection::Node(id) => format!("Node #{id}"),
            })
    }

    fn build_device_choices(&self, snapshot: &RegistrySnapshot) -> Vec<DeviceOption> {
        let mut choices = Vec::new();

        // Use the cached hardware sink label which has fallback to last known value
        let default_label = format!("Default sink - {}", self.hardware_sink.label());
        choices.push(DeviceOption(default_label, DeviceSelection::Default));

        let mut nodes: Vec<_> = snapshot
            .nodes
            .iter()
            .filter(|node| Self::is_capture_candidate(node))
            .map(|node| (node.display_name(), node.id))
            .collect();
        nodes.sort_by(|a, b| a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase()));

        for (label, id) in nodes {
            choices.push(DeviceOption(label, DeviceSelection::Node(id)));
        }
        choices
    }

    fn is_capture_candidate(node: &crate::audio::pw_registry::NodeInfo) -> bool {
        if node.is_virtual || node.app_name().is_some() {
            return false;
        }
        let has_audio =
            |s: Option<&String>| s.is_some_and(|t| t.to_ascii_lowercase().contains("audio"));
        let has_monitor =
            |s: Option<&String>| s.is_some_and(|t| t.to_ascii_lowercase().contains("monitor"));

        has_audio(node.media_class.as_ref())
            || has_monitor(node.name.as_ref())
            || has_monitor(node.description.as_ref())
    }

    fn render_visuals_section(
        &self,
        snapshot: &crate::ui::visualization::visual_manager::VisualSnapshot,
    ) -> Column<'_, ConfigMessage> {
        let total = snapshot.slots.len();
        let enabled = snapshot.slots.iter().filter(|slot| slot.enabled).count();
        let header = text(format!("Visual modules - {enabled}/{total} enabled")).size(14);

        let section = Column::new().spacing(8).push(header);

        if snapshot.slots.is_empty() {
            return section.push(text("No visual modules available."));
        }

        let grid = render_toggle_grid(&snapshot.slots, |slot| {
            (
                format!(
                    "{} ({})",
                    slot.metadata.display_name,
                    if slot.enabled { "enabled" } else { "disabled" }
                ),
                slot.enabled,
                ConfigMessage::VisualToggled {
                    kind: slot.kind,
                    enabled: !slot.enabled,
                },
            )
        });

        section.push(grid)
    }

    fn apply_snapshot(&mut self, snapshot: RegistrySnapshot) {
        self.hardware_sink.update(&snapshot);
        let choices = self.build_device_choices(&snapshot);
        if !choices.iter().any(|opt| opt.1 == self.selected_device) {
            self.selected_device = DeviceSelection::Default;
            self.dispatch_capture_state();
        }
        self.device_choices = choices;

        let mut entries = Vec::new();
        let mut seen = HashSet::new();

        if let Some(sink) = snapshot.find_node_by_label(VIRTUAL_SINK_NAME) {
            for node in snapshot.route_candidates(sink) {
                let enabled = self.preferences.get(&node.id).copied().unwrap_or(true);
                entries.push(ApplicationRow::from_node(node, enabled));
                seen.insert(node.id);
            }
        }

        self.preferences.retain(|node_id, _| seen.contains(node_id));

        entries.sort_by_key(|a| a.sort_key());
        self.applications = entries;
    }
}

impl ConfigPage {
    fn dispatch_capture_state(&self) {
        let _ = self
            .routing_sender
            .send(RoutingCommand::SetCaptureMode(self.capture_mode));
        let _ = self
            .routing_sender
            .send(RoutingCommand::SelectCaptureDevice(self.selected_device));
    }
}

fn render_toggle_grid<'a, T, F>(items: &[T], mut project: F) -> Column<'a, ConfigMessage>
where
    F: FnMut(&T) -> (String, bool, ConfigMessage),
{
    let mut grid = Column::new().spacing(12);

    for chunk in items.chunks(GRID_COLUMNS) {
        let mut row = Row::new().spacing(12);

        for item in chunk {
            let (label, enabled, message) = project(item);
            row = row.push(toggle_button(label, enabled, message));
        }

        for _ in chunk.len()..GRID_COLUMNS {
            row = row.push(Space::new(Length::FillPortion(1), Length::Shrink));
        }

        grid = grid.push(row);
    }

    grid
}

fn toggle_button<'a>(
    label: String,
    enabled: bool,
    message: ConfigMessage,
) -> iced::widget::Button<'a, ConfigMessage> {
    button(text(label).width(Length::Fill))
        .padding(12)
        .width(Length::FillPortion(1))
        .style(move |_theme, status| {
            let base_background = if enabled {
                theme::surface_color()
            } else {
                theme::elevated_color()
            };
            let text_color = if enabled {
                theme::text_color()
            } else {
                theme::text_secondary()
            };
            let mut style = iced::widget::button::Style {
                background: Some(iced::Background::Color(base_background)),
                text_color,
                border: theme::sharp_border(),
                ..Default::default()
            };

            match status {
                iced::widget::button::Status::Hovered => {
                    style.background = Some(iced::Background::Color(theme::hover_color()));
                }
                iced::widget::button::Status::Pressed => {
                    style.border = theme::focus_border();
                }
                _ => {}
            }

            style
        })
        .on_press(message)
}

fn header_button_style(
    _theme: &iced::Theme,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    theme::surface_button_style(status)
}
