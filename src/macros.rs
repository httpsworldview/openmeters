// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

macro_rules! choice_enum {
    (@build [$($default:ident)?] $(#[$attr:meta])* $vis:vis enum $name:ident { $($(#[$var_attr:meta])* $variant:ident => $label:expr),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq $(, $default)?)]
        #[serde(rename_all = "snake_case")]
        $(#[$attr])*
        $vis enum $name { $($(#[$var_attr])* $variant,)+ }

        impl $name {
            pub const fn label(self) -> &'static str { match self { $(Self::$variant => $label),+ } }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.label()) }
        }
    };
    (@build_all [$($default:ident)?] $(#[$attr:meta])* $vis:vis enum $name:ident { $($(#[$var_attr:meta])* $variant:ident => $label:expr),+ $(,)? }) => {
        $crate::macros::choice_enum!(@build [$($default)?] $(#[$attr])* $vis enum $name { $($(#[$var_attr])* $variant => $label,)+ });
        impl $name { pub const ALL: &'static [Self] = &[$(Self::$variant,)+]; }
    };
    (no_default all $($rest:tt)+) => { $crate::macros::choice_enum!(@build_all [] $($rest)+); };
    (all $($rest:tt)+) => { $crate::macros::choice_enum!(@build_all [Default] $($rest)+); };
    (no_default $($rest:tt)+) => { $crate::macros::choice_enum!(@build [] $($rest)+); };
    ($($rest:tt)+) => { $crate::macros::choice_enum!(@build [Default] $($rest)+); };
}

pub(super) use choice_enum;
