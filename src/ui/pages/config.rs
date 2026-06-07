// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::domain::routing::{CaptureMode, DeviceSelection, RoutingCommand};
use crate::infra::pipewire::VIRTUAL_SINK_NAME;
use crate::infra::pipewire::registry::RegistrySnapshot;
use crate::persistence::settings::{
    BAR_MAX_HEIGHT, BAR_MIN_HEIGHT, BUILTIN_THEME, BarAlignment, SettingsHandle, ThemeChoice,
    ThemeFile, ThemeOrigin, canonical_theme_name,
};
use crate::ui::pages::visuals::settings::palette::{PaletteEditor, PaletteEvent};
use crate::ui::subscription::channel_subscription;
use crate::ui::theme;
use crate::util::color::with_alpha;

mod application_row;
use crate::ui::widgets::scroll_glow::ScrollGlow;
use crate::visuals::registry::{VisualKind, VisualManagerHandle, VisualSlotSnapshot};
use application_row::ApplicationRow;
use async_channel::Receiver as AsyncReceiver;
use iced::widget::text::Wrapping;
use iced::widget::{
    Column, Row, Rule, Space, button, container, pick_list, radio, rule, slider, text, text_input,
};
use iced::{Element, Length, Subscription};
use iced_layershell::actions::OutputSnapshot;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, mpsc};

const GRID_COLUMNS: usize = 2;
const TEXT_SIZE: f32 = 12.0;
const TITLE_SIZE: f32 = 14.0;
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

