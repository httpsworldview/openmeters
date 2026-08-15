// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::domain::routing::{CaptureMode, StreamIdentity};
use crate::infra::pipewire::{CaptureControl, CaptureView};
use crate::persistence::settings::{
    BAR_MAX_HEIGHT, BAR_MIN_HEIGHT, BUILTIN_THEME, BarAlignment, SettingsHandle, ThemeChoice,
    ThemeFile, VisualFrameRate, canonical_theme_name,
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
use iced_layershell::actions::OutputSnapshot;
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
    BarMonitorChanged(String),
    ThemeChanged(String),
    SaveTheme(String),
    ThemeNameInput(String),
    Scrolled(ScrollGlow),
}

pub struct ConfigPage {
    capture: CaptureControl,
    capture_view: Option<Arc<CaptureView>>,
    visual_manager: VisualManagerHandle,
    settings: SettingsHandle,
    bar_supported: bool,
    bar_monitors: Vec<String>,
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
            bar_monitors: Vec::new(),
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

    pub fn update(&mut self, message: ConfigMessage) {
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
            }
            ConfigMessage::CaptureModeChanged(mode) => {
                if self.settings.borrow().data.capture_mode != mode {
                    self.settings.update(|s| s.data.capture_mode = mode);
                    self.dispatch_capture_config();
                }
            }
            ConfigMessage::CaptureDeviceChanged(token) => {
                if self.settings.borrow().data.last_device_name != token {
                    self.settings.update(|s| s.data.last_device_name = token);
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
        let bar = &self.settings.borrow().data.bar;
        let mut content = form!(toggle("Enabled", bar.enabled, BarModeToggled););
        if bar.enabled {
            let height = bar.height.clamp(BAR_MIN_HEIGHT, BAR_MAX_HEIGHT);
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
            .align_y(Vertical::Center)
            .width(Length::Fill);
            let alignment = pick("Alignment", BarAlignment::ALL, bar.alignment, Alignment);
            let height_range = SliderRange::new(BAR_MIN_HEIGHT as f32, BAR_MAX_HEIGHT as f32, 1.0);
            let height_slider = slider!(
                "Height",
                height as f32,
                height_range,
                |value| BarHeightChanged(value.round() as u32),
                format!("{height} px")
            );
            content = content.push(split(monitor, alignment)).push(height_slider);
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
            let changed =
                self.settings.borrow().data.last_device_name.as_deref() != Some(selected.as_ref());
            if changed {
                let selected = Arc::clone(selected);
                self.settings
                    .update(|settings| settings.data.last_device_name = Some(selected));
            }
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
