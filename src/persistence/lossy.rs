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

pub(super) fn settings<T: Default>(
    value: Value,
    scope: &str,
    f: impl FnOnce(&mut Map<String, Value>, &mut T),
) -> T {
    let mut out = T::default();
    if let Some(mut map) = object(value, scope) {
        f(&mut map, &mut out);
        unknown(scope, &map);
    }
    out
}

pub(super) fn field<T: DeserializeOwned>(
    map: &mut Map<String, Value>,
    key: &str,
    out: &mut T,
    scope: &str,
) {
    if let Some(value) = map.remove(key)
        && let Err(err) = T::deserialize(value).map(|value| *out = value)
    {
        warn!("[settings] invalid {scope}.{key}: {err}");
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

pub(super) fn unknown(scope: &str, map: &Map<String, Value>) {
    for key in map.keys() {
        warn!("[settings] unsupported {scope}.{key}");
    }
}
