// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::domain::routing::{CaptureMode, DeviceSelection, RoutingCommand};
use crate::infra::pipewire::registry::RegistrySnapshot;
use crate::persistence::settings::{
    BAR_MAX_HEIGHT, BAR_MIN_HEIGHT, BUILTIN_THEME, BarAlignment, SettingsHandle, ThemeChoice,
    ThemeFile, ThemeOrigin, canonical_theme_name,
};
use crate::ui::subscription::channel_subscription;
use crate::ui::theme;
use crate::ui::widgets::palette_editor::{PaletteEditor, PaletteEvent};
use crate::ui::widgets::scroll_glow::ScrollGlow;
use crate::ui::widgets::{SliderRange, action_button, card, pick, selectable_button, toggle};
use crate::visuals::registry::{VisualKind, VisualManagerHandle, VisualSlotSnapshot};
use async_channel::Receiver as AsyncReceiver;
use iced::widget::{Column, Row, column, container, pick_list, row, text, text_input};
use iced::{Element, Length, Subscription};
use iced_layershell::actions::OutputSnapshot;
use std::collections::HashSet;
use std::sync::{Arc, mpsc};

const GRID_COLUMNS: usize = 2;
const MAX_DEVICE_NAME_LEN: usize = 48;

fn truncate_label(label: &str, max_chars: usize) -> (&str, bool) {
    if label.chars().count() <= max_chars {
        return (label, false);
    }
    let end = label
        .char_indices()
        .nth(max_chars.saturating_sub(3))
        .map_or(label.len(), |(i, _)| i);
    (&label[..end], true)
}

#[derive(Clone, PartialEq, Eq)]
struct DeviceOption {
    label: String,
    selection: DeviceSelection,
}

impl std::fmt::Display for DeviceOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (trimmed, truncated) = truncate_label(&self.label, MAX_DEVICE_NAME_LEN);
        write!(f, "{trimmed}{}", if truncated { "..." } else { "" })
    }
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
    BarHeightChanged(u32),
    BarMonitorChanged(String),
    ThemeChanged(String),
    SaveTheme(String),
    ThemeNameInput(String),
    Scrolled(ScrollGlow),
}

struct ApplicationRow {
    node_id: u32,
    label: String,
}

impl ApplicationRow {
    fn from_node(node: &crate::infra::pipewire::registry::NodeInfo) -> Self {
        let primary = node
            .app_name()
            .map(str::to_owned)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| node.capture_device_token());
        let node_label = node.capture_device_token();
        let label = if primary.eq_ignore_ascii_case(&node_label) {
            primary
        } else {
            format!("{primary} ({node_label})")
        };
        Self {
            node_id: node.id,
            label,
        }
    }
}

pub struct ConfigPage {
    routing_sender: mpsc::Sender<RoutingCommand>,
    registry_updates: Option<Arc<AsyncReceiver<RegistrySnapshot>>>,
    visual_manager: VisualManagerHandle,
    settings: SettingsHandle,
    bar_supported: bool,
    bar_monitors: Vec<String>,
    disabled_applications: HashSet<u32>,
    applications: Vec<ApplicationRow>,
    hardware_sink_label: String,
    hardware_sink_last_known: Option<String>,
    registry_ready: bool,
    applications_expanded: bool,
    device_choices: Vec<DeviceOption>,
    selected_device: DeviceSelection,
    bg_palette: PaletteEditor,
    scroll: ScrollGlow,
    theme_choices: Vec<ThemeChoice>,
    save_theme_name: String,
}

impl ConfigPage {
    pub fn new(
        routing_sender: mpsc::Sender<RoutingCommand>,
        registry_updates: Option<Arc<AsyncReceiver<RegistrySnapshot>>>,
        visual_manager: VisualManagerHandle,
        settings: SettingsHandle,
        bar_supported: bool,
    ) -> Self {
        use theme::background as bg;

        let (current_bg, last_device_name, theme_choices) = {
            let guard = settings.borrow();
            let data = &guard.data;
            (
                data.background_color.map_or(theme::BG_BASE, Into::into),
                data.last_device_name.clone(),
                guard.theme_store().list(),
            )
        };
        let mut bg_pal = theme::Palette::new(&bg::COLORS, &bg::DEFAULT_POSITIONS, bg::LABELS);
        bg_pal.set_colors(&[current_bg]);
        let bg_palette = PaletteEditor::new(bg_pal);

        Self {
            routing_sender,
            registry_updates,
            visual_manager,
            settings,
            bar_supported,
            bar_monitors: Vec::new(),
            disabled_applications: HashSet::new(),
            applications: Vec::new(),
            hardware_sink_label: String::from("(detecting hardware sink...)"),
            hardware_sink_last_known: None,
            registry_ready: false,
            applications_expanded: false,
            device_choices: Vec::new(),
            selected_device: DeviceSelection::from_token(last_device_name),
            bg_palette,
            scroll: ScrollGlow::default(),
            theme_choices,
            save_theme_name: String::new(),
        }
    }

