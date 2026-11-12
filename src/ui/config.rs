// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::domain::routing::{CaptureMode, StreamIdentity};
use crate::infra::pipewire::{CaptureControl, CaptureView};
use crate::persistence::settings::{
    BUILTIN_THEME, BarAlignment, BarHeight, SettingsHandle, ThemeChoice, VisualFrameRate,
    clamp_bar_height, theme_name,
};
use crate::ui::theme;
use crate::ui::widgets::palette_editor::{PaletteChange, PaletteEditor, PaletteEvent};
use crate::ui::widgets::scroll_glow::{ScrollEvent, ScrollGlow};
use crate::ui::widgets::{
    Disclosure, SliderRange, action_button, card, chip, pick, selectable_button, split, toggle,
};
use crate::visuals::registry::{VisualKind, VisualManagerHandle, VisualSlotSnapshot};
use iced::alignment::Vertical;
use iced::widget::text::IntoFragment;
use iced::widget::{Column, Text, column, container, pick_list, row, text, text_input};
use iced::{Color, Element, Length};
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

#[derive(Clone, PartialEq, Eq)]
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
}

#[derive(Default)]
struct BarOutputs {
    names: BTreeMap<u32, Option<String>>,
    current: Option<u32>,
}

impl BarOutputs {
    fn name(&self, id: u32) -> Option<&str> {
        self.names.get(&id)?.as_deref()
    }

    fn has_output(&self, name: &str) -> bool {
        self.names
            .values()
            .any(|output| output.as_deref() == Some(name))
    }

