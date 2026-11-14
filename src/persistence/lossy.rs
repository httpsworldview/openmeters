// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use tracing::warn;

pub(super) fn object(value: Value, scope: &str) -> Option<Map<String, Value>> {
    let Value::Object(map) = value else {
        warn!("[settings] {scope} must be an object");
        return None;
    };
    Some(map)
}

// The object at `value` read over `out`, or None when it is not an object.
pub(super) fn read<T>(
    value: Value,
    scope: &str,
    mut out: T,
    fields: impl FnOnce(&mut Map<String, Value>, &mut T, &str),
) -> Option<T> {
    let mut map = object(value, scope)?;
    fields(&mut map, &mut out, scope);
    unknown(scope, &map);
    Some(out)
}

pub(super) fn settings<T: Default>(
    value: Value,
    scope: &str,
    fields: impl FnOnce(&mut Map<String, Value>, &mut T, &str),
) -> T {
    read(value, scope, T::default(), fields).unwrap_or_default()
}

// The object under `key`, read over defaults in a scope that extends this one.
pub(super) fn nested<T: Default>(
    map: &mut Map<String, Value>,
    key: &str,
    out: &mut T,
    scope: &str,
    fields: impl FnOnce(&mut Map<String, Value>, &mut T, &str),
) {
    if let Some(value) = map.remove(key) {
        *out = settings(value, &format!("{scope}.{key}"), fields);
    }
}

pub(super) fn field<T: DeserializeOwned>(
    map: &mut Map<String, Value>,
    key: &str,
    out: &mut T,
    scope: &str,
) {
    if let Some(value) = map.remove(key)
        && let Some(value) = self::value(value, &format!("{scope}.{key}"))
    {
        *out = value;
    }
}

pub(super) fn value<T: DeserializeOwned>(value: Value, scope: &str) -> Option<T> {
    T::deserialize(value)
        .inspect_err(|err| warn!("[settings] invalid {scope}: {err}"))
        .ok()
}

macro_rules! fields {
    ($map:expr, $out:expr, $scope:expr; $($field:ident),+ $(,)?) => {{
        let (map, out, scope) = (&mut *$map, &mut *$out, $scope);
        $($crate::persistence::lossy::field(map, stringify!($field), &mut out.$field, scope);)+
    }};
}
pub(super) use fields;

// A struct with per-field defaults, and a reader over exactly those fields.
macro_rules! settings_struct {
    (
        $(#[$attr:meta])*
        $vis:vis struct $name:ident {
            $($(#[$field_attr:meta])* $field_vis:vis $field:ident: $ty:ty = $default:expr),* $(,)?
        }
    ) => {
        $crate::macros::default_struct! {
            $(#[$attr])*
            $vis struct $name {
                $($(#[$field_attr])* $field_vis $field: $ty = $default,)*
            }
        }

        impl $name {
            pub(in $crate::persistence) fn read_fields(
                map: &mut serde_json::Map<String, serde_json::Value>,
                out: &mut Self,
                scope: &str,
            ) {
                $crate::persistence::lossy::fields!(map, out, scope; $($field),*);
            }
        }
    };
}
pub(super) use settings_struct;

pub(super) fn unknown(scope: &str, map: &Map<String, Value>) {
    for key in map.keys() {
        warn!("[settings] unsupported {scope}.{key}");
    }
}