    pub fn subscription(&self) -> Subscription<ConfigMessage> {
        self.registry_updates
            .as_ref()
            .map_or_else(Subscription::none, |receiver| {
                channel_subscription(Arc::clone(receiver)).map(ConfigMessage::RegistryUpdated)
            })
    }

    pub fn update(&mut self, message: ConfigMessage) {
        match message {
            ConfigMessage::RegistryUpdated(snapshot) => {
                self.registry_ready = true;
                self.apply_snapshot(snapshot);
            }
            ConfigMessage::ToggleChanged { node_id, enabled } => {
                if enabled {
                    self.disabled_applications.remove(&node_id);
                } else {
                    self.disabled_applications.insert(node_id);
                }
                self.send_routing(RoutingCommand::SetApplicationEnabled { node_id, enabled });
            }
            ConfigMessage::ToggleApplicationsVisibility => {
                self.applications_expanded = !self.applications_expanded;
            }
            ConfigMessage::VisualToggled { kind, enabled } => {
                self.visual_manager.borrow_mut().set_enabled(kind, enabled);
                self.settings.update(|s| {
                    s.data.visuals.modules.entry(kind).or_default().enabled = Some(enabled);
                });
            }
            ConfigMessage::CaptureModeChanged(mode) => {
                if self.settings.borrow().data.capture_mode != mode {
                    self.settings.update(|s| s.data.capture_mode = mode);
                    self.dispatch_capture_state();
                }
            }
            ConfigMessage::CaptureDeviceChanged(selection) => {
                if self.selected_device != selection {
                    let token = selection.token().map(str::to_owned);
                    self.selected_device = selection;
                    self.dispatch_capture_state();
                    self.settings.update(|s| s.data.last_device_name = token);
                }
            }
            ConfigMessage::BgPalette(event) => {
                if self.bg_palette.update(event) {
                    let color = self.bg_palette.colors().first().copied();
                    self.settings.update(|s| {
                        s.data.background_color = color.map(Into::into);
                        s.update_active_theme(|theme| theme.background = color.map(Into::into));
                    });
                    self.refresh_theme_choices_if_needed();
                }
            }
            ConfigMessage::DecorationsToggled(v) => {
                self.settings.update(|s| s.data.decorations = v);
            }
            ConfigMessage::BarModeToggled(v) => self.settings.update(|s| s.data.bar.enabled = v),
            ConfigMessage::BarAlignmentChanged(v) => {
                self.settings.update(|s| s.data.bar.alignment = v);
            }
            ConfigMessage::BarHeightChanged(v) => self.settings.update(|s| s.data.bar.height = v),
            ConfigMessage::BarMonitorChanged(v) => {
                self.settings.update(|s| s.data.bar.monitor = Some(v));
            }
            ConfigMessage::ThemeChanged(name) => self.apply_theme(&name),
            ConfigMessage::SaveTheme(name) => {
                let active = self.settings.borrow().active_theme().to_owned();
                if let Some(saved_name) = self.save_current_as_theme(&name)
                    && active != saved_name
                {
                    self.settings.update(|s| s.data.theme = Some(saved_name));
                }
                self.save_theme_name.clear();
            }
            ConfigMessage::ThemeNameInput(val) => self.save_theme_name = val,
            ConfigMessage::Scrolled(g) => self.scroll = g,
        }
    }

