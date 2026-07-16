// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::infra::pipewire::virtual_sink;
use pipewire as pw;
use pw::registry::GlobalObject;
use pw::spa::utils::dict::DictRef;
use std::{collections::HashMap, sync::Arc};

macro_rules! extract_properties {
    ($properties:expr; $($key:literal => $binding:ident),+ $(,)?) => {
        $(let mut $binding = None;)+
        if let Some(properties) = $properties {
            for (key, value) in properties.iter() {
                match key {
                    $($key => $binding = Some(value),)+
                    _ => {}
                }
            }
        }
    };
}

crate::macros::choice_enum!(no_default all
    pub enum AudioChannel {
        FrontLeft => "FL", FrontRight => "FR", FrontCenter => "FC", LowFrequency => "LFE",
        RearLeft => "RL", RearRight => "RR", SideLeft => "SL", SideRight => "SR", Mono => "MONO",
    }
);

impl AudioChannel {
    pub(super) fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|channel| channel.label().eq_ignore_ascii_case(value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    Input,
    Output,
    #[default]
    Unknown,
}

fn pipewire_direction(value: Option<&str>) -> Direction {
    match value {
        Some(s) if s.eq_ignore_ascii_case("in") => Direction::Input,
        Some(s) if s.eq_ignore_ascii_case("out") => Direction::Output,
        _ => Direction::Unknown,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphPort {
    pub global_id: u32,
    pub port_id: u32,
    pub node_id: u32,
    pub channel: Option<AudioChannel>,
    pub direction: Direction,
    pub is_monitor: bool,
}

impl GraphPort {
    pub(super) fn from_global(global: &GlobalObject<&DictRef>) -> Option<Self> {
        extract_properties!(global.props.as_ref();
            "port.id" => port_id,
            "node.id" => node_id,
            "port.direction" => direction,
            "audio.channel" => channel,
            "port.monitor" => monitor,
        );

        Some(Self {
            global_id: global.id,
            port_id: port_id?.parse().ok()?,
            node_id: node_id?.parse().ok()?,
            direction: pipewire_direction(direction),
            channel: channel.and_then(AudioChannel::parse),
            is_monitor: monitor
                .is_some_and(|value| value.eq_ignore_ascii_case("true") || value == "1"),
        })
    }
}

fn contains_ignore_ascii_case(value: &str, pattern: &str) -> bool {
    pattern.is_empty()
        || value
            .as_bytes()
            .windows(pattern.len())
            .any(|window| window.eq_ignore_ascii_case(pattern.as_bytes()))
}

fn derive_node_direction(media_class: Option<&str>, port_direction: Option<&str>) -> Direction {
    let class = media_class.unwrap_or_default();

    if contains_ignore_ascii_case(class, "sink") || contains_ignore_ascii_case(class, "output") {
        Direction::Output
    } else if contains_ignore_ascii_case(class, "source")
        || contains_ignore_ascii_case(class, "input")
    {
        Direction::Input
    } else {
        pipewire_direction(port_direction)
    }
}

pub(super) const DEFAULT_AUDIO_SINK_KEY: &str = "default.audio.sink";
pub(super) const DEFAULT_AUDIO_SOURCE_KEY: &str = "default.audio.source";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DefaultTarget {
    pub metadata_id: Option<u32>,
    pub node_id: Option<u32>,
    pub name: Option<String>,
    pub type_hint: Option<String>,
}

impl DefaultTarget {
    fn new(metadata_id: u32, subject: u32, type_hint: Option<&str>, name: Option<&str>) -> Self {
        Self {
            metadata_id: Some(metadata_id),
            node_id: (subject != 0).then_some(subject),
            type_hint: type_hint.map(str::to_string),
            name: name.map(str::to_string),
        }
    }
}

pub(super) fn parse_metadata_name(type_hint: Option<&str>, value: &str) -> Option<String> {
    use serde_json::Value;
    let trimmed = value.trim();
    let is_json = matches!(type_hint, Some(h) if h.eq_ignore_ascii_case("Spa:String:JSON"))
        || trimmed.starts_with('{');
    if !is_json {
        return (!trimmed.is_empty()).then(|| trimmed.to_string());
    }

    match serde_json::from_str::<Value>(trimmed) {
        Ok(Value::Object(map)) => map.get("name").and_then(Value::as_str).map(str::to_string),
        Ok(Value::String(s)) => Some(s),
        _ => None,
    }
}

pub(super) fn format_target_metadata(
    object_serial: Option<&str>,
    node_id: u32,
) -> (String, String) {
    let target_object = object_serial
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
        .map_or_else(|| node_id.to_string(), str::to_owned);
    (target_object, node_id.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LinkSpec {
    pub output_node: u32,
    pub output_port: u32,
    pub input_node: u32,
    pub input_port: u32,
}

#[derive(Debug, Clone)]
pub enum RegistryCommand {
    SetLinks(Vec<LinkSpec>),
    RouteNode {
        subject: u32,
        target: Option<(String, String)>,
    },
    Sync(std::sync::mpsc::Sender<()>),
    Shutdown,
}

#[derive(Clone, Debug, Default)]
pub struct RegistrySnapshot {
    pub serial: u64,
    pub nodes: Vec<NodeInfo>,
    pub device_count: usize,
    pub defaults: MetadataDefaults,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetDescription {
    pub display: String,
    pub raw: String,
}

impl RegistrySnapshot {
    pub fn describe_default_target(&self, target: Option<&DefaultTarget>) -> TargetDescription {
        let raw = target.and_then(|t| t.name.as_deref()).unwrap_or("(none)");
        let display = target
            .and_then(|t| self.resolve_default_target(t))
            .map_or_else(|| raw.to_string(), NodeInfo::capture_device_token);
        TargetDescription {
            display,
            raw: raw.to_string(),
        }
    }

    pub fn resolve_default_target(&self, target: &DefaultTarget) -> Option<&NodeInfo> {
        target
            .node_id
            .and_then(|id| self.nodes.iter().find(|n| n.id == id))
            .or_else(|| {
                target
                    .name
                    .as_deref()
                    .and_then(|name| self.find_node_by_label(name))
            })
    }

    pub fn find_node_by_label(&self, label: &str) -> Option<&NodeInfo> {
        self.nodes.iter().find(|n| n.matches_label(label))
    }

    pub fn virtual_sink(&self) -> Option<&NodeInfo> {
        self.nodes
            .iter()
            .find(|n| n.name.as_deref() == Some(virtual_sink::NODE_NAME))
    }

    pub fn find_capture_device_by_token(&self, token: &str) -> Option<&NodeInfo> {
        let node_token_id = token
            .get(..5)
            .filter(|prefix| prefix.eq_ignore_ascii_case("node#"))
            .and_then(|_| token.get(5..))
            .and_then(|id| id.parse::<u32>().ok())
            .filter(|id| format!("node#{id}").eq_ignore_ascii_case(token));
        let candidates = || {
            self.nodes
                .iter()
                .filter(|n| n.is_capture_device_candidate())
        };
        candidates()
            .find(|n| {
                n.name
                    .as_deref()
                    .is_some_and(|name| name.eq_ignore_ascii_case(token))
            })
            .or_else(|| {
                candidates().find(|n| {
                    n.description
                        .as_deref()
                        .is_some_and(|desc| desc.eq_ignore_ascii_case(token))
                        || (n.name.is_none()
                            && n.description.is_none()
                            && node_token_id == Some(n.id))
                })
            })
    }

    pub fn route_candidates(&self, sink: &NodeInfo) -> impl Iterator<Item = &NodeInfo> {
        self.nodes.iter().filter(|n| n.should_route_to(sink))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodeInfo {
    pub id: u32,
    pub name: Option<Arc<str>>,
    pub description: Option<Arc<str>>,
    pub media_class: Option<Arc<str>>,
    pub direction: Direction,
    pub is_virtual: bool,
    pub(super) app_name: Option<Arc<str>>,
    pub(super) object_serial: Option<Arc<str>>,
    pub ports: Vec<GraphPort>,
}

impl NodeInfo {
    pub(super) fn from_global(global: &GlobalObject<&DictRef>) -> Self {
        extract_properties!(global.props.as_ref();
            "node.name" => name,
            "node.description" => description,
            "media.name" => media_name,
            "media.class" => media_class,
            "node.virtual" => virtual_node,
            "port.direction" => port_direction,
            "application.name" => app_name,
            "object.serial" => object_serial,
        );
        let name: Option<Arc<str>> = name.map(Arc::from);
        let description = description
            .or(media_name)
            .map(Arc::from)
            .or_else(|| name.clone());
        let media_class = media_class.map(Arc::from);
        let is_virtual = virtual_node.map_or_else(
            || name.as_deref() == Some(virtual_sink::NODE_NAME),
            |value| value == "true",
        );

        Self {
            id: global.id,
            direction: derive_node_direction(media_class.as_deref(), port_direction),
            name,
            description,
            media_class,
            is_virtual,
            app_name: app_name.map(Arc::from),
            object_serial: object_serial.map(Arc::from),
            ports: Vec::new(),
        }
    }

    pub fn capture_device_token(&self) -> String {
        self.name
            .as_deref()
            .or(self.description.as_deref())
            .map_or_else(|| format!("node#{}", self.id), str::to_owned)
    }

    pub fn app_name(&self) -> Option<&str> {
        self.app_name.as_deref()
    }

    pub fn object_serial(&self) -> Option<&str> {
        self.object_serial.as_deref()
    }

    pub fn matches_label(&self, label: &str) -> bool {
        [self.name.as_deref(), self.description.as_deref()]
            .into_iter()
            .flatten()
            .any(|v| v.eq_ignore_ascii_case(label))
    }

    pub fn is_capture_device_candidate(&self) -> bool {
        let contains = |value: Option<&str>, pattern| {
            value.is_some_and(|value| contains_ignore_ascii_case(value, pattern))
        };
        !self.is_virtual
            && self.app_name().is_none()
            && (contains(self.media_class.as_deref(), "audio")
                || contains(self.name.as_deref(), "monitor")
                || contains(self.description.as_deref(), "monitor"))
    }

    pub fn should_route_to(&self, sink: &Self) -> bool {
        self.id != sink.id && self.is_audio_application_output()
    }

    fn is_audio_application_output(&self) -> bool {
        self.direction == Direction::Output
            && self
                .media_class
                .as_deref()
                .is_some_and(|class| contains_ignore_ascii_case(class, "audio"))
            && self.app_name().is_some()
    }

    pub fn output_ports_for_loopback(&self) -> Vec<&GraphPort> {
        self.ports_for_loopback(Direction::Output, true)
    }

    pub fn input_ports_for_loopback(&self) -> Vec<&GraphPort> {
        self.ports_for_loopback(Direction::Input, false)
    }

    fn ports_for_loopback(&self, dir: Direction, prefer_monitor: bool) -> Vec<&GraphPort> {
        for monitor in [Some(prefer_monitor), None] {
            let ports: Vec<_> = self
                .ports
                .iter()
                .filter(|p| p.direction == dir && monitor.is_none_or(|m| p.is_monitor == m))
                .collect();
            if !ports.is_empty() {
                return ports;
            }
        }
        self.ports.iter().collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetadataDefaults {
    pub audio_sink: Option<DefaultTarget>,
    pub audio_source: Option<DefaultTarget>,
}

impl MetadataDefaults {
    pub(super) fn apply_update(
        &mut self,
        metadata_id: u32,
        subject: u32,
        key: &str,
        type_hint: Option<&str>,
        value: Option<&str>,
    ) -> bool {
        let slot = match key {
            DEFAULT_AUDIO_SINK_KEY => &mut self.audio_sink,
            DEFAULT_AUDIO_SOURCE_KEY => &mut self.audio_source,
            _ => return false,
        };

        match value {
            Some(val) => {
                let parsed_name = parse_metadata_name(type_hint, val);
                let name_ref = parsed_name.as_deref().or(Some(val));
                let target = DefaultTarget::new(metadata_id, subject, type_hint, name_ref);
                let changed = slot.as_ref() != Some(&target);
                *slot = Some(target);
                changed
            }
            None => {
                let remove = slot
                    .as_ref()
                    .is_some_and(|t| t.metadata_id == Some(metadata_id));
                if remove {
                    *slot = None;
                }
                remove
            }
        }
    }

    pub(super) fn reconcile_with_nodes(&mut self, nodes: &HashMap<u32, NodeInfo>) {
        for target in [&mut self.audio_sink, &mut self.audio_source]
            .into_iter()
            .flatten()
        {
            if target.node_id.is_some_and(|id| !nodes.contains_key(&id)) {
                target.node_id = None;
            }
            if target.node_id.is_none() {
                target.node_id = target.name.as_ref().and_then(|name| {
                    nodes
                        .iter()
                        .find(|(_, n)| n.name.as_deref() == Some(name))
                        .map(|(&id, _)| id)
                });
            }
        }
    }

    pub(super) fn clear_metadata(&mut self, metadata_id: u32) -> bool {
        self.clear_slots(|t| t.metadata_id == Some(metadata_id), |_| {})
    }

    pub(super) fn clear_node(&mut self, node_id: u32, fallback_name: Option<String>) -> bool {
        self.clear_slots(
            |t| t.node_id == Some(node_id),
            |t| {
                t.node_id = None;
                if t.name.is_none() {
                    t.name.clone_from(&fallback_name);
                }
            },
        )
    }

    fn clear_slots(
        &mut self,
        predicate: impl Fn(&DefaultTarget) -> bool,
        mutate: impl Fn(&mut DefaultTarget),
    ) -> bool {
        let mut changed = false;
        for slot in [&mut self.audio_sink, &mut self.audio_source] {
            if let Some(target) = slot
                && predicate(target)
            {
                mutate(target);
                if target.node_id.is_none() && target.name.is_none() {
                    *slot = None;
                }
                changed = true;
            }
        }
        changed
    }
}
