// Configuration page for application and visual settings.

use crate::audio::VIRTUAL_SINK_NAME;
use crate::audio::pw_registry::RegistrySnapshot;
use crate::ui::app::visuals::settings::palette::{PaletteEditor, PaletteEvent};
use crate::ui::application_row::ApplicationRow;
use crate::ui::channel_subscription::channel_subscription;
use crate::ui::settings::SettingsHandle;
use crate::ui::settings::{BAR_MAX_HEIGHT, BAR_MIN_HEIGHT, BarAlignment};
use crate::ui::theme;
use crate::ui::visualization::visual_manager::{VisualKind, VisualManagerHandle};
use async_channel::Receiver as AsyncReceiver;
use iced::alignment;
use iced::widget::text::Wrapping;
use iced::widget::{
    Column, Row, Rule, Space, button, container, pick_list, radio, rule, scrollable, slider, text,
};
use iced::{Element, Length, Subscription, Task};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, mpsc};

const GRID_COLUMNS: usize = 2;
const TEXT_SIZE: f32 = 12.0;
const TITLE_SIZE: f32 = 14.0;
const MAX_DEVICE_NAME_LEN: usize = 48;

fn truncate_label(label: &str, max_len: usize) -> (&str, bool) {
    if max_len == 0 {
        return ("", !label.is_empty());
    }

    let mut cutoff = label.len();
    let trunc_at = max_len.saturating_sub(3);

    for (count, (idx, _)) in label.char_indices().enumerate() {
        if count == trunc_at {
            cutoff = idx;
        }
        if count == max_len {
            return (&label[..cutoff], true);
        }
    }

    (label, false)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceOption {
    label: String,
    token: Option<String>,
    selection: DeviceSelection,
}

impl std::fmt::Display for DeviceOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (trimmed, truncated) = truncate_label(&self.label, MAX_DEVICE_NAME_LEN);
        if truncated {
            write!(f, "{trimmed}...")
        } else {
            f.write_str(trimmed)
        }
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
    BgPalette(PaletteEvent),
    DecorationsToggled(bool),
    BarModeToggled(bool),
    BarAlignmentChanged(BarAlignment),
    BarHeightChanged(u16),
}

#[derive(Debug)]
pub struct ConfigPage {
    routing_sender: mpsc::Sender<RoutingCommand>,
    registry_updates: Option<Arc<AsyncReceiver<RegistrySnapshot>>>,
    visual_manager: VisualManagerHandle,
    settings: SettingsHandle,
    bar_supported: bool,
    preferences: HashMap<u32, bool>,
    applications: Vec<ApplicationRow>,
    hardware_sink_label: String,
    hardware_sink_last_known: Option<String>,
    registry_ready: bool,
    applications_expanded: bool,
    capture_mode: CaptureMode,
    device_choices: Vec<DeviceOption>,
    selected_device: DeviceSelection,
    pending_device_name: Option<String>,
    bg_palette: PaletteEditor,
}