    pub fn view(&self) -> Element<'_, ConfigMessage> {
        let snapshot = self.visual_manager.borrow().snapshot();
        let mut content = column![
            self.render_capture_card(),
            self.render_visuals_card(&snapshot),
            self.render_theme_card(),
            self.render_global_card(),
        ]
        .spacing(theme::SECTION_GAP);
        if self.bar_supported {
            content = content.push(self.render_bar_card());
        }
        self.scroll.vertical(content, ConfigMessage::Scrolled)
    }

    fn render_capture_card(&self) -> container::Container<'_, ConfigMessage> {
        let mode = self.settings.borrow().data.capture_mode;
        let content = form!(
            pick("Mode", CaptureMode::ALL, mode, ConfigMessage::CaptureModeChanged);
            match mode {
                CaptureMode::Applications => self.render_applications_section(),
                CaptureMode::Device => self.render_device_section(),
            };
        );
        card("Audio Capture", content)
    }

    fn render_applications_section(&self) -> Column<'_, ConfigMessage> {
        let status_suffix: String = match (
            self.applications.len(),
            self.registry_updates.is_some(),
            self.registry_ready,
        ) {
            (0, false, _) => " - unavailable".into(),
            (0, true, false) => " - waiting...".into(),
            (0, true, true) => " - none detected".into(),
            (n, _, _) => format!(" - {n} total"),
        };

        let indicator = if self.applications_expanded { "v" } else { ">" };
        let summary_button = selectable_button(
            format!("{indicator} Applications{status_suffix}"),
            !self.applications_expanded,
            ConfigMessage::ToggleApplicationsVisibility,
        );

        let mut section = Column::new()
            .spacing(theme::CONTROL_GAP)
            .push(summary_button);
        if self.applications_expanded {
            let content: Element<'_, _> = if self.applications.is_empty() {
                let message = match (self.registry_updates.is_some(), self.registry_ready) {
                    (false, _) => "Registry unavailable; routing controls disabled.",
                    (_, true) => "No audio applications detected. Launch something to see it here.",
                    _ => "Waiting for PipeWire registry snapshots...",
                };
                text(message).size(theme::BODY_TEXT_SIZE).into()
            } else {
                render_toggle_grid(&self.applications, |entry| {
                    let enabled = !self.disabled_applications.contains(&entry.node_id);
                    (
                        entry.label.as_str(),
                        enabled,
                        ConfigMessage::ToggleChanged {
                            node_id: entry.node_id,
                            enabled: !enabled,
                        },
                    )
                })
                .into()
            };
            section = section.push(content);
        }
        section
    }

    fn render_device_section(&self) -> Column<'_, ConfigMessage> {
        let selected = self
            .device_choices
            .iter()
            .find(|opt| opt.selection == self.selected_device);
        let mut picker = pick_list(self.device_choices.as_slice(), selected, |opt| {
            ConfigMessage::CaptureDeviceChanged(opt.selection)
        })
        .text_size(theme::BODY_TEXT_SIZE)
        .width(Length::Fill);
        if self.device_choices.len() <= 1 {
            picker = picker.placeholder("No devices available");
        }

        column![
            container(picker).width(Length::Fill).clip(true),
            text("Direct device capture. Application routing disabled.")
                .size(theme::BODY_TEXT_SIZE)
                .style(theme::weak_text_style)
        ]
        .spacing(6)
    }

    fn build_device_choices(&self, snapshot: &RegistrySnapshot) -> Vec<DeviceOption> {
        let mut choices = vec![DeviceOption {
            label: format!("Default sink - {}", self.hardware_sink_label),
            selection: DeviceSelection::Default,
        }];
        let mut devices: Vec<_> = snapshot
            .nodes
            .iter()
            .filter(|node| node.is_capture_device_candidate())
            .map(|node| {
                let token = node.capture_device_token();
                DeviceOption {
                    label: token.clone(),
                    selection: DeviceSelection::Device(token),
                }
            })
            .collect();
        devices.sort_by_cached_key(|opt| opt.label.to_ascii_lowercase());
        choices.extend(devices);
        choices
    }

    fn render_global_card(&self) -> container::Container<'_, ConfigMessage> {
        use ConfigMessage::{BgPalette, DecorationsToggled};
        let decorations = self.settings.borrow().data.decorations;
        let content = column![
            self.bg_palette.view().map(BgPalette),
            toggle("Window decorations", decorations, DecorationsToggled),
        ]
        .spacing(theme::SECTION_GAP);
        card("Global", content)
    }

    fn render_theme_card(&self) -> container::Container<'_, ConfigMessage> {
        let active = self.settings.borrow().active_theme().to_owned();
        let selected = self.theme_choices.iter().find(|c| c.name == active);
        let is_builtin = selected.is_some_and(|c| c.origin == ThemeOrigin::BuiltIn);

        let picker = pick_list(self.theme_choices.as_slice(), selected, |choice| {
            ConfigMessage::ThemeChanged(choice.name)
        })
        .text_size(theme::BODY_TEXT_SIZE)
        .width(Length::Fill);

        let save_btn = action_button(
            "Save",
            (!is_builtin).then(|| ConfigMessage::SaveTheme(active.clone())),
        )
        .padding([4, 8]);

        let save_as_input = text_input("New theme name...", &self.save_theme_name)
            .on_input(ConfigMessage::ThemeNameInput)
            .size(theme::BODY_TEXT_SIZE)
            .width(Length::Fill);
        let trimmed = self.save_theme_name.trim();
        let save_as_btn = action_button(
            "Save as",
            (!trimmed.is_empty() && trimmed != BUILTIN_THEME)
                .then(|| ConfigMessage::SaveTheme(trimmed.to_owned())),
        )
        .padding([4, 8]);

        let content = form!(
            row![picker, save_btn].spacing(theme::CONTROL_GAP);
            row![save_as_input, save_as_btn].spacing(theme::CONTROL_GAP);
        );
        card("Theme", content)
    }

    fn apply_theme(&mut self, name: &str) {
        let Some(theme_file) = self.settings.borrow().theme_store().load(name) else {
            return;
        };
        self.visual_manager.borrow_mut().apply_theme(&theme_file);
        let bg = theme_file.background.map_or(theme::BG_BASE, Into::into);
        self.bg_palette.set_colors(&[bg]);
        let theme_val = (name != BUILTIN_THEME).then(|| name.to_owned());
        self.settings.update(|s| {
            s.data.background_color = Some(bg.into());
            s.data.theme = theme_val;
        });
    }

    fn save_current_as_theme(&mut self, name: &str) -> Option<String> {
        let name = canonical_theme_name(name);
        if name.is_empty() || name == BUILTIN_THEME {
            tracing::warn!("[theme] invalid theme name {name:?}");
            return None;
        }

        let theme_file = self.export_theme(&name);
        let saved = self
            .settings
            .borrow()
            .theme_store()
            .save(&name, &theme_file);
        if let Err(e) = saved {
            tracing::warn!("[theme] failed to save theme {name:?}: {e}");
            return None;
        }
        self.refresh_theme_choices();
        Some(name)
    }

    pub(in crate::ui) fn refresh_theme_choices_if_needed(&mut self) {
        let active = self.settings.borrow().active_theme().to_owned();
        if !self.theme_choices.iter().any(|c| c.name == active) {
            self.refresh_theme_choices();
        }
    }

    fn refresh_theme_choices(&mut self) {
        self.theme_choices = self.settings.borrow().theme_store().list();
    }

    fn export_theme(&self, name: &str) -> ThemeFile {
        let bg = self.settings.borrow().data.background_color;
        ThemeFile {
            name: Some(name.to_owned()),
            author: None,
            background: bg,
            palettes: self.visual_manager.borrow().theme_palettes().collect(),
        }
    }

    pub(in crate::ui) fn sync_bar_outputs(&mut self, snapshot: OutputSnapshot) {
        self.bar_monitors = snapshot.outputs;
        if let Some(monitor) = snapshot.current
            && self.settings.borrow().data.bar.monitor.as_ref() != Some(&monitor)
        {
            self.settings.update(|s| s.data.bar.monitor = Some(monitor));
        }
    }

    fn render_bar_card(&self) -> container::Container<'_, ConfigMessage> {
        use ConfigMessage::{
            BarAlignmentChanged as Alignment, BarHeightChanged, BarModeToggled, BarMonitorChanged,
        };
        let bar = self.settings.borrow().data.bar.clone();
        let mut content = column![toggle("Bar mode", bar.enabled, BarModeToggled)].spacing(10);
        if bar.enabled {
            let height = bar.height.clamp(BAR_MIN_HEIGHT, BAR_MAX_HEIGHT);
            let height_range = SliderRange::new(BAR_MIN_HEIGHT as f32, BAR_MAX_HEIGHT as f32, 1.0);
            let monitor = row![
                text("Monitor").size(theme::BODY_TEXT_SIZE),
                pick_list(
                    self.bar_monitors.as_slice(),
                    bar.monitor.clone(),
                    BarMonitorChanged,
                )
                .placeholder("Detecting monitor...")
                .text_size(theme::BODY_TEXT_SIZE)
                .width(Length::Fill),
            ]
            .spacing(theme::CONTROL_GAP)
            .width(Length::Fill);
            let alignment = pick("Alignment", BarAlignment::ALL, bar.alignment, Alignment);
            let height_slider = slider!(
                "Height",
                height as f32,
                height_range,
                |value| BarHeightChanged(value.round() as u32),
                format!("{height} px")
            );
            content = content.push(monitor).push(alignment).push(height_slider);
        }
        card("Bar Mode", content)
    }

    fn render_visuals_card(
        &self,
        snapshot: &[VisualSlotSnapshot],
    ) -> container::Container<'_, ConfigMessage> {
        let enabled = snapshot.iter().filter(|slot| slot.enabled).count();
        card(
            format!("Visual Modules ({enabled}/{})", snapshot.len()),
            render_toggle_grid(snapshot, |slot| {
                (
                    slot.kind.label(),
                    slot.enabled,
                    ConfigMessage::VisualToggled {
                        kind: slot.kind,
                        enabled: !slot.enabled,
                    },
                )
            }),
        )
    }

    fn update_hardware_sink_label(&mut self, snapshot: &RegistrySnapshot) {
        let summary = snapshot.describe_default_target(snapshot.defaults.audio_sink.as_ref());
        let known = summary.display != "(none)" || summary.raw != "(none)";
        if known {
            self.hardware_sink_last_known = Some(summary.display.clone());
            self.hardware_sink_label = summary.display;
        } else {
            self.hardware_sink_label = self
                .hardware_sink_last_known
                .clone()
                .unwrap_or(summary.display);
        }
    }

    fn apply_snapshot(&mut self, snapshot: RegistrySnapshot) {
        self.update_hardware_sink_label(&snapshot);
        let mut choices = self.build_device_choices(&snapshot);
        if sync_selected_device_with_choices(&mut self.selected_device, &mut choices, &snapshot) {
            let token = self.selected_device.token().map(str::to_owned);
            self.settings.update(|s| s.data.last_device_name = token);
            self.dispatch_capture_state();
        }
        self.device_choices = choices;

        let mut seen = HashSet::new();
        let mut entries: Vec<_> = snapshot
            .virtual_sink()
            .into_iter()
            .flat_map(|sink| snapshot.route_candidates(sink))
            .map(|node| {
                seen.insert(node.id);
                ApplicationRow::from_node(node)
            })
            .collect();
        self.disabled_applications.retain(|id| seen.contains(id));
        entries.sort_by_cached_key(|entry| (entry.label.to_ascii_lowercase(), entry.node_id));
        self.applications = entries;
    }

    fn dispatch_capture_state(&self) {
        self.send_routing(RoutingCommand::SetCaptureState(
            self.settings.borrow().data.capture_mode,
            self.selected_device.clone(),
        ));
    }

    fn send_routing(&self, command: RoutingCommand) {
        if let Err(err) = self.routing_sender.send(command) {
            tracing::error!("[ui] failed to send routing command: {err}");
        }
    }
}

