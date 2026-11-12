// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::{ApplicationView, CaptureView};
use crate::domain::routing::StreamIdentity;
use crate::dsp::{ChannelPosition, MAX_AUDIO_CHANNELS};
use pipewire as pw;
use pw::registry::GlobalObject;
use pw::spa::utils::{Direction, dict::DictRef};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

const NODE_TOKEN_PREFIX: &str = "node#";

macro_rules! channel_positions {
    ($($name:literal => $variant:ident => $id:path),* $(,)?) => {
        impl ChannelPosition {
            fn parse(value: &str) -> Option<Self> {
                let value = value.trim();
                [$(($name, Self::$variant)),*]
                    .into_iter()
                    .find_map(|(name, channel)| value.eq_ignore_ascii_case(name).then_some(channel))
                    .or_else(|| parse_after(value, "AUX").and_then(Self::aux))
            }

            fn aux(index: u8) -> Option<Self> {
                (usize::from(index) < MAX_AUDIO_CHANNELS).then_some(Self::Aux(index))
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
                    id => id
                        .checked_sub(pw::spa::sys::SPA_AUDIO_CHANNEL_AUX0)
                        .and_then(|index| Self::aux(u8::try_from(index).ok()?))
                        .unwrap_or(Self::Unknown),
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
    "FLC" => FrontLeftCenter => pw::spa::sys::SPA_AUDIO_CHANNEL_FLC,
    "FRC" => FrontRightCenter => pw::spa::sys::SPA_AUDIO_CHANNEL_FRC,
    "RC" => RearCenter => pw::spa::sys::SPA_AUDIO_CHANNEL_RC,
    "MONO" => Mono => pw::spa::sys::SPA_AUDIO_CHANNEL_MONO,
}

fn direction(value: Option<&str>) -> Option<Direction> {
    match value {
        Some(value) if value.eq_ignore_ascii_case("in") => Some(Direction::Input),
        Some(value) if value.eq_ignore_ascii_case("out") => Some(Direction::Output),
        _ => None,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Port {
    pub global_id: u32,
    pub local_id: u32,
    pub node_id: u32,
    pub channel: ChannelPosition,
    pub direction: Option<Direction>,
    pub monitor: bool,
}

impl Port {
    pub(super) fn from_global(global: &GlobalObject<&DictRef>) -> Option<Self> {
        Some(Self {
            global_id: global.id,
            local_id: parsed_property(global, "port.id")?,
            node_id: parsed_property(global, "node.id")?,
            channel: property(global, "audio.channel")
                .and_then(ChannelPosition::parse)
                .unwrap_or_default(),
            direction: direction(property(global, "port.direction")),
            monitor: bool_property(property(global, "port.monitor")),
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

#[cfg_attr(test, derive(Default))]
pub(super) struct Node {
    pub id: u32,
    pub serial: Option<u64>,
    pub name: Option<Arc<str>>,
    pub description: Option<Arc<str>>,
    pub kind: NodeKind,
    pub device: bool,
    pub client_id: Option<u32>,
    pub identity: Option<StreamIdentity>,
    pub application_name: Option<Arc<str>>,
}

impl Node {
    pub(super) fn from_global(global: &GlobalObject<&DictRef>) -> Self {
        let name = property(global, "node.name");
        let media_class = property(global, "media.class");
        let application_name = property(global, "application.name");
        let kind = classify(clean_property(media_class));
        let identity = stream_identity(media_class, application_name, name);
        let name = arc_property(name);
        let description =
            arc_property(property(global, "node.description")).or_else(|| name.clone());
        let application_name = arc_property(application_name);
        let device = application_name.is_none()
            && kind != NodeKind::Playback
            && (matches!(kind, NodeKind::Sink | NodeKind::Source)
                || contains_ascii(media_class.unwrap_or_default(), "audio")
                || [name.as_deref(), description.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|name| contains_ascii(name, "monitor")));
        Self {
            id: global.id,
            serial: parsed_property(global, "object.serial"),
            name,
            description,
            kind,
            device,
            client_id: parsed_property(global, "client.id"),
            identity,
            application_name,
        }
    }

    pub(super) fn token(&self) -> Arc<str> {
        self.name
            .as_ref()
            .or(self.description.as_ref())
            .cloned()
            .unwrap_or_else(|| Arc::from(format!("{NODE_TOKEN_PREFIX}{}", self.id)))
    }

    pub(super) fn application(&self) -> Option<ApplicationView> {
        let identity = self.identity.as_ref()?;
        (self.kind == NodeKind::Playback).then(|| ApplicationView {
            identity: identity.clone(),
            label: self
                .application_name
                .as_ref()
                .or(self.description.as_ref())
                .unwrap_or(&identity.0)
                .clone(),
            active: false,
        })
    }
}

pub(super) fn property<'a>(global: &GlobalObject<&'a DictRef>, key: &str) -> Option<&'a str> {
    global.props?.get(key)
}

fn clean_property(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn parsed_property<T: FromStr>(global: &GlobalObject<&DictRef>, key: &str) -> Option<T> {
    clean_property(property(global, key))?.parse().ok()
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
    application_name: Option<&str>,
    node_name: Option<&str>,
) -> Option<StreamIdentity> {
    let media_class = clean_property(media_class)?;
    let media_class = clean_property(Some(
        media_class.strip_prefix("Stream/").unwrap_or(media_class),
    ))?;
    let (property, value) = match clean_property(application_name) {
        Some(name) => ("application.name", name),
        None => ("node.name", clean_property(node_name)?),
    };
    Some(StreamIdentity(Arc::from(format!(
        "{media_class}:{property}:{value}"
    ))))
}

pub(super) struct GraphLink {
    pub output_node: u32,
    pub input_node: u32,
    pub active: bool,
}

struct Target {
    metadata_id: u32,
    subject: u32,
    node_id: Option<u32>,
    name: Option<Arc<str>>,
}

#[derive(Default)]
pub(super) struct Graph {
    nodes: HashMap<u32, Node>,
    ports: HashMap<u32, Port>,
    links: HashMap<u32, GraphLink>,
    clients: HashSet<u32>,
    remembered_apps: HashMap<u32, HashMap<StreamIdentity, ApplicationView>>,
    default_sink: Option<Target>,
}

impl Graph {
    pub(super) fn node(&self, id: u32) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub(super) fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    pub(super) fn output_ports(&self, node: &Node) -> Vec<&Port> {
        self.ports_in(node.id, Direction::Output, node.kind == NodeKind::Sink)
    }

    pub(super) fn input_ports(&self, node: &Node) -> Vec<&Port> {
        self.ports_in(node.id, Direction::Input, false)
    }

    fn ports_in(&self, node: u32, direction: Direction, prefer_monitor: bool) -> Vec<&Port> {
        let mut ports: Vec<_> = self
            .ports
            .values()
            .filter(|port| port.node_id == node && port.direction == Some(direction))
            .collect();
        if ports.iter().any(|port| port.monitor == prefer_monitor) {
            ports.retain(|port| port.monitor == prefer_monitor);
        }
        ports.sort_by_key(|port| (port.local_id, port.global_id));
        ports
    }

    pub(super) fn upsert_node(&mut self, node: Node) {
        self.remember_application(&node);
        self.nodes.insert(node.id, node);
        self.reconcile_targets();
    }

    pub(super) fn add_client(&mut self, id: u32) {
        self.clients.insert(id);
    }

    pub(super) fn upsert_port(&mut self, port: Port) {
        self.ports.insert(port.global_id, port);
    }

    pub(super) fn upsert_link(&mut self, id: u32, link: GraphLink) {
        self.links.insert(id, link);
    }

    pub(super) fn remove_global(&mut self, id: u32) {
        let _ = self.default_sink.take_if(|target| target.metadata_id == id);
        if self.ports.remove(&id).is_some() {
            return;
        }
        if let Some(node) = self.nodes.remove(&id) {
            self.ports.retain(|_, port| port.node_id != id);
            self.links
                .retain(|_, link| link.output_node != id && link.input_node != id);
            if let Some(target) = &mut self.default_sink
                && target.node_id == Some(id)
            {
                target.name.get_or_insert_with(|| node.token());
            }
            self.reconcile_targets();
            return;
        }
        if self.clients.remove(&id) {
            self.remembered_apps.remove(&id);
            return;
        }
        self.links.remove(&id);
    }

    pub(super) fn metadata(
        &mut self,
        metadata_id: u32,
        subject: u32,
        key: Option<&str>,
        type_hint: Option<&str>,
        value: Option<&str>,
    ) {
        let same_entry =
            |target: &mut Target| target.metadata_id == metadata_id && target.subject == subject;
        let Some(key) = key else {
            let _ = self.default_sink.take_if(same_entry);
            return;
        };
        if key != "default.audio.sink" {
            return;
        }
        if let Some(value) = value {
            self.default_sink = Some(Target {
                metadata_id,
                subject,
                node_id: (subject != 0).then_some(subject),
                name: metadata_name(type_hint, value).map(Arc::from),
            });
        } else if self.default_sink.take_if(same_entry).is_none() {
            return;
        }
        self.reconcile_targets();
    }

    pub(super) fn default_sink(&self) -> Option<&Node> {
        self.nodes.get(&self.default_sink.as_ref()?.node_id?)
    }

    pub(super) fn find_device(&self, token: &str) -> Option<&Node> {
        let candidates = || self.nodes.values().filter(|node| node.device);
        let named = |name: Option<&str>| name.is_some_and(|name| name.eq_ignore_ascii_case(token));
        let token_id = parse_after(token, NODE_TOKEN_PREFIX);
        candidates()
            .find(|node| named(node.name.as_deref()))
            .or_else(|| {
                candidates()
                    .find(|node| named(node.description.as_deref()) || token_id == Some(node.id))
            })
    }

    pub(super) fn external_routes(
        &self,
        node_id: u32,
        tap_id: Option<u32>,
    ) -> impl Iterator<Item = &GraphLink> {
        self.links
            .values()
            .filter(move |link| link.output_node == node_id && Some(link.input_node) != tap_id)
    }

    pub(super) fn view(&self, tap_id: Option<u32>, selected_device: Option<&str>) -> CaptureView {
        let mut applications: HashMap<StreamIdentity, ApplicationView> = HashMap::new();
        let remembered = self
            .clients
            .iter()
            .filter_map(|client| self.remembered_apps.get(client))
            .flat_map(HashMap::values)
            .cloned();
        let live = self.nodes.values().filter_map(|node| {
            let application = node.application()?;
            Some(ApplicationView {
                active: self
                    .external_routes(node.id, tap_id)
                    .any(|link| link.active),
                ..application
            })
        });
        for application in remembered.chain(live) {
            merge_application(&mut applications, application);
        }
        let mut applications: Vec<_> = applications.into_values().collect();
        applications.sort_by(|a, b| {
            caseless_cmp(&a.label, &b.label).then_with(|| a.identity.cmp(&b.identity))
        });

        let mut devices: Vec<_> = self
            .nodes
            .values()
            .filter(|node| node.device)
            .map(Node::token)
            .collect();
        devices.sort_by(|a, b| caseless_cmp(a, b));
        devices.dedup_by(|a, b| a.eq_ignore_ascii_case(b));

        let default_sink = self
            .default_sink()
            .map(Node::token)
            .or_else(|| self.default_sink.as_ref()?.name.clone())
            .unwrap_or_else(|| Arc::from("(none)"));
        CaptureView {
            applications: applications.into(),
            devices: devices.into(),
            default_sink,
            selected_device: selected_device
                .and_then(|token| self.find_device(token))
                .map(Node::token),
        }
    }

    fn remember_application(&mut self, node: &Node) {
        if let Some((client, application)) = node.client_id.zip(node.application()) {
            merge_application(self.remembered_apps.entry(client).or_default(), application);
        }
    }

    fn reconcile_targets(&mut self) {
        let Some(target) = &mut self.default_sink else {
            return;
        };
        let _ = target.node_id.take_if(|id| !self.nodes.contains_key(id));
        if target.node_id.is_none() {
            target.node_id = target.name.as_deref().and_then(|name| {
                self.nodes
                    .iter()
                    .find(|(_, node)| node.name.as_deref() == Some(name))
                    .map(|(&id, _)| id)
            });
        }
    }
}

fn merge_application(
    applications: &mut HashMap<StreamIdentity, ApplicationView>,
    application: ApplicationView,
) {
    let ApplicationView { label, active, .. } = &application;
    applications
        .entry(application.identity.clone())
        .and_modify(|current| {
            if (*active && !current.active)
                || (*active == current.active && label_precedes(label, &current.label))
            {
                current.label = Arc::clone(label);
            }
            current.active |= active;
        })
        .or_insert(application);
}

fn caseless_cmp(a: &str, b: &str) -> Ordering {
    let lower = |byte: u8| byte.to_ascii_lowercase();
    a.bytes().map(lower).cmp(b.bytes().map(lower))
}

fn label_precedes(candidate: &str, current: &str) -> bool {
    caseless_cmp(candidate, current)
        .then_with(|| candidate.cmp(current))
        .is_lt()
}

fn parse_after<T: FromStr>(value: &str, prefix: &str) -> Option<T> {
    let (head, tail) = value.split_at_checked(prefix.len())?;
    let tail = head.eq_ignore_ascii_case(prefix).then_some(tail)?;
    tail.parse().ok()
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
            kind: NodeKind::Playback,
            client_id: Some(client),
            identity: Some(StreamIdentity(
                "Output/Audio:application.id:org.example.Player".into(),
            )),
            application_name: Some("Player".into()),
            ..Default::default()
        }
    }

    #[test]
    fn identity_precedence_ignores_empty_properties() {
        let identity = |app, node| stream_identity(Some("Stream/Output/Audio"), app, node);
        assert_eq!(
            identity(Some(" app "), Some("node")).unwrap(),
            StreamIdentity("Output/Audio:application.name:app".into())
        );
        assert_eq!(
            identity(Some(" "), Some("node")).unwrap(),
            StreamIdentity("Output/Audio:node.name:node".into())
        );
    }

    #[test]
    fn graph_order_pause_and_metadata_are_stale_safe() {
        let mut graph = Graph::default();
        graph.upsert_port(Port {
            global_id: 100,
            local_id: 0,
            node_id: 10,
            direction: Some(Direction::Output),
            ..Default::default()
        });
        graph.add_client(5);
        graph.upsert_node(playback(10, 5));
        assert_eq!(
            graph.output_ports(graph.node(10).unwrap())[0].global_id,
            100
        );
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

        for id in [20, 21] {
            graph.upsert_node(Node {
                id,
                name: Some("sink.current".into()),
                ..Default::default()
            });
        }
        assert_eq!(graph.default_sink().map(|node| node.id), Some(20));
        graph.remove_global(20);
        assert_eq!(graph.default_sink().map(|node| node.id), Some(21));
    }
}
