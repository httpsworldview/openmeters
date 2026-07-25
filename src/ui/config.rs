// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::domain::routing::{CaptureMode, DeviceSelection, StreamIdentity};
use crate::infra::pipewire::{ApplicationView, CaptureControl, CaptureView};
use crate::persistence::settings::{
    BAR_MAX_HEIGHT, BAR_MIN_HEIGHT, BUILTIN_THEME, BarAlignment, SettingsHandle, ThemeChoice,
    ThemeFile, ThemeOrigin, canonical_theme_name,
};
use crate::ui::theme;
use crate::ui::widgets::palette_editor::{PaletteEditor, PaletteEvent};
use crate::ui::widgets::scroll_glow::ScrollGlow;
use crate::ui::widgets::{SliderRange, action_button, card, pick, selectable_button, toggle};
use crate::visuals::registry::{VisualKind, VisualManagerHandle, VisualSlotSnapshot};
use iced::widget::{Column, Row, column, container, pick_list, row, text, text_input};
use iced::{Element, Length};
use iced_layershell::actions::OutputSnapshot;
use std::sync::Arc;

const GRID_COLUMNS: usize = 2;
const MAX_DEVICE_NAME_LEN: usize = 48;
const REGISTRY_UNAVAILABLE_MESSAGE: &str = "PipeWire unavailable; reconnecting...";

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
    ToggleChanged {
        identity: StreamIdentity,
        enabled: bool,
    },
    ToggleApplicationsVisibility,
    VisualToggled {
        kind: VisualKind,
        enabled: bool,
    },
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
    capture: CaptureControl,
    view_revision: Option<u64>,
    visual_manager: VisualManagerHandle,
    settings: SettingsHandle,
    bar_supported: bool,
    bar_monitors: Vec<String>,
    applications: Arc<[ApplicationView]>,
    hardware_sink_label: String,
    registry_alive: bool,
    applications_expanded: bool,
    device_choices: Vec<DeviceOption>,
    bg_palette: PaletteEditor,
    scroll: ScrollGlow,
    theme_choices: Vec<ThemeChoice>,
    save_theme_name: String,
    pub(super) window_themes: [iced::Theme; 3],
}

impl ConfigPage {
    pub fn new(
        capture: CaptureControl,
        visual_manager: VisualManagerHandle,
        settings: SettingsHandle,
        bar_supported: bool,
    ) -> Self {
        use theme::background as bg;

        let (current_bg, theme_choices) = {
            let guard = settings.borrow();
            let data = &guard.data;
            (
                data.background_color.map_or(theme::BG_BASE, Into::into),
                guard.theme_store().list(),
            )
        };
        let window_themes = theme::window_themes(Some(current_bg));
        let mut bg_pal = theme::Palette::new(&bg::COLORS, &bg::DEFAULT_POSITIONS, bg::LABELS);
        bg_pal.set_colors(&[current_bg]);
        let bg_palette = PaletteEditor::new(bg_pal);

        Self {
            capture,
            view_revision: None,
            visual_manager,
            settings,
            bar_supported,
            bar_monitors: Vec::new(),
            applications: Arc::default(),
            hardware_sink_label: String::from("(detecting hardware sink...)"),
            registry_alive: true,
            applications_expanded: false,
            device_choices: Vec::new(),
            bg_palette,
            scroll: ScrollGlow::default(),
            theme_choices,
            save_theme_name: String::new(),
            window_themes,
        }
    }

    pub(in crate::ui) fn refresh_registry(&mut self) {
        self.registry_alive = self.capture.is_alive();
        if !self.registry_alive {
            self.view_revision = None;
            self.applications = Arc::default();
            self.device_choices.clear();
            self.hardware_sink_label = "(unavailable)".into();
            return;
        }
        let view = self.capture.view();
        if self.view_revision != Some(view.revision) {
            self.view_revision = Some(view.revision);
            self.apply_capture_view(&view);
        }
    }