pub struct ConfigPage {
    routing_sender: mpsc::Sender<RoutingCommand>,
    registry_updates: Option<Arc<AsyncReceiver<RegistrySnapshot>>>,
    visual_manager: VisualManagerHandle,
    settings: SettingsHandle,
    bar_supported: bool,
    bar_monitors: Vec<String>,
    preferences: HashMap<u32, bool>,
    applications: Vec<ApplicationRow>,
    hardware_sink_label: String,
    hardware_sink_last_known: Option<String>,
    registry_ready: bool,
    applications_expanded: bool,
    capture_mode: CaptureMode,
    device_choices: Vec<DeviceOption>,
    selected_device: DeviceSelection,
    bg_palette: PaletteEditor,
    scroll: ScrollGlow,
    active_theme: String,
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
        let (current_bg, capture_mode, last_device_name, active_theme, theme_choices) = {
            let guard = settings.borrow();
            let settings = &guard.data;
            (
                settings.background_color.map_or(theme::BG_BASE, Into::into),
                settings.capture_mode,
                settings.last_device_name.clone(),
                guard.active_theme().to_owned(),
                guard.theme_store().list(),
            )
        };
        use theme::background as bg;
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
            preferences: HashMap::new(),
            applications: Vec::new(),
            hardware_sink_label: String::from("(detecting hardware sink...)"),
            hardware_sink_last_known: None,
            registry_ready: false,
            applications_expanded: false,
            capture_mode,
            device_choices: Vec::new(),
            selected_device: DeviceSelection::from_token(last_device_name),
            bg_palette,
            scroll: ScrollGlow::default(),
            active_theme,
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
                self.settings.update(|s| {
                    s.data.visuals.modules.entry(kind).or_default().enabled = Some(enabled);
                });
            }
            ConfigMessage::CaptureModeChanged(mode) => {
                if self.capture_mode != mode {
                    self.capture_mode = mode;
                    self.dispatch_capture_state();
                    self.settings.update(|s| s.data.capture_mode = mode);
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
                    self.sync_active_theme();
                }
            }
            ConfigMessage::DecorationsToggled(v) => {
                self.settings.update(|s| s.data.decorations = v)
            }
            ConfigMessage::BarModeToggled(v) => self.settings.update(|s| s.data.bar.enabled = v),
            ConfigMessage::BarAlignmentChanged(v) => {
                self.settings.update(|s| s.data.bar.alignment = v)
            }
            ConfigMessage::BarHeightChanged(v) => self.settings.update(|s| s.data.bar.height = v),
            ConfigMessage::BarMonitorChanged(v) => {
                self.settings.update(|s| s.data.bar.monitor = Some(v))
            }
            ConfigMessage::ThemeChanged(name) => self.apply_theme(&name),
            ConfigMessage::SaveTheme(name) => {
                if let Some(saved_name) = self.save_current_as_theme(&name)
                    && self.active_theme != saved_name
                {
                    self.active_theme = saved_name.clone();
                    self.settings.update(|s| s.data.theme = Some(saved_name));
                }
                self.save_theme_name.clear();
            }
            ConfigMessage::ThemeNameInput(val) => self.save_theme_name = val,
            ConfigMessage::Scrolled(g) => self.scroll = g,
        }
    }

    pub fn view(&self) -> Element<'_, ConfigMessage> {
        let mut content = Column::new()
            .spacing(14)
            .padding(8)
            .push(self.render_capture_section())
            .push(self.render_visuals_section(&self.visual_manager.borrow().snapshot()))
            .push(self.render_theme_section())
            .push(self.render_global_section());
        if self.bar_supported {
            content = content.push(self.render_bar_section());
        }
        self.scroll.vertical(content, ConfigMessage::Scrolled)
    }

    fn render_capture_section(&self) -> Column<'_, ConfigMessage> {
        let capture_controls = radio_row(
            CaptureMode::ALL,
            self.capture_mode,
            ConfigMessage::CaptureModeChanged,
        );
        let content =
            Column::new()
                .spacing(8)
                .push(capture_controls)
                .push(match self.capture_mode {
                    CaptureMode::Applications => self.render_applications_section(),
                    CaptureMode::Device => self.render_device_section(),
                });

        section_with_divider("Audio Capture", content)
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
        let summary_button = tab_button(
            format!("{indicator} Applications{status_suffix}"),
            !self.applications_expanded,
            ConfigMessage::ToggleApplicationsVisibility,
        );

        let mut section = Column::new().spacing(8).push(summary_button);
        if self.applications_expanded {
            let content: Element<'_, _> = if !self.applications.is_empty() {
                render_toggle_grid(&self.applications, |entry| {
                    (
                        entry.label.as_str(),
                        entry.enabled,
                        ConfigMessage::ToggleChanged {
                            node_id: entry.node_id,
                            enabled: !entry.enabled,
                        },
                    )
                })
                .into()
            } else {
                let msg = match (self.registry_updates.is_some(), self.registry_ready) {
                    (false, _) => "Registry unavailable; routing controls disabled.",
                    (_, true) => "No audio applications detected. Launch something to see it here.",
                    _ => "Waiting for PipeWire registry snapshots...",
                };
                text(msg).size(TEXT_SIZE).into()
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
        let mut picker = pick_list(
            self.device_choices.as_slice(),
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
        let mut choices = vec![DeviceOption {
            label: format!("Default sink - {}", &self.hardware_sink_label),
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

    fn render_global_section(&self) -> Column<'_, ConfigMessage> {
        let decorations_enabled = self.settings.borrow().data.decorations;
        let content = Column::new()
            .spacing(12)
            .push(self.bg_palette.view().map(ConfigMessage::BgPalette))
            .push(
                iced::widget::checkbox(decorations_enabled)
                    .label("Enable Window Decorations")
                    .size(14)
                    .text_size(TEXT_SIZE)
                    .on_toggle(ConfigMessage::DecorationsToggled),
            );

        section_with_divider("Global", content)
    }

    fn render_theme_section(&self) -> Column<'_, ConfigMessage> {
        let selected = self
            .theme_choices
            .iter()
            .find(|c| c.name == self.active_theme);
        let is_builtin = selected.is_some_and(|c| c.origin == ThemeOrigin::BuiltIn);

        let picker = pick_list(
            self.theme_choices.as_slice(),
            selected,
            |choice: ThemeChoice| ConfigMessage::ThemeChanged(choice.name),
        )
        .text_size(TEXT_SIZE)
        .width(Length::Fill);

        let save_btn = button(text("Save").size(TEXT_SIZE))
            .padding([4, 8])
            .style(|t, s| theme::tab_button_style(t, false, s))
            .on_press_maybe(
                (!is_builtin).then(|| ConfigMessage::SaveTheme(self.active_theme.clone())),
            );

        let save_as_input = text_input("New theme name...", &self.save_theme_name)
            .on_input(ConfigMessage::ThemeNameInput)
            .size(TEXT_SIZE)
            .width(Length::Fill);
        let trimmed = self.save_theme_name.trim();
        let save_as_btn = button(text("Save as").size(TEXT_SIZE))
            .padding([4, 8])
            .style(|t, s| theme::tab_button_style(t, false, s))
            .on_press_maybe(
                (!trimmed.is_empty() && trimmed != BUILTIN_THEME)
                    .then(|| ConfigMessage::SaveTheme(trimmed.to_owned())),
            );

        let content = Column::new()
            .spacing(8)
            .push(Row::new().spacing(8).push(picker).push(save_btn))
            .push(Row::new().spacing(8).push(save_as_input).push(save_as_btn));

        section_with_divider("Theme", content)
    }

    pub(crate) fn sync_active_theme(&mut self) {
        let (active, choices) = {
            let guard = self.settings.borrow();
            if guard.active_theme() == self.active_theme {
                return;
            }
            (guard.active_theme().to_owned(), guard.theme_store().list())
        };
        self.active_theme = active;
        self.theme_choices = choices;
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
        self.active_theme = name.to_owned();
    }

    fn save_current_as_theme(&mut self, name: &str) -> Option<String> {
        let name = canonical_theme_name(name);
        if name.is_empty() || name == BUILTIN_THEME {
            tracing::warn!("[theme] invalid theme name {name:?}");
            return None;
        }

        let theme_file = self.export_theme(&name);
        let guard = self.settings.borrow();
        let store = guard.theme_store();
        if let Err(e) = store.save(&name, &theme_file) {
            tracing::warn!("[theme] failed to save theme {name:?}: {e}");
            return None;
        }
        self.theme_choices = store.list();
        Some(name)
    }

    fn export_theme(&self, name: &str) -> ThemeFile {
        let manager = self.visual_manager.borrow();
        let palettes = manager
            .snapshot()
            .iter()
            .filter_map(|slot| {
                manager
                    .module_settings(slot.kind)?
                    .extract_palette()
                    .map(|ps| (slot.kind, ps))
            })
            .collect();
        let bg = self.settings.borrow().data.background_color;
        ThemeFile {
            name: Some(name.to_owned()),
            author: None,
            background: bg,
            palettes,
        }
    }

    pub(crate) fn sync_bar_outputs(&mut self, snapshot: OutputSnapshot) {
        self.bar_monitors = snapshot.outputs;
        if let Some(monitor) = snapshot.current
            && self.settings.borrow().data.bar.monitor.as_ref() != Some(&monitor)
        {
            self.settings.update(|s| s.data.bar.monitor = Some(monitor));
        }
    }

    fn render_bar_section(&self) -> Column<'_, ConfigMessage> {
        let bar = self.settings.borrow().data.bar.clone();
        let bar_toggle = iced::widget::checkbox(bar.enabled)
            .label("Enable Bar Mode")
            .size(14)
            .text_size(TEXT_SIZE)
            .on_toggle(ConfigMessage::BarModeToggled);
        let mut content = Column::new().spacing(10).push(bar_toggle);
        if bar.enabled {
            content = content
                .push(
                    pick_list(
                        self.bar_monitors.as_slice(),
                        bar.monitor.clone(),
                        ConfigMessage::BarMonitorChanged,
                    )
                    .placeholder("Detecting monitor...")
                    .text_size(TEXT_SIZE)
                    .width(Length::Fill),
                )
                .push(radio_row(
                    BarAlignment::ALL,
                    bar.alignment,
                    ConfigMessage::BarAlignmentChanged,
                ))
                .push(
                    slider(
                        BAR_MIN_HEIGHT..=BAR_MAX_HEIGHT,
                        bar.height.clamp(BAR_MIN_HEIGHT, BAR_MAX_HEIGHT),
                        ConfigMessage::BarHeightChanged,
                    )
                    .step(1u32)
                    .width(Length::Fill),
                )
                .push(
                    text(format!("Height: {} px", bar.height))
                        .size(TEXT_SIZE)
                        .style(theme::weak_text_style),
                );
        }
        section_with_divider("Bar Mode", content)
    }

    fn render_visuals_section(&self, snapshot: &[VisualSlotSnapshot]) -> Column<'_, ConfigMessage> {
        let enabled = snapshot.iter().filter(|s| s.enabled).count();
        let title = format!("Visual Modules ({enabled}/{})", snapshot.len());
        let content: Element<'_, ConfigMessage> = if snapshot.is_empty() {
            text("No visual modules available.").size(TEXT_SIZE).into()
        } else {
            render_toggle_grid(snapshot, |slot| {
                (
                    slot.kind.label(),
                    slot.enabled,
                    ConfigMessage::VisualToggled {
                        kind: slot.kind,
                        enabled: !slot.enabled,
                    },
                )
            })
            .into()
        };
        section_with_divider(title, content)
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
            .find_node_by_label(VIRTUAL_SINK_NAME)
            .into_iter()
            .flat_map(|sink| snapshot.route_candidates(sink))
            .map(|node| {
                seen.insert(node.id);
                ApplicationRow::from_node(
                    node,
                    self.preferences.get(&node.id).copied().unwrap_or(true),
                )
            })
            .collect();
        self.preferences.retain(|id, _| seen.contains(id));
        entries.sort_unstable_by(|a, b| a.sort_key.cmp(&b.sort_key));
        self.applications = entries;
    }

    fn dispatch_capture_state(&self) {
        if let Err(err) = self.routing_sender.send(RoutingCommand::SetCaptureState(
            self.capture_mode,
            self.selected_device.clone(),
        )) {
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

fn divider<'a>() -> Rule<'a> {
    rule::horizontal(1).style(|theme: &iced::Theme| rule::Style {
        color: with_alpha(theme.extended_palette().secondary.weak.text, 0.2),
        radius: 0.0.into(),
        fill_mode: rule::FillMode::Percent(100.0),
        snap: true,
    })
}

fn section_with_divider<'a>(
    title: impl Into<String>,
    content: impl Into<Element<'a, ConfigMessage>>,
) -> Column<'a, ConfigMessage> {
    Column::new()
        .spacing(8)
        .push(text(title.into()).size(TITLE_SIZE))
        .push(divider())
        .push(content)
}

fn radio_row<'a, T: Copy + Eq + std::fmt::Display + 'a>(
    options: &'a [T],
    selected: T,
    on_select: fn(T) -> ConfigMessage,
) -> Row<'a, ConfigMessage> {
    options.iter().fold(Row::new().spacing(12), |row, &val| {
        row.push(
            radio(val.to_string(), val, Some(selected), on_select)
                .size(14)
                .text_size(TEXT_SIZE),
        )
    })
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
            row = row.push(tab_button(label, enabled, message).width(Length::FillPortion(1)));
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

fn tab_button<'a>(
    label: impl Into<String>,
    active: bool,
    message: ConfigMessage,
) -> iced::widget::Button<'a, ConfigMessage> {
    button(
        container(text(label.into()).size(TEXT_SIZE).wrapping(Wrapping::None))
            .width(Length::Fill)
            .clip(true),
    )
    .padding(8)
    .width(Length::Fill)
    .style(move |theme, status| theme::tab_button_style(theme, active, status))
    .on_press(message)
}