fn sync_selected_device_with_choices(
    selected: &mut DeviceSelection,
    choices: &mut Vec<DeviceOption>,
    snapshot: &RegistrySnapshot,
) -> bool {
    let DeviceSelection::Device(token) = selected else {
        return false;
    };
    let mut changed = false;
    if let Some(node) = snapshot.find_capture_device_by_token(token) {
        let canonical = node.capture_device_token();
        changed = token.as_str() != canonical;
        *token = canonical;
    }
    if !choices
        .iter()
        .any(|opt| opt.selection.token() == Some(token.as_str()))
    {
        choices.push(DeviceOption {
            label: format!("{token} (unavailable)"),
            selection: DeviceSelection::Device(token.clone()),
        });
    }
    changed
}

fn render_toggle_grid<'a, T, F>(items: &[T], mut project: F) -> Column<'a, ConfigMessage>
where
    for<'b> F: FnMut(&'b T) -> (&'b str, bool, ConfigMessage),
{
    let mut grid = Column::new().spacing(6);
    for chunk in items.chunks(GRID_COLUMNS) {
        let mut row = Row::new().spacing(6);
        for item in chunk {
            let (name, enabled, message) = project(item);
            let label = format!("{name} ({})", if enabled { "enabled" } else { "disabled" });
            row =
                row.push(selectable_button(label, enabled, message).width(Length::FillPortion(1)));
        }
        grid = grid.push(row);
    }
    grid
}
