// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{ApplicationView, CaptureView, MAX_CAPTURE_CHANNELS};
use crate::domain::routing::StreamIdentity;
use crate::dsp::ChannelPosition;
use pipewire as pw;
use pw::registry::GlobalObject;
use pw::spa::utils::dict::DictRef;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

macro_rules! properties {
    ($props:expr; $($key:literal => $name:ident),+ $(,)?) => {
        $(let mut $name = None;)+
        if let Some(props) = $props {
            for (key, value) in props.iter() {
                match key {
                    $($key => $name = Some(value),)+
                    _ => {}
                }
            }
        }
    };
}

pub(super) type Channel = ChannelPosition;

macro_rules! channel_positions {
    ($($name:literal => $variant:ident => $id:path),* $(,)?) => {
        impl ChannelPosition {
            fn parse(value: &str) -> Option<Self> {
                let value = value.trim();
                [$(($name, Self::$variant)),*]
                    .into_iter()
                    .find_map(|(name, channel)| value.eq_ignore_ascii_case(name).then_some(channel))
                    .or_else(|| {
                        value
                            .get(..3)
                            .filter(|prefix| prefix.eq_ignore_ascii_case("AUX"))
                            .and_then(|_| value.get(3..))
                            .and_then(|number| number.parse::<u8>().ok())
                            .filter(|number| *number < MAX_CAPTURE_CHANNELS as u8)
                            .map(Self::Aux)
                    })
            }

            pub(super) const fn spa_id(self) -> u32 {
                match self {
                    $(Self::$variant => $id,)*
                    Self::Aux(index) => pw::spa::sys::SPA_AUDIO_CHANNEL_AUX0 + index as u32,
                    Self::Unknown => 0,
                }
            }

            pub(super) fn from_spa_id(id: u32) -> Self {
                match id {
                    $($id => Self::$variant,)*
                    id if (pw::spa::sys::SPA_AUDIO_CHANNEL_AUX0
                        ..pw::spa::sys::SPA_AUDIO_CHANNEL_AUX0 + MAX_CAPTURE_CHANNELS as u32)
                        .contains(&id) => {
                            Self::Aux((id - pw::spa::sys::SPA_AUDIO_CHANNEL_AUX0) as u8)
                        },
                    _ => Self::Unknown,
                }
            }
        }
    };
}

