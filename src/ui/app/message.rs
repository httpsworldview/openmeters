// OpenMeters - an audio analysis and visualization tool
// Copyright (C) 2026  Maika Namuo
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use crate::ui::pages::config::ConfigMessage;
use crate::ui::pages::visuals::{SettingsMessage, VisualsMessage};
use iced::event::{self, Event};
use iced::keyboard::{self, Key};
use iced::{Size, Task, window};
use iced_layershell::actions::IcedXdgWindowSettings;
use iced_layershell::reexport::NewLayerShellSettings;
use iced_layershell::to_layer_message;

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
pub(super) enum Message {
    Config(ConfigMessage),
    Visuals(VisualsMessage),
    AudioFrame(Vec<f32>),
    ToggleDrawer,
    TogglePause,
    PopOutOrDock(window::Id),
    DrawerResizeStart,
    DrawerResizeMove(iced::Point),
    DrawerResizeEnd,
    BarResizeStart,
    BarResizeMove(iced::Point),
    BarResizeEnd,
    Quit,
    Resize,
    WindowOpened,
    WindowClosed(window::Id),
    WindowResized(window::Id, Size),
    WindowFocused(window::Id),
    Settings(window::Id, SettingsMessage),
}

// Forwarding functions for macro-generated private methods on Message,
// so sibling modules can access them.
pub(super) fn base_window_open(settings: IcedXdgWindowSettings) -> (window::Id, Task<Message>) {
    Message::base_window_open(settings)
}

pub(super) fn layershell_open(settings: NewLayerShellSettings) -> (window::Id, Task<Message>) {
    Message::layershell_open(settings)
}

fn drag_events(
    evt: Event,
    on_move: fn(iced::Point) -> Message,
    on_release: Message,
) -> Option<Message> {
    match evt {
        Event::Mouse(iced::mouse::Event::CursorMoved { position }) => Some(on_move(position)),
        Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
            Some(on_release)
        }
        _ => None,
    }
}

pub(super) fn drawer_drag_events(evt: Event, _: event::Status, _: window::Id) -> Option<Message> {
    drag_events(evt, Message::DrawerResizeMove, Message::DrawerResizeEnd)
}

pub(super) fn bar_drag_events(evt: Event, _: event::Status, _: window::Id) -> Option<Message> {
    drag_events(evt, Message::BarResizeMove, Message::BarResizeEnd)
}

pub(super) fn keyboard_shortcut(
    event: Event,
    _status: event::Status,
    window_id: window::Id,
) -> Option<Message> {
    let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else {
        return None;
    };
    let (ctrl, shift, no_modifiers) =
        (modifiers.control(), modifiers.shift(), modifiers.is_empty());
    match key {
        Key::Character(ch) if ctrl && shift && ch.eq_ignore_ascii_case("h") => {
            Some(Message::ToggleDrawer)
        }
        Key::Named(keyboard::key::Named::Space) if ctrl => Some(Message::PopOutOrDock(window_id)),
        Key::Character(ch) if no_modifiers && ch.eq_ignore_ascii_case("p") => {
            Some(Message::TogglePause)
        }
        Key::Character(ch) if no_modifiers && ch.eq_ignore_ascii_case("q") => Some(Message::Quit),
        _ => None,
    }
}
