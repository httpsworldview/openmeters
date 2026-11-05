// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

macro_rules! choice_enum {
    ($(#[$attr:meta])* $vis:vis enum $name:ident { $($(#[$var_attr:meta])* $variant:ident => $label:expr),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
        #[serde(rename_all = "snake_case")]
        $(#[$attr])*
        $vis enum $name { $($(#[$var_attr])* $variant,)+ }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];
            pub const fn label(self) -> &'static str { match self { $(Self::$variant => $label),+ } }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.label()) }
        }
    };
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

        impl std::default::Default for $name {
            fn default() -> Self {
                Self { $($field: $default,)* }
            }
        }
    };
}

pub(super) use default_struct;

macro_rules! bounded {
    ($($(#[$attr:meta])* $vis:vis struct $name:ident($ty:ty) in $range:expr;)+) => {$(
        $(#[$attr])*
        #[derive(Debug, Clone, Copy, PartialEq)]
        $vis struct $name($ty);

        impl $name {
            pub const MIN: $ty = *($range).start();
            pub const MAX: $ty = *($range).end();

            pub const fn new(value: $ty) -> Option<Self> {
                if Self::MIN <= value && value <= Self::MAX {
                    Some(Self(value))
                } else {
                    None
                }
            }

            pub const fn must(value: $ty) -> Self {
                match Self::new(value) {
                    Some(value) => value,
                    None => panic!(concat!(stringify!($name), " out of range")),
                }
            }

            pub const fn get(self) -> $ty {
                self.0
            }

            #[allow(dead_code, reason = "not every bounded type takes UI input")]
            pub fn set(&mut self, value: $ty) -> bool {
                Self::new(value).is_some_and(|value| $crate::util::set_if_changed(self, value))
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serde::Serialize::serialize(&self.0, serializer)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = <$ty as serde::Deserialize>::deserialize(deserializer)?;
                Self::new(value)
                    .ok_or_else(|| serde::de::Error::custom(format!("{value:?} is out of range")))
            }
        }
    )+};
}

pub(super) use bounded;

macro_rules! widget_method {
    (layout $size:expr, |$limits:ident| $resolved:expr) => {
        fn size(&self) -> iced::Size<iced::Length> {
            $size
        }

        fn layout(
            &mut self,
            _: &mut iced::advanced::widget::Tree,
            _: &iced::Renderer,
            $limits: &iced::advanced::layout::Limits,
        ) -> iced::advanced::layout::Node {
            iced::advanced::layout::Node::new($resolved)
        }
    };
    (state $state:ty) => {
        fn tag(&self) -> iced::advanced::widget::tree::Tag {
            iced::advanced::widget::tree::Tag::of::<$state>()
        }
        fn state(&self) -> iced::advanced::widget::tree::State {
            iced::advanced::widget::tree::State::new(<$state>::default())
        }
    };
    (update $message:ty; $this:ident; $tree:pat, $event:pat, $layout:pat, $cursor:pat, $renderer:pat, $clipboard:pat, $shell:pat, $viewport:pat => $body:block) => {
        fn update(
            &mut self,
            $tree: &mut iced::advanced::widget::Tree,
            $event: &iced::Event,
            $layout: iced::advanced::Layout<'_>,
            $cursor: iced::advanced::mouse::Cursor,
            $renderer: &iced::Renderer,
            $clipboard: &mut dyn iced::advanced::Clipboard,
            $shell: &mut iced::advanced::Shell<'_, $message>,
            $viewport: &iced::Rectangle,
        ) {
            let $this = self;
            $body
        }
    };
    (draw $this:ident; $tree:pat, $renderer:pat, $theme:pat, $style:pat, $layout:pat, $cursor:pat, $viewport:pat => $body:block) => {
        fn draw(
            &self,
            $tree: &iced::advanced::widget::Tree,
            $renderer: &mut iced::Renderer,
            $theme: &iced::Theme,
            $style: &iced::advanced::renderer::Style,
            $layout: iced::advanced::Layout<'_>,
            $cursor: iced::advanced::mouse::Cursor,
            $viewport: &iced::Rectangle,
        ) {
            let $this = self;
            $body
        }
    };
}

pub(super) use widget_method;