impl ConfigPage {
    pub fn new(
        routing_sender: mpsc::Sender<RoutingCommand>,
        registry_updates: Option<Arc<AsyncReceiver<RegistrySnapshot>>>,
        visual_manager: VisualManagerHandle,
        settings: SettingsHandle,
        bar_supported: bool,
    ) -> Self {
        let settings_ref = settings.borrow();
        let current_bg = settings_ref
            .settings()
            .background_color
            .map(Into::into)
            .unwrap_or(theme::BG_BASE);
        let capture_mode = settings_ref.settings().capture_mode;
        let last_device_name = settings_ref.settings().last_device_name.clone();
        drop(settings_ref);

        let mut bg_pal = theme::Palette::new(&theme::background::COLORS, theme::background::LABELS);
        bg_pal.set(&[current_bg]);
        let bg_palette = PaletteEditor::new(bg_pal);

        Self {
            routing_sender,
            registry_updates,
            visual_manager,
            settings,
            bar_supported,
            preferences: HashMap::new(),
            applications: Vec::new(),
            hardware_sink_label: String::from("(detecting hardware sink...)"),
            hardware_sink_last_known: None,
            registry_ready: false,
            applications_expanded: false,
            capture_mode,
            device_choices: Vec::new(),
            selected_device: DeviceSelection::Default,
            pending_device_name: last_device_name,
            bg_palette,
        }
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
                    self.dispatch_capture_state(self.selected_device);
                    self.settings.update(|s| s.set_capture_mode(mode));
                }
            }
            ConfigMessage::CaptureDeviceChanged(selection) => {
                if self.selected_device != selection {
                    self.selected_device = selection;
                    self.dispatch_capture_state(selection);
                    self.settings.update(|s| {
                        s.set_last_device_name(self.device_token_for(selection));
                    });
                }
            }
            ConfigMessage::BgPalette(event) => {
                if self.bg_palette.update(event) {
                    let color = self.bg_palette.colors().first().copied();
                    self.settings.update(|s| s.set_background_color(color));
                }
            }
            ConfigMessage::DecorationsToggled(enabled) => {
                self.settings.update(|s| s.set_decorations(enabled));
            }
            ConfigMessage::BarModeToggled(enabled) => {
                self.settings.update(|s| s.set_bar_enabled(enabled));
            }
            ConfigMessage::BarAlignmentChanged(alignment) => {
                self.settings.update(|s| s.set_bar_alignment(alignment));
            }
            ConfigMessage::BarHeightChanged(height) => {
                self.settings.update(|s| s.set_bar_height(height as u32));
            }
        }

        Task::none()
    }

    pub fn view(&self) -> Element<'_, ConfigMessage> {
        let visuals_snapshot = self.visual_manager.snapshot();

        let capture_section = self.render_capture_section();
        let visuals_section = self.render_visuals_section(&visuals_snapshot);
        let bg_section = self.render_bg_section();

        let content = Column::new()
            .spacing(14)
            .push(capture_section)
            .push(visuals_section)
            .push(bg_section);

        let content = if self.bar_supported {
            content.push(self.render_bar_section())
        } else {
            content
        };

        container(scrollable(content).style(theme::transparent_scrollable))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(8)
            .into()
    }

    fn render_capture_section(&self) -> Column<'_, ConfigMessage> {
        let capture_controls = self.render_capture_mode_controls();
        let primary_section: Element<'_, ConfigMessage> = match self.capture_mode {
            CaptureMode::Applications => self.render_applications_section().into(),
            CaptureMode::Device => self.render_device_section().into(),
        };

        let content = Column::new()
            .spacing(8)
            .push(capture_controls)
            .push(primary_section);

        self.section_with_divider("Audio Capture", content)
    }

    fn render_applications_section(&self) -> Column<'_, ConfigMessage> {
        let status_suffix = if self.applications.is_empty() {
            if self.registry_updates.is_some() {
                if self.registry_ready {
                    " - none detected".to_string()
                } else {
                    " - waiting...".to_string()
                }
            } else {
                " - unavailable".to_string()
            }
        } else {
            format!(" - {} total", self.applications.len())
        };

        let indicator = if self.applications_expanded { "v" } else { ">" };
        let summary_label = format!("{indicator} Applications{status_suffix}");

        let summary_button = button(
            container(
                text(summary_label)
                    .size(TEXT_SIZE)
                    .wrapping(Wrapping::None)
                    .align_x(alignment::Horizontal::Left),
            )
            .width(Length::Fill)
            .clip(true),
        )
        .padding(6)
        .width(Length::Fill)
        .style({
            let expanded = self.applications_expanded;
            move |theme, status| theme::tab_button_style(theme, !expanded, status)
        })
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
                text(msg).size(TEXT_SIZE).into()
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

    fn render_capture_mode_controls(&self) -> Row<'_, ConfigMessage> {
        Row::new()
            .spacing(12)
            .push(
                radio(
                    "Applications",
                    CaptureMode::Applications,
                    Some(self.capture_mode),
                    ConfigMessage::CaptureModeChanged,
                )
                .size(14)
                .text_size(TEXT_SIZE),
            )
            .push(
                radio(
                    "Devices",
                    CaptureMode::Device,
                    Some(self.capture_mode),
                    ConfigMessage::CaptureModeChanged,
                )
                .size(14)
                .text_size(TEXT_SIZE),
            )
    }

    fn render_device_section(&self) -> Column<'_, ConfigMessage> {
        let selected = self
            .device_choices
            .iter()
            .find(|opt| opt.selection == self.selected_device)
            .cloned();
        let mut picker = pick_list(
            self.device_choices.clone(),
            selected,
            |opt: DeviceOption| ConfigMessage::CaptureDeviceChanged(opt.selection),
        )
        .text_size(TEXT_SIZE)
        .width(Length::Fill);
        if self.device_choices.len() <= 1 {
            picker = picker.placeholder("No devices available");
        }

        Column::new()
            .spacing(6)
            .push(container(picker).width(Length::Fill).clip(true))
            .push(
                text("Direct device capture. Application routing disabled.")
                    .size(TEXT_SIZE)
                    .style(theme::weak_text_style),
            )
    }

    fn build_device_choices(&self, snapshot: &RegistrySnapshot) -> Vec<DeviceOption> {
        let mut choices = Vec::new();

        // Use the cached hardware sink label which has fallback to last known value
        let default_label = format!("Default sink - {}", &self.hardware_sink_label);
        choices.push(DeviceOption {
            label: default_label,
            token: None,
            selection: DeviceSelection::Default,
        });

        let mut nodes: Vec<_> = snapshot
            .nodes
            .iter()
            .filter(|node| Self::is_capture_candidate(node))
            .collect();
        nodes.sort_by_key(|node| node.display_name().to_ascii_lowercase());

        for node in nodes {
            let label = node.display_name();
            let token = node
                .name
                .clone()
                .or(node.description.clone())
                .or_else(|| Some(label.clone()));
            choices.push(DeviceOption {
                label,
                token,
                selection: DeviceSelection::Node(node.id),
            });
        }
        choices
    }

    fn is_capture_candidate(node: &crate::audio::pw_registry::NodeInfo) -> bool {
        if node.is_virtual || node.app_name().is_some() {
            return false;
        }

        let contains = |opt: Option<&String>, pattern: &str| {
            opt.is_some_and(|s| s.to_ascii_lowercase().contains(pattern))
        };

        contains(node.media_class.as_ref(), "audio")
            || contains(node.name.as_ref(), "monitor")
            || contains(node.description.as_ref(), "monitor")
    }

    fn render_bg_section(&self) -> Column<'_, ConfigMessage> {
        let decorations_enabled = self.settings.borrow().settings().decorations;

        let decorations_toggle = iced::widget::checkbox(decorations_enabled)
            .label("Enable Window Decorations")
            .size(14)
            .text_size(TEXT_SIZE)
            .on_toggle(ConfigMessage::DecorationsToggled);

        let content = Column::new()
            .spacing(12)
            .push(self.bg_palette.view().map(ConfigMessage::BgPalette))
            .push(decorations_toggle);

        self.section_with_divider("Global", content)
    }

    fn render_bar_section(&self) -> Column<'_, ConfigMessage> {
        let bar_settings = self.settings.borrow().settings().bar.clone();
        let bar_enabled = bar_settings.enabled;

        let bar_toggle = iced::widget::checkbox(bar_enabled)
            .label("Enable Bar Mode")
            .size(14)
            .text_size(TEXT_SIZE)
            .on_toggle(ConfigMessage::BarModeToggled);

        let alignment_controls = Row::new()
            .spacing(12)
            .push(
                radio(
                    "Top",
                    BarAlignment::Top,
                    Some(bar_settings.alignment),
                    ConfigMessage::BarAlignmentChanged,
                )
                .size(14)
                .text_size(TEXT_SIZE),
            )
            .push(
                radio(
                    "Bottom",
                    BarAlignment::Bottom,
                    Some(bar_settings.alignment),
                    ConfigMessage::BarAlignmentChanged,
                )
                .size(14)
                .text_size(TEXT_SIZE),
            );

        let height_range = BAR_MIN_HEIGHT..=BAR_MAX_HEIGHT;
        let height_slider = slider(
            height_range,
            bar_settings.height.clamp(BAR_MIN_HEIGHT, BAR_MAX_HEIGHT),
            |value| ConfigMessage::BarHeightChanged(value as u16),
        )
        .step(1u32)
        .width(Length::Fill);

        let height_label = text(format!("Height: {} px", bar_settings.height))
            .size(TEXT_SIZE)
            .style(theme::weak_text_style);

        let mut content = Column::new().spacing(10).push(bar_toggle);

        if bar_enabled {
            content = content
                .push(alignment_controls)
                .push(height_slider)
                .push(height_label);
        }

        self.section_with_divider("Bar Mode", content)
    }

    fn render_visuals_section(
        &self,
        snapshot: &crate::ui::visualization::visual_manager::VisualSnapshot,
    ) -> Column<'_, ConfigMessage> {
        let total = snapshot.slots.len();
        let enabled = snapshot.slots.iter().filter(|slot| slot.enabled).count();
        let title = format!("Visual Modules ({enabled}/{total})");

        let content: Element<'_, ConfigMessage> = if snapshot.slots.is_empty() {
            text("No visual modules available.").size(TEXT_SIZE).into()
        } else {
            render_toggle_grid(&snapshot.slots, |slot| {
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
            })
            .into()
        };

        self.section_with_divider(title, content)
    }

    fn divider<'a>(&self) -> Rule<'a> {
        rule::horizontal(1).style(|theme: &iced::Theme| rule::Style {
            color: theme::with_alpha(theme.extended_palette().secondary.weak.text, 0.2),
            radius: 0.0.into(),
            fill_mode: rule::FillMode::Percent(100.0),
            snap: true,
        })
    }

    fn section_with_divider<'a>(
        &self,
        title: impl Into<String>,
        content: impl Into<Element<'a, ConfigMessage>>,
    ) -> Column<'a, ConfigMessage> {
        Column::new()
            .spacing(8)
            .push(text(title.into()).size(TITLE_SIZE))
            .push(self.divider())
            .push(content)
    }

    fn update_hardware_sink_label(&mut self, snapshot: &RegistrySnapshot) {
        let summary = snapshot.describe_default_target(snapshot.defaults.audio_sink.as_ref());

        if summary.display != "(none)" || summary.raw != "(none)" {
            self.hardware_sink_last_known = Some(summary.display.clone());
            self.hardware_sink_label = summary.display;
        } else if let Some(previous) = &self.hardware_sink_last_known {
            self.hardware_sink_label = previous.clone();
        } else {
            self.hardware_sink_label = summary.display;
        }
    }

    fn apply_snapshot(&mut self, snapshot: RegistrySnapshot) {
        self.update_hardware_sink_label(&snapshot);
        let choices = self.build_device_choices(&snapshot);
        self.resolve_pending_device(&choices);

        if !choices
            .iter()
            .any(|opt| opt.selection == self.selected_device)
        {
            self.selected_device = DeviceSelection::Default;
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

    fn resolve_pending_device(&mut self, choices: &[DeviceOption]) {
        let Some(token) = self.pending_device_name.as_ref() else {
            return;
        };

        let opt = choices
            .iter()
            .find(|opt| opt.token.as_deref() == Some(token) || opt.label == *token);

        if let Some(opt) = opt {
            self.selected_device = opt.selection;
            self.pending_device_name = None;
            self.settings
                .update(|s| s.set_last_device_name(opt.token.clone()));
        }
    }
}

impl ConfigPage {
    fn dispatch_capture_state(&self, selection: DeviceSelection) {
        let _ = self
            .routing_sender
            .send(RoutingCommand::SetCaptureMode(self.capture_mode));
        let _ = self
            .routing_sender
            .send(RoutingCommand::SelectCaptureDevice(selection));
    }

    fn device_token_for(&self, selection: DeviceSelection) -> Option<String> {
        self.device_choices
            .iter()
            .find(|opt| opt.selection == selection)
            .and_then(|opt| opt.token.clone())
    }
}

fn render_toggle_grid<'a, T, F>(items: &[T], mut project: F) -> Column<'a, ConfigMessage>
where
    F: FnMut(&T) -> (String, bool, ConfigMessage),
{
    let mut grid = Column::new().spacing(6);

    for chunk in items.chunks(GRID_COLUMNS) {
        let mut row = Row::new().spacing(6);

        for item in chunk {
            let (label, enabled, message) = project(item);
            row = row.push(toggle_button(label, enabled, message));
        }

        for _ in chunk.len()..GRID_COLUMNS {
            row = row.push(
                Space::new()
                    .width(Length::FillPortion(1))
                    .height(Length::Shrink),
            );
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
    button(
        container(text(label).size(TEXT_SIZE).wrapping(Wrapping::None))
            .width(Length::Fill)
            .clip(true),
    )
    .padding(8)
    .width(Length::FillPortion(1))
    .style(move |theme: &iced::Theme, status| theme::tab_button_style(theme, enabled, status))
    .on_press(message)
}
