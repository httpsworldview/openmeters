// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

crate::macros::choice_enum!(no_default pub enum Channel {
    Left => "Left",
    Right => "Right",
    Mid => "Mid",
    Side => "Side",
    None => "None",
});

impl Channel {
    pub(crate) fn project(self, [left, right]: [f32; 2]) -> f32 {
        match self {
            Self::Left => left,
            Self::Right => right,
            Self::Mid => (left + right) * 0.5,
            Self::Side => (left - right) * 0.5,
            Self::None => 0.0,
        }
    }
}
