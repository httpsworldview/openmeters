// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::domain::routing::{CaptureMode, StreamIdentity};
use crate::domain::visuals::VisualKind;
use crate::infra::pipewire::{CaptureControl, CaptureView};
use crate::persistence::settings::{
    BUILTIN_THEME, BarAlignment, BarHeight, SettingsHandle, ThemeChoice, ThemeName,
    VisualFrameRate, clamp_bar_height,
};
use crate::ui::outputs::{BarMonitorOption, BarOutputs};
use crate::ui::theme;
use crate::ui::widgets::palette_editor::{PaletteChange, PaletteEditor, PaletteEvent};
use crate::ui::widgets::scroll_glow::{ScrollEvent, ScrollGlow};
use crate::ui::widgets::{
    Disclosure, SliderRange, action_button, card, chip, pick, selectable_button, split, toggle,
};
use crate::visuals::registry::VisualManager;
use iced::alignment::Vertical;
use iced::widget::text::IntoFragment;
use iced::widget::{Column, Grid, Text, column, container, pick_list, row, text, text_input};
use iced::{Color, Element, Length};
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
    ThemeChanged(Option<ThemeName>),
    SaveTheme(ThemeName),
    ThemeNameInput(String),
    Scrolled(ScrollEvent),
}

pub struct ConfigPage {
    capture: CaptureControl,
    // None while PipeWire is unreachable. Refreshed before the page is shown.
    capture_view: Option<Arc<CaptureView>>,
    settings: SettingsHandle,
    bar_supported: bool,
    disclosure: Disclosure,
    bg_palette: PaletteEditor,
    scroll: ScrollGlow,
    theme_choices: Vec<ThemeChoice>,
    save_theme_name: String,
    pub(super) window_themes: [iced::Theme; 3],
}

impl ConfigPage {
    pub fn new(capture: CaptureControl, settings: SettingsHandle, bar_supported: bool) -> Self {
        let theme_choices = settings.borrow().themes();
        let mut page = Self {
            capture,
            capture_view: None,
            settings,
            bar_supported,
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
                    let background = (!self.bg_palette.is_default()).then(|| color.into());
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
            ConfigMessage::ThemeChanged(name) => settings.edit(|s| s.theme = name),
            ConfigMessage::SaveTheme(name) => {
                settings.save_theme_as(name);
                self.save_theme_name.clear();
            }
            ConfigMessage::ThemeNameInput(val) => self.save_theme_name = val,
            ConfigMessage::Scrolled(event) => self.scroll.update(event),
        }
    }

