// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::domain::routing::{CaptureMode, StreamIdentity};
use crate::infra::pipewire::{CaptureControl, CaptureView};
use crate::persistence::settings::{
    BAR_MAX_HEIGHT, BAR_MIN_HEIGHT, BUILTIN_THEME, BarAlignment, SettingsHandle, ThemeChoice,
    ThemeFile, VisualFrameRate, canonical_theme_name, clamp_bar_height,
};
use crate::ui::theme;
use crate::ui::widgets::palette_editor::{PaletteEditor, PaletteEvent};
use crate::ui::widgets::scroll_glow::ScrollGlow;
use crate::ui::widgets::{
    SliderRange, action_button, card, pick, selectable_button, split, toggle,
};
use crate::visuals::registry::{VisualKind, VisualManagerHandle, VisualSlotSnapshot};
use iced::alignment::Vertical;
use iced::widget::{Column, column, container, pick_list, row, text, text_input};
use iced::{Element, Length};
use std::collections::BTreeMap;
use std::sync::Arc;

const GRID_COLUMNS: usize = 2;
const MAX_DEVICE_NAME_LEN: usize = 48;
const REGISTRY_UNAVAILABLE_MESSAGE: &str = "PipeWire unavailable; reconnecting...";

#[derive(Clone, PartialEq, Eq)]
struct DeviceOption {
    label: Arc<str>,
    selection: Option<Arc<str>>,
}

