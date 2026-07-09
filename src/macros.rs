// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

macro_rules! choice_enum {
    (@parse [$($default:ident)?] [$($all:ident)?] no_default $($rest:tt)+) => {
        $crate::macros::choice_enum!(@parse [] [$($all)?] $($rest)+);
    };
    (@parse [$($default:ident)?] [$($all:ident)?] all $($rest:tt)+) => {
        $crate::macros::choice_enum!(@parse [$($default)?] [all] $($rest)+);
    };
    (@parse [$($default:ident)?] [$($all:ident)?] $(#[$attr:meta])* $vis:vis enum $name:ident { $($(#[$var_attr:meta])* $variant:ident => $label:expr),+ $(,)? }) => {
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

        $crate::macros::choice_enum!(@all [$($all)?] $name { $($variant,)+ });
    };
    (@all [] $($rest:tt)*) => {};
    (@all [all] $name:ident { $($variant:ident,)+ }) => {
        impl $name { pub const ALL: &'static [Self] = &[$(Self::$variant,)+]; }
    };
    ($($rest:tt)+) => { $crate::macros::choice_enum!(@parse [Default] [] $($rest)+); };
}

pub(super) use choice_enum;

macro_rules! default_struct {
    (
        $(#[$struct_attr:meta])*
        $visibility:vis struct $name:ident {
            $(
                $(#[$field_attr:meta])*
                $field_visibility:vis $field:ident: $field_type:ty = $default:expr
            ),* $(,)?
        }
    ) => {
        $(#[$struct_attr])*
        $visibility struct $name {
            $($(#[$field_attr])* $field_visibility $field: $field_type,)*
        }

        impl Default for $name {
            fn default() -> Self {
                Self { $($field: $default,)* }
            }
        }
    };
}

pub(super) use default_struct;