    pub(in crate::ui) fn view(
        &self,
        manager: &VisualManager,
        bar_outputs: &BarOutputs,
    ) -> Element<'_, ConfigMessage> {
        let content = column![
            self.render_capture_card(),
            self.render_visuals_card(manager),
            self.render_global_card(),
            (self.bar_supported).then(|| self.render_bar_card(bar_outputs)),
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
        let (empty_status, empty_message) = if self.capture_view.is_some() {
            (
                "none detected",
                "No audio applications detected. Launch something to see it here.",
            )
        } else {
            ("unavailable", REGISTRY_UNAVAILABLE_MESSAGE)
        };
        let summary = || {
            let disabled_count = applications
                .iter()
                .filter(|application| disabled.contains(&application.identity))
                .count();
            let status = if applications.is_empty() {
                empty_status.to_owned()
            } else {
                format!("{} total", applications.len())
            };
            let muted = (disabled_count > 0).then(|| format!("{disabled_count} disabled"));
            [Some(status), muted].into_iter().flatten().map(chip)
        };
        let body = || -> Element<'_, _> {
            if applications.is_empty() {
                return text(empty_message).size(theme::BODY_TEXT_SIZE).into();
            }
            toggle_grid(applications.iter().map(|application| {
                let enabled = !disabled.contains(&application.identity);
                let paused = if application.active { "" } else { " (paused)" };
                let message = ConfigMessage::ToggleChanged {
                    identity: application.identity.clone(),
                    enabled: !enabled,
                };
                (format!("{}{paused}", application.label), enabled, message)
            }))
            .into()
        };
        self.disclosure
            .card("Applications", summary, ConfigMessage::ToggleSection, body)
            .into()
    }

    fn render_device_section(&self) -> Column<'_, ConfigMessage> {
        let Some(view) = &self.capture_view else {
            return column![note(REGISTRY_UNAVAILABLE_MESSAGE)];
        };
        let choices = self.device_choices(view);
        let selected_token = self.settings.borrow().data.last_device_name.clone();
        let selected = choices
            .iter()
            .find(|opt| opt.selection == selected_token)
            .cloned();
        let picker = pick_list(choices, selected, |opt| {
            ConfigMessage::CaptureDeviceChanged(opt.selection)
        })
        .text_size(theme::BODY_TEXT_SIZE)
        .width(Length::Fill);

        column![
            container(picker).width(Length::Fill).clip(true),
            note("Direct device capture. Per-application taps disabled."),
        ]
        .spacing(6)
    }

    fn render_appearance_card(&self) -> container::Container<'_, ConfigMessage> {
        let active = self.settings.borrow().active_theme().cloned();
        let summary = || {
            let name = active.as_ref().map_or(BUILTIN_THEME, ThemeName::as_str);
            [chip(name.to_owned()), self.bg_palette.preview()]
        };
        let body = || {
            let selected = ThemeChoice(active.clone());
            let picker = pick_list(self.theme_choices.as_slice(), Some(selected), |choice| {
                ConfigMessage::ThemeChanged(choice.0)
            })
            .text_size(theme::BODY_TEXT_SIZE)
            .width(Length::Fill);

            let save = active.clone().map(ConfigMessage::SaveTheme);
            let save_btn = action_button("Save", save).padding([4, 8]);

            let save_as_input = text_input("New theme name...", &self.save_theme_name)
                .on_input(ConfigMessage::ThemeNameInput)
                .size(theme::BODY_TEXT_SIZE)
                .width(Length::Fill);
            let save_as = ThemeName::new(&self.save_theme_name).map(ConfigMessage::SaveTheme);
            let save_as_btn = action_button("Save as", save_as).padding([4, 8]);

            form!(
                row![picker, save_btn].spacing(theme::CONTROL_GAP);
                row![save_as_input, save_as_btn].spacing(theme::CONTROL_GAP);
                self.bg_palette.view().map(ConfigMessage::BgPalette);
            )
        };
        self.disclosure
            .card("Appearance", summary, ConfigMessage::ToggleSection, body)
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
        self.bg_palette.set_colors([background]);
        self.window_themes = theme::window_themes(background);
    }

    pub(in crate::ui) fn refresh_theme_choices(&mut self) {
        self.theme_choices = self.settings.borrow().themes();
    }

    fn render_bar_card(&self, outputs: &BarOutputs) -> container::Container<'_, ConfigMessage> {
        use ConfigMessage::{
            BarAlignmentChanged as Alignment, BarHeightChanged, BarModeToggled, BarMonitorChanged,
        };
        let bar = &self.settings.borrow().data.bar;
        let mut content = form!(toggle("Enabled", bar.enabled, BarModeToggled););
        if bar.enabled {
            let height = bar.height.get();
            let (monitors, selected) = outputs.choices(bar.monitor.as_deref());
            let status = matches!(selected, BarMonitorOption::Disconnected(_)).then(|| {
                outputs.current_name().map_or_else(
                    || "Finding a fallback...".into(),
                    |current| format!("Fallback: {current}"),
                )
            });
            let monitor = pick("Monitor", monitors, selected, |choice| {
                BarMonitorChanged(choice.monitor())
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
        manager: &VisualManager,
    ) -> container::Container<'_, ConfigMessage> {
        let summary = || {
            let enabled = manager.slots().filter(|slot| slot.enabled).count();
            let total = manager.slots().count();
            [chip(format!("{enabled} of {total} enabled"))]
        };
        let body = || {
            toggle_grid(manager.slots().map(|slot| {
                let message = ConfigMessage::VisualToggled {
                    kind: slot.kind(),
                    enabled: !slot.enabled,
                };
                (slot.kind().label().to_owned(), slot.enabled, message)
            }))
        };
        self.disclosure
            .card("Visuals", summary, ConfigMessage::ToggleSection, body)
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
        if let Some(token) = &self.settings.borrow().data.last_device_name
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

fn toggle_grid<'a>(
    items: impl Iterator<Item = (String, bool, ConfigMessage)>,
) -> Grid<'a, ConfigMessage> {
    let buttons = items.map(|(name, enabled, message)| {
        let state = if enabled { "enabled" } else { "disabled" };
        selectable_button(format!("{name} ({state})"), enabled, message).into()
    });
    Grid::with_children(buttons)
        .columns(GRID_COLUMNS)
        .spacing(6)
        .height(Length::Shrink)
}