    pub fn update(&mut self, message: ConfigMessage) {
        match message {
            ConfigMessage::ToggleChanged { identity, enabled } => {
                let key = identity.as_str().to_owned();
                self.settings.update(|settings| {
                    if enabled {
                        settings.data.disabled_streams.remove(&key);
                    } else {
                        settings.data.disabled_streams.insert(key);
                    }
                });
                self.dispatch_capture_config();
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
                    self.dispatch_capture_config();
                }
            }
            ConfigMessage::CaptureDeviceChanged(selection) => {
                let token = selection.token().map(str::to_owned);
                if self.settings.borrow().data.last_device_name != token {
                    self.settings.update(|s| s.data.last_device_name = token);
                    self.dispatch_capture_config();
                }
            }
            ConfigMessage::BgPalette(event) => {
                if self.bg_palette.update(event) {
                    let color = self.bg_palette.colors().first().copied();
                    self.settings.update(|s| {
                        s.data.background_color = color.map(Into::into);
                        s.update_active_theme(|theme| theme.background = color.map(Into::into));
                    });
                    self.window_themes = theme::window_themes(color);
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
            self.registry_alive,
            self.view_revision.is_some(),
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
            let settings = self.settings.borrow();
            let disabled = &settings.data.disabled_streams;
            let content: Element<'_, _> = if self.applications.is_empty() {
                let message = if !self.registry_alive {
                    REGISTRY_UNAVAILABLE_MESSAGE
                } else if self.view_revision.is_some() {
                    "No audio applications detected. Launch something to see it here."
                } else {
                    "Waiting for PipeWire registry..."
                };
                text(message).size(theme::BODY_TEXT_SIZE).into()
            } else {
                render_toggle_grid(&self.applications, |application| {
                    let enabled = !disabled.contains(application.identity.as_str());
                    (
                        application.label.as_ref(),
                        if application.active { "" } else { " (paused)" },
                        enabled,
                        ConfigMessage::ToggleChanged {
                            identity: application.identity.clone(),
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
        if !self.registry_alive {
            return column![
                text(REGISTRY_UNAVAILABLE_MESSAGE)
                    .size(theme::BODY_TEXT_SIZE)
                    .style(theme::weak_text_style)
            ];
        }

        let settings = self.settings.borrow();
        let selected_device =
            DeviceSelection::from_token(settings.data.last_device_name.as_deref());
        let selected = self
            .device_choices
            .iter()
            .find(|opt| opt.selection == selected_device);
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
            text("Direct device capture. Per-application taps disabled.")
                .size(theme::BODY_TEXT_SIZE)
                .style(theme::weak_text_style)
        ]
        .spacing(6)
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
        self.window_themes = theme::window_themes(Some(bg));
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
                    "",
                    slot.enabled,
                    ConfigMessage::VisualToggled {
                        kind: slot.kind,
                        enabled: !slot.enabled,
                    },
                )
            }),
        )
    }

    fn apply_capture_view(&mut self, view: &CaptureView) {
        self.hardware_sink_label = view.default_sink.to_string();
        if let Some(selected) = &view.selected_device {
            let changed =
                self.settings.borrow().data.last_device_name.as_deref() != Some(selected.as_ref());
            if changed {
                let selected = selected.to_string();
                self.settings
                    .update(|settings| settings.data.last_device_name = Some(selected));
            }
        }
        let mut choices = vec![DeviceOption {
            label: format!("Default sink - {}", self.hardware_sink_label),
            selection: DeviceSelection::Default,
        }];
        choices.extend(view.devices.iter().map(|token| DeviceOption {
            label: token.to_string(),
            selection: DeviceSelection::Device(token.to_string()),
        }));
        if let Some(token) = self.settings.borrow().data.last_device_name.as_deref()
            && !choices
                .iter()
                .any(|choice| choice.selection.token() == Some(token))
        {
            choices.push(DeviceOption {
                label: format!("{token} (unavailable)"),
                selection: DeviceSelection::Device(token.to_owned()),
            });
        }
        self.device_choices = choices;
        self.applications = Arc::clone(&view.applications);
    }

    fn dispatch_capture_config(&self) {
        if !self
            .capture
            .configure(self.settings.borrow().data.capture_config())
        {
            tracing::error!("[ui] PipeWire capture backend is unavailable");
        }
    }
}

fn render_toggle_grid<'a, T, F>(items: &[T], mut project: F) -> Column<'a, ConfigMessage>
where
    for<'b> F: FnMut(&'b T) -> (&'b str, &'static str, bool, ConfigMessage),
{
    let mut grid = Column::new().spacing(6);
    for chunk in items.chunks(GRID_COLUMNS) {
        let mut row = Row::new().spacing(6);
        for item in chunk {
            let (name, suffix, enabled, message) = project(item);
            let label = format!(
                "{name}{suffix} ({})",
                if enabled { "enabled" } else { "disabled" }
            );
            row =
                row.push(selectable_button(label, enabled, message).width(Length::FillPortion(1)));
        }
        grid = grid.push(row);
    }
    grid
}