channel_positions! {
    "FL" => FrontLeft => pw::spa::sys::SPA_AUDIO_CHANNEL_FL,
    "FR" => FrontRight => pw::spa::sys::SPA_AUDIO_CHANNEL_FR,
    "FC" => FrontCenter => pw::spa::sys::SPA_AUDIO_CHANNEL_FC,
    "LFE" => LowFrequency => pw::spa::sys::SPA_AUDIO_CHANNEL_LFE,
    "RL" => RearLeft => pw::spa::sys::SPA_AUDIO_CHANNEL_RL,
    "RR" => RearRight => pw::spa::sys::SPA_AUDIO_CHANNEL_RR,
    "SL" => SideLeft => pw::spa::sys::SPA_AUDIO_CHANNEL_SL,
    "SR" => SideRight => pw::spa::sys::SPA_AUDIO_CHANNEL_SR,
    "MONO" => Mono => pw::spa::sys::SPA_AUDIO_CHANNEL_MONO,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CaptureLayout {
    pub channels: Vec<Channel>,
    pub truncated: usize,
}

impl CaptureLayout {
    pub(super) fn stereo() -> Self {
        Self {
            channels: vec![Channel::FrontLeft, Channel::FrontRight],
            truncated: 0,
        }
    }

    pub(super) fn surround() -> Self {
        Self {
            channels: Channel::SURROUND.into(),
            truncated: 0,
        }
    }

    pub(super) fn spa_positions(&self) -> [u32; pw::spa::sys::SPA_AUDIO_MAX_CHANNELS as usize] {
        let mut positions = [0; pw::spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
        for (position, channel) in positions.iter_mut().zip(&self.channels) {
            *position = channel.spa_id();
        }
        positions
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Direction {
    Input,
    Output,
    #[default]
    Unknown,
}

fn direction(value: Option<&str>) -> Direction {
    match value {
        Some(value) if value.eq_ignore_ascii_case("in") => Direction::Input,
        Some(value) if value.eq_ignore_ascii_case("out") => Direction::Output,
        _ => Direction::Unknown,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Port {
    pub global_id: u32,
    pub local_id: u32,
    pub node_id: u32,
    pub channel: Option<Channel>,
    pub direction: Direction,
    pub monitor: bool,
}

impl Port {
    pub(super) fn from_global(global: &GlobalObject<&DictRef>) -> Option<Self> {
        properties!(global.props.as_ref();
            "port.id" => local_id,
            "node.id" => node_id,
            "port.direction" => port_direction,
            "audio.channel" => audio_channel,
            "port.monitor" => monitor,
        );
        Some(Self {
            global_id: global.id,
            local_id: local_id?.parse().ok()?,
            node_id: node_id?.parse().ok()?,
            channel: audio_channel.and_then(Channel::parse),
            direction: direction(port_direction),
            monitor: bool_property(monitor),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(Default))]
pub(super) enum NodeKind {
    Playback,
    Sink,
    Source,
    #[cfg_attr(test, default)]
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(Default))]
pub(super) struct Node {
    pub id: u32,
    pub serial: Option<u64>,
    pub name: Option<Arc<str>>,
    pub description: Option<Arc<str>>,
    pub media_class: Option<Arc<str>>,
    pub kind: NodeKind,
    pub virtual_node: bool,
    pub client_id: Option<u32>,
    pub identity: Option<StreamIdentity>,
    pub application_name: Option<Arc<str>>,
    pub ports: Vec<Port>,
}

impl Node {
    pub(super) fn from_global(global: &GlobalObject<&DictRef>) -> Self {
        properties!(global.props.as_ref();
            "object.serial" => serial,
            "node.name" => name,
            "node.description" => description,
            "media.name" => media_name,
            "media.class" => media_class,
            "node.virtual" => virtual_node,
            "client.id" => client_id,
            "application.id" => application_id,
            "application.name" => application_name,
        );
        let kind = classify(clean_property(media_class));
        let identity = stream_identity(
            media_class,
            application_id,
            application_name,
            media_name,
            name,
        );
        let name = arc_property(name);
        let description = arc_property(description)
            .or_else(|| arc_property(media_name))
            .or_else(|| name.clone());
        Self {
            id: global.id,
            serial: clean_property(serial).and_then(|value| value.parse().ok()),
            name,
            description,
            media_class: arc_property(media_class),
            kind,
            virtual_node: bool_property(virtual_node),
            client_id: clean_property(client_id).and_then(|value| value.parse().ok()),
            identity,
            application_name: arc_property(application_name),
            ports: Vec::new(),
        }
    }

    pub(super) fn token(&self) -> Arc<str> {
        self.name
            .as_ref()
            .or(self.description.as_ref())
            .cloned()
            .unwrap_or_else(|| Arc::from(format!("node#{}", self.id)))
    }

    pub(super) fn target_object(&self) -> Option<String> {
        self.serial
            .map(|serial| serial.to_string())
            .or_else(|| self.name.as_deref().map(str::to_owned))
    }

    pub(super) fn is_device(&self) -> bool {
        !self.virtual_node
            && self.application_name.is_none()
            && self.kind != NodeKind::Playback
            && (matches!(self.kind, NodeKind::Sink | NodeKind::Source)
                || self
                    .media_class
                    .as_deref()
                    .is_some_and(|class| contains_ascii(class, "audio"))
                || self
                    .name
                    .as_deref()
                    .is_some_and(|name| contains_ascii(name, "monitor"))
                || self
                    .description
                    .as_deref()
                    .is_some_and(|name| contains_ascii(name, "monitor")))
    }

    pub(super) fn output_ports(&self) -> Vec<&Port> {
        self.ports_in(Direction::Output, self.kind == NodeKind::Sink)
    }

    pub(super) fn input_ports(&self) -> Vec<&Port> {
        self.ports_in(Direction::Input, false)
    }

    fn ports_in(&self, direction: Direction, prefer_monitor: bool) -> Vec<&Port> {
        for monitor in [Some(prefer_monitor), None] {
            let mut ports: Vec<_> = self
                .ports
                .iter()
                .filter(|port| {
                    port.direction == direction && monitor.is_none_or(|value| port.monitor == value)
                })
                .collect();
            if !ports.is_empty() {
                ports.sort_by_key(|port| (port.local_id, port.global_id));
                return ports;
            }
        }
        Vec::new()
    }
}

fn clean_property(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn arc_property(value: Option<&str>) -> Option<Arc<str>> {
    clean_property(value).map(Arc::from)
}

fn bool_property(value: Option<&str>) -> bool {
    clean_property(value).is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

fn contains_ascii(value: &str, pattern: &str) -> bool {
    value
        .as_bytes()
        .windows(pattern.len())
        .any(|window| window.eq_ignore_ascii_case(pattern.as_bytes()))
}

fn classify(media_class: Option<&str>) -> NodeKind {
    let class = media_class.unwrap_or_default();
    if class.eq_ignore_ascii_case("Stream/Output/Audio") {
        NodeKind::Playback
    } else if contains_ascii(class, "audio/sink") {
        NodeKind::Sink
    } else if contains_ascii(class, "audio/source") {
        NodeKind::Source
    } else {
        NodeKind::Other
    }
}

fn stream_identity(
    media_class: Option<&str>,
    application_id: Option<&str>,
    application_name: Option<&str>,
    media_name: Option<&str>,
    node_name: Option<&str>,
) -> Option<StreamIdentity> {
    let media_class = clean_property(media_class)?;
    let media_class = clean_property(Some(
        media_class.strip_prefix("Stream/").unwrap_or(media_class),
    ))?;
    let (property, value) = [
        ("application.id", application_id),
        ("application.name", application_name),
        ("media.name", media_name),
        ("node.name", node_name),
    ]
    .into_iter()
    .find_map(|(property, value)| clean_property(value).map(|value| (property, value)))?;
    Some(StreamIdentity::new(format!(
        "{media_class}:{property}:{value}"
    )))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GraphLink {
    pub id: u32,
    pub output_node: u32,
    pub input_node: u32,
    pub active: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Target {
    metadata_id: u32,
    subject: u32,
    node_id: Option<u32>,
    name: Option<Arc<str>>,
}

#[derive(Debug, Default)]
pub(super) struct Graph {
    nodes: HashMap<u32, Node>,
    pending_ports: HashMap<u32, Vec<Port>>,
    port_nodes: HashMap<u32, u32>,
    links: HashMap<u32, GraphLink>,
    clients: HashSet<u32>,
    remembered_apps: HashMap<u32, HashMap<StreamIdentity, Arc<str>>>,
    default_sink: Option<Target>,
}

impl Graph {
    pub(super) fn node(&self, id: u32) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub(super) fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    pub(super) fn upsert_node(&mut self, mut node: Node) {
        if let Some(previous) = self.nodes.remove(&node.id) {
            node.ports = previous.ports;
        }
        if let Some(mut pending) = self.pending_ports.remove(&node.id) {
            node.ports.append(&mut pending);
        }
        self.remember_application(&node);
        self.nodes.insert(node.id, node);
        self.reconcile_targets();
    }

    pub(super) fn add_client(&mut self, id: u32) {
        self.clients.insert(id);
    }

    pub(super) fn upsert_port(&mut self, port: Port) {
        self.port_nodes.insert(port.global_id, port.node_id);
        let ports = self
            .nodes
            .get_mut(&port.node_id)
            .map(|node| &mut node.ports)
            .unwrap_or_else(|| self.pending_ports.entry(port.node_id).or_default());
        match ports
            .iter()
            .position(|candidate| candidate.global_id == port.global_id)
        {
            Some(index) => ports[index] = port,
            None => ports.push(port),
        }
    }

    pub(super) fn upsert_link(&mut self, link: GraphLink) {
        self.links.insert(link.id, link);
    }

    pub(super) fn remove_global(&mut self, id: u32) {
        if let Some(node_id) = self.port_nodes.remove(&id) {
            if let Some(node) = self.nodes.get_mut(&node_id) {
                node.ports.retain(|port| port.global_id != id);
            }
            if self.pending_ports.get_mut(&node_id).is_some_and(|ports| {
                ports.retain(|port| port.global_id != id);
                ports.is_empty()
            }) {
                self.pending_ports.remove(&node_id);
            }
            return;
        }
        if let Some(node) = self.nodes.remove(&id) {
            self.port_nodes.retain(|_, node_id| *node_id != id);
            self.links
                .retain(|_, link| link.output_node != id && link.input_node != id);
            self.clear_target_node(id, node.token());
            return;
        }
        if self.clients.remove(&id) {
            self.remembered_apps.remove(&id);
            return;
        }
        self.links.remove(&id);
    }

    pub(super) fn remove_metadata(&mut self, id: u32) {
        if self
            .default_sink
            .as_ref()
            .is_some_and(|target| target.metadata_id == id)
        {
            self.default_sink = None;
        }
    }

    pub(super) fn metadata(
        &mut self,
        metadata_id: u32,
        subject: u32,
        key: Option<&str>,
        type_hint: Option<&str>,
        value: Option<&str>,
    ) {
        let Some(key) = key else {
            if self.default_sink.as_ref().is_some_and(|target| {
                target.metadata_id == metadata_id && target.subject == subject
            }) {
                self.default_sink = None;
            }
            return;
        };
        if key != "default.audio.sink" {
            return;
        }
        let slot = &mut self.default_sink;
        match value {
            Some(value) => {
                *slot = Some(Target {
                    metadata_id,
                    subject,
                    node_id: (subject != 0).then_some(subject),
                    name: metadata_name(type_hint, value).map(Arc::from),
                });
            }
            None if slot.as_ref().is_some_and(|target| {
                target.metadata_id == metadata_id && target.subject == subject
            }) =>
            {
                *slot = None;
            }
            None => return,
        }
        self.reconcile_targets();
    }

    pub(super) fn default_sink(&self) -> Option<&Node> {
        self.default_sink
            .as_ref()
            .and_then(|target| self.resolve_target(target))
    }

    pub(super) fn find_device(&self, token: &str) -> Option<&Node> {
        let candidates = || self.nodes.values().filter(|node| node.is_device());
        candidates()
            .find(|node| {
                node.name
                    .as_deref()
                    .is_some_and(|name| name.eq_ignore_ascii_case(token))
            })
            .or_else(|| {
                candidates().find(|node| {
                    node.description
                        .as_deref()
                        .is_some_and(|name| name.eq_ignore_ascii_case(token))
                        || node_token_id(token) == Some(node.id)
                })
            })
    }

    pub(super) fn has_external_route(&self, node_id: u32, tap_id: Option<u32>) -> bool {
        self.links
            .values()
            .any(|link| link.output_node == node_id && Some(link.input_node) != tap_id)
    }

    fn has_active_external_route(&self, node_id: u32, tap_id: Option<u32>) -> bool {
        self.links.values().any(|link| {
            link.active && link.output_node == node_id && Some(link.input_node) != tap_id
        })
    }

    pub(super) fn view(&self, tap_id: Option<u32>, selected_device: Option<&str>) -> CaptureView {
        let mut applications: HashMap<StreamIdentity, ApplicationView> = HashMap::new();
        for client in &self.clients {
            if let Some(remembered) = self.remembered_apps.get(client) {
                for (identity, label) in remembered {
                    merge_application(
                        &mut applications,
                        identity.clone(),
                        Arc::clone(label),
                        false,
                    );
                }
            }
        }
        for node in self
            .nodes
            .values()
            .filter(|node| node.kind == NodeKind::Playback)
        {
            let Some(identity) = node.identity.clone() else {
                continue;
            };
            let label = application_label(node, &identity);
            merge_application(
                &mut applications,
                identity,
                label,
                self.has_active_external_route(node.id, tap_id),
            );
        }
        let mut applications: Vec<_> = applications.into_values().collect();
        applications.sort_by_cached_key(|application| {
            (
                application.label.to_ascii_lowercase(),
                application.identity.clone(),
            )
        });

        let mut devices: Vec<_> = self
            .nodes
            .values()
            .filter(|node| node.is_device())
            .map(Node::token)
            .collect();
        devices.sort_by_cached_key(|token| token.to_ascii_lowercase());
        devices.dedup_by(|a, b| a.eq_ignore_ascii_case(b));

        let default_sink = self.default_sink().map_or_else(
            || {
                self.default_sink
                    .as_ref()
                    .and_then(|target| target.name.clone())
                    .unwrap_or_else(|| Arc::from("(none)"))
            },
            Node::token,
        );
        CaptureView {
            revision: 0,
            applications: applications.into(),
            devices: devices.into(),
            default_sink,
            selected_device: selected_device
                .and_then(|token| self.find_device(token))
                .map(Node::token),
        }
    }

    fn remember_application(&mut self, node: &Node) {
        if node.kind != NodeKind::Playback {
            return;
        }
        let Some((client, identity)) = node.client_id.zip(node.identity.clone()) else {
            return;
        };
        let label = application_label(node, &identity);
        self.remembered_apps
            .entry(client)
            .or_default()
            .entry(identity)
            .and_modify(|current| {
                if label_precedes(&label, current) {
                    *current = Arc::clone(&label);
                }
            })
            .or_insert(label);
    }

    fn resolve_target(&self, target: &Target) -> Option<&Node> {
        target
            .node_id
            .and_then(|id| self.nodes.get(&id))
            .or_else(|| {
                target.name.as_deref().and_then(|name| {
                    self.nodes
                        .values()
                        .find(|node| node.name.as_deref() == Some(name))
                })
            })
    }

    fn reconcile_targets(&mut self) {
        let Some(target) = &mut self.default_sink else {
            return;
        };
        if target
            .node_id
            .is_some_and(|id| !self.nodes.contains_key(&id))
        {
            target.node_id = None;
        }
        if target.node_id.is_none() {
            target.node_id = target.name.as_deref().and_then(|name| {
                self.nodes
                    .iter()
                    .find(|(_, node)| node.name.as_deref() == Some(name))
                    .map(|(&id, _)| id)
            });
        }
    }

    fn clear_target_node(&mut self, id: u32, fallback: Arc<str>) {
        let Some(target) = &mut self.default_sink else {
            return;
        };
        if target.node_id == Some(id) {
            target.node_id = None;
            target.name.get_or_insert(fallback);
        }
    }
}

fn merge_application(
    applications: &mut HashMap<StreamIdentity, ApplicationView>,
    identity: StreamIdentity,
    label: Arc<str>,
    active: bool,
) {
    applications
        .entry(identity.clone())
        .and_modify(|current| {
            if (active && !current.active)
                || (active == current.active && label_precedes(&label, &current.label))
            {
                current.label = Arc::clone(&label);
            }
            current.active |= active;
        })
        .or_insert(ApplicationView {
            identity,
            label,
            active,
        });
}

fn label_precedes(candidate: &str, current: &str) -> bool {
    candidate
        .bytes()
        .map(|byte| byte.to_ascii_lowercase())
        .cmp(current.bytes().map(|byte| byte.to_ascii_lowercase()))
        .then_with(|| candidate.cmp(current))
        .is_lt()
}

fn application_label(node: &Node, identity: &StreamIdentity) -> Arc<str> {
    node.application_name
        .as_ref()
        .or(node.description.as_ref())
        .cloned()
        .unwrap_or_else(|| Arc::from(identity.as_str()))
}

fn node_token_id(token: &str) -> Option<u32> {
    token
        .get(..5)
        .filter(|prefix| prefix.eq_ignore_ascii_case("node#"))
        .and_then(|_| token.get(5..))
        .and_then(|id| id.parse().ok())
}

fn metadata_name(type_hint: Option<&str>, value: &str) -> Option<String> {
    let value = value.trim();
    let json = type_hint.is_some_and(|hint| hint.eq_ignore_ascii_case("Spa:String:JSON"))
        || value.starts_with('{');
    if !json {
        return (!value.is_empty()).then(|| value.to_owned());
    }
    match serde_json::from_str::<serde_json::Value>(value).ok()? {
        serde_json::Value::Object(map) => map.get("name")?.as_str().map(str::to_owned),
        serde_json::Value::String(value) => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playback(id: u32, client: u32) -> Node {
        Node {
            id,
            name: Some("player.node".into()),
            description: Some("Player".into()),
            media_class: Some("Stream/Output/Audio".into()),
            kind: NodeKind::Playback,
            client_id: Some(client),
            identity: Some(StreamIdentity::new(
                "Output/Audio:application.id:org.example.Player",
            )),
            application_name: Some("Player".into()),
            ..Default::default()
        }
    }

    #[test]
    fn identity_precedence_ignores_empty_properties() {
        let values = ["id", "app", "media", "node"];
        for (skip, expected) in [
            "Output/Audio:application.id:id",
            "Output/Audio:application.name:app",
            "Output/Audio:media.name:media",
            "Output/Audio:node.name:node",
        ]
        .into_iter()
        .enumerate()
        {
            let mut fields = values.map(Some);
            fields[..skip].fill(None);
            let [id, app, media, node] = fields;
            assert_eq!(
                stream_identity(Some("Stream/Output/Audio"), id, app, media, node)
                    .unwrap()
                    .as_str(),
                expected
            );
        }
        assert_eq!(
            stream_identity(
                Some("Stream/Output/Audio"),
                Some(" "),
                Some(" app "),
                None,
                None
            )
            .unwrap()
            .as_str(),
            "Output/Audio:application.name:app"
        );
    }

    #[test]
    fn graph_order_pause_and_metadata_are_stale_safe() {
        let mut graph = Graph::default();
        graph.upsert_port(Port {
            global_id: 100,
            local_id: 0,
            node_id: 10,
            direction: Direction::Output,
            ..Default::default()
        });
        graph.add_client(5);
        graph.upsert_node(playback(10, 5));
        assert_eq!(graph.node(10).unwrap().ports[0].global_id, 100);
        graph.remove_global(10);
        assert_eq!(graph.view(None, None).applications.len(), 1);
        graph.remove_global(5);
        assert!(graph.view(None, None).applications.is_empty());

        graph.metadata(1, 0, Some("default.audio.sink"), None, Some("sink.old"));
        graph.metadata(
            2,
            0,
            Some("default.audio.sink"),
            Some("Spa:String:JSON"),
            Some(r#"{"name":"sink.current"}"#),
        );
        graph.metadata(1, 0, Some("default.audio.sink"), None, None);
        assert_eq!(graph.view(None, None).default_sink.as_ref(), "sink.current");
    }
}