    fn selected_is_elsewhere(&self, selected: &str) -> bool {
        self.current.is_some_and(|current| {
            self.name(current) != Some(selected) && self.has_output(selected)
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

        if event == Removed && self.current == Some(id) {
            self.current = None;
        }
        let name = name.filter(|name| !name.is_empty());
        let previous = if event == Removed {
            self.names.remove(&id)
        } else {
            self.names.insert(id, name.clone())
        };

        let topology_changed = event != Updated;
        let changed = match event {
            Removed => previous.is_some(),
            Added | Updated => previous.as_ref() != Some(&name),
        };
        let route_changed = changed
            && match selected {
                None => match event {
                    Added => self.current != Some(id),
                    Removed => self.current.is_none(),
                    Updated => false,
                },
                Some(selected) => {
                    self.current == Some(id)
                        || previous.as_ref().and_then(|name| name.as_deref()) == Some(selected)
                        || name.as_deref() == Some(selected)
                        || topology_changed && self.current.is_none()
                }
            };
        let retarget = route_changed
            && match selected {
                None => event == Added && self.current.is_some_and(|current| current != id),
                Some(selected) => self.selected_is_elsewhere(selected),
            };
        if retarget {
            BarOutputChange::Retarget
        } else if route_changed {
            BarOutputChange::Changed
        } else {
            BarOutputChange::Unchanged
        }
    }

    fn set_current(&mut self, output: Option<u32>, selected: Option<&str>) -> bool {
        self.current = output.filter(|id| self.names.contains_key(id));
        selected.is_some_and(|selected| self.selected_is_elsewhere(selected))
    }

    fn choices(&self, selected: Option<&str>) -> (Vec<BarMonitorOption>, BarMonitorOption) {
        let disconnected = selected.is_some_and(|name| !self.has_output(name));
        let selected = BarMonitorOption {
            monitor: selected.map(str::to_owned),
            disconnected,
        };
        let options = std::iter::once(None)
            .chain(self.names.values().flatten().cloned().map(Some))
            .map(|monitor| BarMonitorOption {
                monitor,
                disconnected: false,
            })
            .chain(disconnected.then(|| selected.clone()))
            .collect();
        (options, selected)
    }
}

#[derive(Debug, Clone)]
pub enum ConfigMessage {
    ToggleChanged {
        identity: StreamIdentity,
        enabled: bool,
    },
    ToggleSection(&'static str),
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
    Scrolled(ScrollEvent),
}

pub struct ConfigPage {
    capture: CaptureControl,
    // None while PipeWire is unreachable. Refreshed before the page is shown.
    capture_view: Option<Arc<CaptureView>>,
    visual_manager: VisualManagerHandle,
    settings: SettingsHandle,
    bar_supported: bool,
    bar_outputs: BarOutputs,
    disclosure: Disclosure,
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
        let theme_choices = settings.borrow().themes();
        let mut page = Self {
            capture,
            capture_view: None,
            visual_manager,
            settings,
            bar_supported,
            bar_outputs: BarOutputs::default(),
            disclosure: Disclosure::default(),
            bg_palette: PaletteEditor::new(theme::background::PALETTE),
            scroll: ScrollGlow::default(),
            theme_choices,
            save_theme_name: String::new(),
            window_themes: theme::window_themes(theme::BG_BASE),
        };
        page.show_background();
        page
    }

    /// The page outlives its window, so closing it commits unfinished edits.
    pub(in crate::ui) fn commit_edits(&mut self) {
        self.update(ConfigMessage::BgPalette(PaletteEvent::Commit));
    }

    pub(in crate::ui) fn refresh_registry(&mut self) {
        if !self.capture.is_alive() {
            self.capture_view = None;
            return;
        }
        let view = self.capture.view();
        if self
            .capture_view
            .as_ref()
            .is_none_or(|current| !Arc::ptr_eq(current, &view))
        {
            if let Some(selected) = &view.selected_device {
                let selected = Some(Arc::clone(selected));
                self.settings
                    .observe(|s| s.last_device_name.clone_from(&selected));
            }
            self.capture_view = Some(view);
        }
    }

    pub(in crate::ui) fn update(&mut self, message: ConfigMessage) {
        let settings = &self.settings;
        match message {
            ConfigMessage::ToggleChanged { identity, enabled } => settings.edit(|s| {
                if enabled {
                    s.disabled_streams.remove(&identity);
                } else {
                    s.disabled_streams.insert(identity);
                }
            }),
            ConfigMessage::ToggleSection(title) => self.disclosure.toggle(title),
            ConfigMessage::VisualToggled { kind, enabled } => {
                settings.edit(|s| s.visuals.set_enabled(kind, enabled));
            }
            ConfigMessage::CaptureModeChanged(mode) => settings.edit(|s| s.capture_mode = mode),
            ConfigMessage::CaptureDeviceChanged(token) => {
                settings.edit(|s| s.last_device_name = token);
            }
            ConfigMessage::BgPalette(event) => {
                let Some(change) = self.bg_palette.update(event) else {
                    return;
                };
                let color = self.bg_palette.colors()[0];
                self.window_themes = theme::window_themes(color);
                if change == PaletteChange::Save {
                    // Stored only while it differs from the default.
                    let background = self.bg_palette.settings().map(|_| color.into());
                    settings.edit_theme(|theme| theme.background = background);
                }
            }
            ConfigMessage::VisualFrameRateChanged(rate) => {
                settings.edit(|s| s.visual_frame_rate = rate);
            }
            ConfigMessage::DecorationsToggled(value) => settings.edit(|s| s.decorations = value),
            ConfigMessage::BarModeToggled(value) => settings.edit(|s| s.bar.enabled = value),
            ConfigMessage::BarAlignmentChanged(value) => {
                settings.edit(|s| s.bar.alignment = value);
            }
            ConfigMessage::BarHeightChanged(value) => {
                settings.edit(|s| s.bar.height = clamp_bar_height(value));
            }
            ConfigMessage::BarMonitorChanged(value) => settings.edit(|s| s.bar.monitor = value),
            ConfigMessage::ThemeChanged(name) => settings.edit(|s| s.theme = theme_name(&name)),
            ConfigMessage::SaveTheme(name) => {
                settings.save_theme_as(&name);
                self.save_theme_name.clear();
            }
            ConfigMessage::ThemeNameInput(val) => self.save_theme_name = val,
            ConfigMessage::Scrolled(event) => self.scroll.update(event),
        }
    }

    pub fn view(&self) -> Element<'_, ConfigMessage> {
        let snapshot = self.visual_manager.borrow().snapshot();
        let content = column![
            self.render_capture_card(),
            self.render_visuals_card(&snapshot),
            self.render_global_card(),
            self.bar_supported.then(|| self.render_bar_card()),
            self.render_appearance_card(),
        ]
        .spacing(theme::SECTION_GAP);
        self.scroll.vertical(content, ConfigMessage::Scrolled)
    }

    fn render_capture_card(&self) -> container::Container<'_, ConfigMessage> {
        let mode = self.settings.borrow().data.capture_mode;
        let content = form!(
            pick("Mode", CaptureMode::ALL, mode, ConfigMessage::CaptureModeChanged);
            match mode {
                CaptureMode::Applications => self.render_applications_section(),
                CaptureMode::Device => self.render_device_section().into(),
            };
        );
        card("Audio Capture", content)
    }