impl std::fmt::Display for DeviceOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.label.chars().nth(MAX_DEVICE_NAME_LEN).is_some() {
            write!(f, "{:.1$}...", self.label, MAX_DEVICE_NAME_LEN - 3)
        } else {
            f.write_str(&self.label)
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
struct BarMonitorOption {
    monitor: Option<String>,
    disconnected: bool,
}

impl std::fmt::Display for BarMonitorOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.monitor {
            None => f.write_str("Automatic"),
            Some(name) if !self.disconnected => f.write_str(name),
            Some(name) => write!(f, "{name} (disconnected)"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::ui) enum BarOutputEvent {
    Added,
    Updated,
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::ui) enum BarOutputChange {
    Unchanged,
    Changed,
    Retarget,
    CurrentRemoved,
}

#[derive(Default)]
struct BarOutputs {
    names: BTreeMap<u32, String>,
    current: Option<u32>,
}

impl BarOutputs {
    fn selected_is_elsewhere(&self, selected: &str) -> bool {
        self.current.is_some_and(|current| {
            self.names.get(&current).map(String::as_str) != Some(selected)
                && self.names.values().any(|name| name == selected)
        })
    }

    fn sync(
        &mut self,
        id: u32,
        name: Option<String>,
        event: BarOutputEvent,
        selected: Option<&str>,
    ) -> BarOutputChange {
        use BarOutputEvent::{Added, Removed, Updated};

        let current_removed = event == Removed && self.current == Some(id);
        let name = name.filter(|name| !name.is_empty());
        let previous = self.names.remove(&id);
        if event != Removed
            && let Some(name) = &name
        {
            self.names.insert(id, name.clone());
        }

        let topology_changed = event != Updated;
        let changed = event == Removed
            || previous.as_deref() != name.as_deref()
            || event == Added && previous.is_none();
        let route_changed = changed
            && match selected {
                None => match event {
                    Added => self.current != Some(id),
                    Removed => self.current.is_none_or(|current| current == id),
                    Updated => false,
                },
                Some(selected) => {
                    self.current == Some(id)
                        || previous.as_deref() == Some(selected)
                        || name.as_deref() == Some(selected)
                        || topology_changed && self.current.is_none()
                }
            };
        let retarget = route_changed
            && match selected {
                None => event == Added && self.current.is_some_and(|current| current != id),
                Some(selected) => self.selected_is_elsewhere(selected),
            };
        if current_removed {
            BarOutputChange::CurrentRemoved
        } else if retarget {
            BarOutputChange::Retarget
        } else if route_changed {
            BarOutputChange::Changed
        } else {
            BarOutputChange::Unchanged
        }
    }

    fn set_current(&mut self, output: Option<u32>, selected: Option<&str>) -> bool {
        self.current = output;
        output.is_some() && selected.is_some_and(|selected| self.selected_is_elsewhere(selected))
    }

    fn choices(&self, selected: Option<&str>) -> (Vec<BarMonitorOption>, BarMonitorOption) {
        let disconnected =
            selected.is_some_and(|name| !self.names.values().any(|output| output == name));
        let selected = BarMonitorOption {
            monitor: selected.map(str::to_owned),
            disconnected,
        };
        let mut options = vec![BarMonitorOption::default()];
        options.extend(
            self.names
                .values()
                .cloned()
                .map(|monitor| BarMonitorOption {
                    monitor: Some(monitor),
                    disconnected: false,
                }),
        );
        if disconnected {
            options.push(selected.clone());
        }
        (options, selected)
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
    CaptureDeviceChanged(Option<Arc<str>>),
    BgPalette(PaletteEvent),
    VisualFrameRateChanged(VisualFrameRate),
    DecorationsToggled(bool),
    BarModeToggled(bool),
    BarAlignmentChanged(BarAlignment),
    BarHeightChanged(u32),
    BarMonitorChanged(Option<String>),
    ThemeChanged(String),
    SaveTheme(String),
    ThemeNameInput(String),
    Scrolled(ScrollGlow),
}

pub(in crate::ui) enum BarChange {
    Mode,
    Layout,
    Monitor,
}

pub(in crate::ui) enum ConfigEffect {
    VisualToggled { kind: VisualKind, enabled: bool },
    FrameRateChanged(VisualFrameRate),
    DecorationsChanged,
    BarChanged(BarChange),
    ThemeChanged,
}

pub struct ConfigPage {
    capture: CaptureControl,
    capture_view: Option<Arc<CaptureView>>,
    visual_manager: VisualManagerHandle,
    settings: SettingsHandle,
    bar_supported: bool,
    bar_outputs: BarOutputs,
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
            capture_view: None,
            visual_manager,
            settings,
            bar_supported,
            bar_outputs: BarOutputs::default(),
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
            self.capture_view = None;
            self.device_choices.clear();
            return;
        }
        let view = self.capture.view();
        if self
            .capture_view
            .as_ref()
            .is_none_or(|current| !Arc::ptr_eq(current, &view))
        {
            self.apply_capture_view(&view);
            self.capture_view = Some(view);
        }
    }

    pub(in crate::ui) fn update(&mut self, message: ConfigMessage) -> Option<ConfigEffect> {
        let mut effect = None;
        match message {
            ConfigMessage::ToggleChanged { identity, enabled } => {
                self.settings.update(|settings| {
                    if enabled {
                        settings.data.disabled_streams.remove(&identity);
                    } else {
                        settings.data.disabled_streams.insert(identity);
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
                effect = Some(ConfigEffect::VisualToggled { kind, enabled });
            }
            ConfigMessage::CaptureModeChanged(mode) => {
                if self.settings.set(|s| &mut s.capture_mode, mode) {
                    self.dispatch_capture_config();
                }
            }
            ConfigMessage::CaptureDeviceChanged(token) => {
                if self.settings.set(|s| &mut s.last_device_name, token) {
                    self.dispatch_capture_config();
                }
            }
            ConfigMessage::BgPalette(event) => {
                if self.bg_palette.update(event) {
                    let color = self.bg_palette.colors()[0];
                    self.settings.update(|s| {
                        s.data.background_color = Some(color.into());
                        s.update_active_theme(|theme| theme.background = Some(color.into()));
                    });
                    self.window_themes = theme::window_themes(Some(color));
                    self.refresh_theme_choices_if_needed();
                }
            }
            ConfigMessage::VisualFrameRateChanged(rate) => {
                self.settings.update(|s| s.data.visual_frame_rate = rate);
                effect = Some(ConfigEffect::FrameRateChanged(rate));
            }
            ConfigMessage::DecorationsToggled(value) => {
                self.settings.update(|s| s.data.decorations = value);
                effect = Some(ConfigEffect::DecorationsChanged);
            }
            ConfigMessage::BarModeToggled(value) => {
                if self.settings.set(|s| &mut s.bar.enabled, value) {
                    effect = Some(ConfigEffect::BarChanged(BarChange::Mode));
                }
            }
            ConfigMessage::BarAlignmentChanged(value) => {
                self.settings.update(|s| s.data.bar.alignment = value);
                effect = Some(ConfigEffect::BarChanged(BarChange::Layout));
            }
            ConfigMessage::BarHeightChanged(value) => {
                self.settings.update(|s| s.data.bar.height = value);
                effect = Some(ConfigEffect::BarChanged(BarChange::Layout));
            }
            ConfigMessage::BarMonitorChanged(value) => {
                if self.settings.set(|s| &mut s.bar.monitor, value) {
                    effect = Some(ConfigEffect::BarChanged(BarChange::Monitor));
                }
            }
            ConfigMessage::ThemeChanged(name) => {
                self.apply_theme(&name);
                effect = Some(ConfigEffect::ThemeChanged);
            }
            ConfigMessage::SaveTheme(name) => {
                if let Some(saved_name) = self.save_current_as_theme(&name) {
                    self.settings.set(|s| &mut s.theme, Some(saved_name));
                }
                self.save_theme_name.clear();
            }
            ConfigMessage::ThemeNameInput(val) => self.save_theme_name = val,
            ConfigMessage::Scrolled(g) => self.scroll = g,
        }
        effect
    }

    pub fn view(&self) -> Element<'_, ConfigMessage> {
        let snapshot = self.visual_manager.borrow().snapshot();
        let mut content = column![
            self.render_capture_card(),
            Self::render_visuals_card(&snapshot),
            self.render_global_card(),
        ]
        .spacing(theme::SECTION_GAP);
        if self.bar_supported {
            content = content.push(self.render_bar_card());
        }
        content = content.push(self.render_appearance_card());
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
        let applications = self
            .capture_view
            .as_ref()
            .map_or(&[][..], |view| view.applications.as_ref());
        let status_suffix: String = match (
            applications.len(),
            self.registry_alive,
            self.capture_view.is_some(),
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
            let content: Element<'_, _> = if applications.is_empty() {
                let message = if !self.registry_alive {
                    REGISTRY_UNAVAILABLE_MESSAGE
                } else if self.capture_view.is_some() {
                    "No audio applications detected. Launch something to see it here."
                } else {
                    "Waiting for PipeWire registry..."
                };
                text(message).size(theme::BODY_TEXT_SIZE).into()
            } else {
                render_toggle_grid(applications, |application| {
                    let enabled = !disabled.contains(&application.identity);
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
        let selected_token = settings
            .data
            .last_device_name
            .as_deref()
            .filter(|token| !token.is_empty());
        let selected = self
            .device_choices
            .iter()
            .find(|opt| opt.selection.as_deref() == selected_token);
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

    fn render_appearance_card(&self) -> container::Container<'_, ConfigMessage> {
        let active = self.settings.borrow().active_theme().to_owned();
        let selected = self.theme_choices.iter().find(|c| c.name == active);
        let is_builtin = selected.is_some_and(|c| c.name == BUILTIN_THEME);

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
            self.bg_palette.view().map(ConfigMessage::BgPalette);
        );
        card("Appearance", content)
    }

    fn render_global_card(&self) -> container::Container<'_, ConfigMessage> {
        use ConfigMessage::{
            DecorationsToggled as Decorations, VisualFrameRateChanged as FrameRate,
        };
        let data = &self.settings.borrow().data;
        let frame_rate = data.visual_frame_rate;
        let frame_rate = pick("Frame rate", VisualFrameRate::ALL, frame_rate, FrameRate);
        let decorations = toggle("Window decorations", data.decorations, Decorations);
        card(
            "Global",
            split(frame_rate, decorations).align_y(Vertical::Center),
        )
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

        let theme_file = ThemeFile {
            name: Some(name.clone()),
            author: None,
            background: self.settings.borrow().data.background_color,
            palettes: self.visual_manager.borrow().theme_palettes().collect(),
        };
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

    pub(in crate::ui) fn sync_bar_output(
        &mut self,
        id: u32,
        name: Option<String>,
        event: BarOutputEvent,
    ) -> BarOutputChange {
        let settings = self.settings.borrow();
        self.bar_outputs
            .sync(id, name, event, settings.data.bar.monitor.as_deref())
    }

    pub(in crate::ui) fn sync_current_bar_output(&mut self, output: Option<u32>) -> bool {
        let settings = self.settings.borrow();
        self.bar_outputs
            .set_current(output, settings.data.bar.monitor.as_deref())
    }

    fn render_bar_card(&self) -> container::Container<'_, ConfigMessage> {
        use ConfigMessage::{
            BarAlignmentChanged as Alignment, BarHeightChanged, BarModeToggled, BarMonitorChanged,
        };
        let bar = &self.settings.borrow().data.bar;
        let mut content = form!(toggle("Enabled", bar.enabled, BarModeToggled););
        if bar.enabled {
            let height = clamp_bar_height(bar.height);
            let (monitors, selected) = self.bar_outputs.choices(bar.monitor.as_deref());
            let current = self.bar_outputs.current.map(|id| {
                self.bar_outputs
                    .names
                    .get(&id)
                    .map_or("unnamed monitor", String::as_str)
            });
            let status = selected.disconnected.then(|| {
                current.map_or_else(
                    || "Finding a fallback...".into(),
                    |current| format!("Fallback: {current}"),
                )
            });
            let monitor = pick("Monitor", monitors, selected, |choice| {
                BarMonitorChanged(choice.monitor)
            });
            let alignment = pick("Alignment", BarAlignment::ALL, bar.alignment, Alignment);
            let height_range = SliderRange::new(BAR_MIN_HEIGHT as f32, BAR_MAX_HEIGHT as f32, 1.0);
            let height_slider = slider!(
                "Height",
                height as f32,
                height_range,
                |value| BarHeightChanged(value.round() as u32),
                format!("{height} px")
            );
            content = content.push(split(monitor, alignment));
            if let Some(status) = status {
                content = content.push(
                    text(status)
                        .size(theme::BODY_TEXT_SIZE)
                        .style(theme::weak_text_style)
                        .width(Length::Fill),
                );
            }
            content = content.push(height_slider);
        }
        card("Bar Mode", content)
    }

    fn render_visuals_card(
        snapshot: &[VisualSlotSnapshot],
    ) -> container::Container<'static, ConfigMessage> {
        card(
            "Visuals",
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
        if let Some(selected) = &view.selected_device {
            self.settings
                .set(|s| &mut s.last_device_name, Some(Arc::clone(selected)));
        }
        let mut choices = vec![DeviceOption {
            label: Arc::from(format!("Default sink - {}", view.default_sink)),
            selection: None,
        }];
        choices.extend(view.devices.iter().map(|token| DeviceOption {
            label: Arc::clone(token),
            selection: Some(Arc::clone(token)),
        }));
        if let Some(token) = self.settings.borrow().data.last_device_name.as_ref()
            && !choices
                .iter()
                .any(|choice| choice.selection.as_ref() == Some(token))
        {
            choices.push(DeviceOption {
                label: Arc::from(format!("{token} (unavailable)")),
                selection: Some(Arc::clone(token)),
            });
        }
        self.device_choices = choices;
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

fn render_toggle_grid<T, F>(items: &[T], mut project: F) -> Column<'static, ConfigMessage>
where
    for<'b> F: FnMut(&'b T) -> (&'b str, &'static str, bool, ConfigMessage),
{
    column(items.chunks(GRID_COLUMNS).map(|chunk| {
        row(chunk.iter().map(|item| {
            let (name, suffix, enabled, message) = project(item);
            let state = if enabled { "enabled" } else { "disabled" };
            selectable_button(format!("{name}{suffix} ({state})"), enabled, message)
                .width(Length::FillPortion(1))
                .into()
        }))
        .spacing(6)
        .into()
    }))
    .spacing(6)
}

#[cfg(test)]
mod tests {
    use super::{BarOutputChange::*, BarOutputEvent::*, BarOutputs};

    #[test]
    fn bar_retargeting_follows_output_routes() {
        let mut outputs = BarOutputs {
            current: Some(1),
            ..Default::default()
        };
        outputs.names.insert(1, "HDMI".into());

        assert_eq!(outputs.sync(2, Some("DP".into()), Added, None), Retarget);
        assert_eq!(outputs.sync(2, Some("DP".into()), Added, None), Unchanged);
        assert_eq!(
            outputs.sync(1, Some("HDMI".into()), Removed, None),
            CurrentRemoved
        );

        outputs.names.insert(1, "HDMI".into());
        assert!(outputs.set_current(Some(1), Some("DP")));
        assert!(!outputs.set_current(Some(1), Some("missing")));
    }
}