    fn render_applications_section(&self) -> Element<'_, ConfigMessage> {
        let applications = self
            .capture_view
            .as_ref()
            .map_or(&[][..], |view| view.applications.as_slice());
        let settings = self.settings.borrow();
        let disabled = &settings.data.disabled_streams;
        let disabled_count = applications
            .iter()
            .filter(|application| disabled.contains(&application.identity))
            .count();
        let (empty_status, empty_message) = if self.capture_view.is_some() {
            (
                "none detected",
                "No audio applications detected. Launch something to see it here.",
            )
        } else {
            ("unavailable", REGISTRY_UNAVAILABLE_MESSAGE)
        };
        let status = if applications.is_empty() {
            empty_status.to_owned()
        } else {
            format!("{} total", applications.len())
        };
        let summary = [
            Some(status),
            (disabled_count > 0).then(|| format!("{disabled_count} disabled")),
        ];

        let body: Element<'_, _> = if applications.is_empty() {
            text(empty_message).size(theme::BODY_TEXT_SIZE).into()
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
        self.disclosure
            .card(
                "Applications",
                summary.into_iter().flatten().map(chip),
                ConfigMessage::ToggleSection,
                body,
            )
            .into()
    }

    fn render_device_section(&self) -> Column<'_, ConfigMessage> {
        let Some(view) = &self.capture_view else {
            return column![note(REGISTRY_UNAVAILABLE_MESSAGE)];
        };
        let choices = self.device_choices(view);
        let selected_token = self.settings.borrow().data.capture_device().cloned();
        let selected = choices
            .iter()
            .find(|opt| opt.selection == selected_token)
            .cloned();
        let available = choices.len() > 1;
        let mut picker = pick_list(choices, selected, |opt| {
            ConfigMessage::CaptureDeviceChanged(opt.selection)
        })
        .text_size(theme::BODY_TEXT_SIZE)
        .width(Length::Fill);
        if !available {
            picker = picker.placeholder("No devices available");
        }

        column![
            container(picker).width(Length::Fill).clip(true),
            note("Direct device capture. Per-application taps disabled."),
        ]
        .spacing(6)
    }

    fn render_appearance_card(&self) -> container::Container<'_, ConfigMessage> {
        let active = self.settings.borrow().active_theme().to_owned();
        let is_builtin = active == BUILTIN_THEME;
        let selected = ThemeChoice {
            name: active.clone(),
        };

        let picker = pick_list(self.theme_choices.as_slice(), Some(selected), |choice| {
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
        let save_as_btn = action_button(
            "Save as",
            theme_name(&self.save_theme_name).map(ConfigMessage::SaveTheme),
        )
        .padding([4, 8]);

        let content = form!(
            row![picker, save_btn].spacing(theme::CONTROL_GAP);
            row![save_as_input, save_as_btn].spacing(theme::CONTROL_GAP);
            self.bg_palette.view().map(ConfigMessage::BgPalette);
        );
        self.disclosure.card(
            "Appearance",
            [chip(active), self.bg_palette.preview()],
            ConfigMessage::ToggleSection,
            content,
        )
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

    pub(in crate::ui) fn show_background(&mut self) {
        let background = self.settings.borrow().theme().background;
        let background = background.map_or(theme::BG_BASE, Color::from);
        self.bg_palette.set_colors(&[background]);
        self.window_themes = theme::window_themes(background);
    }

    pub(in crate::ui) fn refresh_theme_choices(&mut self) {
        self.theme_choices = self.settings.borrow().themes();
    }

    pub(in crate::ui) fn has_bar_outputs(&self) -> bool {
        !self.bar_outputs.names.is_empty()
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
            let height = bar.height.get();
            let (monitors, selected) = self.bar_outputs.choices(bar.monitor.as_deref());
            let current = self
                .bar_outputs
                .current
                .map(|id| self.bar_outputs.name(id).unwrap_or("unnamed monitor"));
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
            let height_range = SliderRange::new(BarHeight::MIN as f32, BarHeight::MAX as f32, 1.0);
            let height_slider = slider!(
                "Height",
                height as f32,
                height_range,
                |value| BarHeightChanged(value.round() as u32),
                format!("{height} px")
            );
            content = content
                .push(split(monitor, alignment))
                .push(status.map(|status| note(status).width(Length::Fill)))
                .push(height_slider);
        }
        card("Bar Mode", content)
    }

    fn render_visuals_card(
        &self,
        snapshot: &[VisualSlotSnapshot],
    ) -> container::Container<'_, ConfigMessage> {
        let enabled = snapshot.iter().filter(|slot| slot.enabled).count();
        self.disclosure.card(
            "Visuals",
            [chip(format!("{enabled} of {} enabled", snapshot.len()))],
            ConfigMessage::ToggleSection,
            render_toggle_grid(snapshot, |slot| {
                (
                    slot.kind().label(),
                    "",
                    slot.enabled,
                    ConfigMessage::VisualToggled {
                        kind: slot.kind(),
                        enabled: !slot.enabled,
                    },
                )
            }),
        )
    }

    fn device_choices(&self, view: &CaptureView) -> Vec<DeviceOption> {
        let mut choices = vec![DeviceOption {
            label: Arc::from(format!(
                "Default sink - {}",
                view.default_sink.as_deref().unwrap_or("(none)")
            )),
            selection: None,
        }];
        choices.extend(view.devices.iter().map(|token| DeviceOption {
            label: Arc::clone(token),
            selection: Some(Arc::clone(token)),
        }));
        if let Some(token) = self.settings.borrow().data.capture_device()
            && !choices
                .iter()
                .any(|choice| choice.selection.as_ref() == Some(token))
        {
            choices.push(DeviceOption {
                label: Arc::from(format!("{token} (unavailable)")),
                selection: Some(Arc::clone(token)),
            });
        }
        choices
    }

    pub(in crate::ui) fn dispatch_capture_config(&self) {
        if !self
            .capture
            .configure(self.settings.borrow().data.capture_config())
        {
            tracing::error!("[ui] PipeWire capture backend is unavailable");
        }
    }
}

fn note<'a>(content: impl IntoFragment<'a>) -> Text<'a> {
    text(content)
        .size(theme::BODY_TEXT_SIZE)
        .style(theme::weak_text_style)
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
        outputs.names.insert(1, Some("HDMI".into()));

        assert_eq!(outputs.sync(2, Some("DP".into()), Added, None), Retarget);
        assert_eq!(outputs.sync(2, Some("DP".into()), Added, None), Unchanged);
        assert_eq!(outputs.sync(1, Some("HDMI".into()), Removed, None), Changed);

        assert_eq!(outputs.current, None);
        assert_eq!(
            outputs.sync(1, Some("HDMI".into()), Removed, None),
            Unchanged
        );
        outputs.names.insert(1, Some("HDMI".into()));
        assert!(outputs.set_current(Some(1), Some("DP")));
        assert!(!outputs.set_current(Some(1), Some("missing")));

        outputs.sync(3, None, Added, None);
        assert_eq!(outputs.names.get(&3), Some(&None));
        assert_eq!(outputs.sync(3, Some(String::new()), Added, None), Unchanged);
        let (choices, _) = outputs.choices(None);
        assert_eq!(
            choices.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["Automatic", "HDMI", "DP"]
        );
        assert!(!outputs.set_current(Some(99), None));
        assert_eq!(outputs.current, None);
    }
}
