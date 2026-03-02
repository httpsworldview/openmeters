use pipewire as pw;
use pw::registry::GlobalObject;
use pw::spa::utils::dict::DictRef;
use std::collections::HashMap;

pub(crate) fn dict_to_map(dict: Option<&DictRef>) -> HashMap<String, String> {
    dict.into_iter()
        .flat_map(|d| d.iter())
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PortDirection {
    Input,
    Output,
    #[default]
    Unknown,
}

impl PortDirection {
    fn from_str(value: &str) -> Self {
        match value {
            s if s.eq_ignore_ascii_case("in") => Self::Input,
            s if s.eq_ignore_ascii_case("out") => Self::Output,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphPort {
    pub global_id: u32,
    pub port_id: u32,
    pub node_id: u32,
    pub channel: Option<String>,
    pub direction: PortDirection,
    pub is_monitor: bool,
}

impl GraphPort {
    pub(crate) fn from_global(global: &GlobalObject<&DictRef>) -> Option<Self> {
        let props = dict_to_map(global.props.as_ref().copied());
        let get = |primary, fallback| props.get(primary).or_else(|| props.get(fallback));
        let parse = |p, f| get(p, f).and_then(|v| v.parse().ok());

        Some(Self {
            global_id: global.id,
            port_id: parse(*pw::keys::PORT_ID, "port.id")?,
            node_id: parse(*pw::keys::NODE_ID, "node.id")?,
            direction: get(*pw::keys::PORT_DIRECTION, "port.direction")
                .map(|d| PortDirection::from_str(d))
                .unwrap_or_default(),
            channel: get(*pw::keys::AUDIO_CHANNEL, "audio.channel").cloned(),
            is_monitor: get(*pw::keys::PORT_MONITOR, "port.monitor")
                .is_some_and(|v| v.eq_ignore_ascii_case("true") || v == "1"),
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NodeDirection {
    Input,
    Output,
    #[default]
    Unknown,
}

fn derive_node_direction(
    media_class: Option<&str>,
    props: &HashMap<String, String>,
) -> NodeDirection {
    let class = media_class.map(|c| c.to_ascii_lowercase());
    let has = |needle| class.as_ref().is_some_and(|c| c.contains(needle));

    if has("sink") || has("output") {
        return NodeDirection::Output;
    }
    if has("source") || has("input") {
        return NodeDirection::Input;
    }
    match props.get(*pw::keys::PORT_DIRECTION).map(String::as_str) {
        Some(s) if s.eq_ignore_ascii_case("in") => NodeDirection::Input,
        Some(s) if s.eq_ignore_ascii_case("out") => NodeDirection::Output,
        _ => NodeDirection::Unknown,
    }
}

pub(crate) const DEFAULT_AUDIO_SINK_KEY: &str = "default.audio.sink";
pub(crate) const DEFAULT_AUDIO_SOURCE_KEY: &str = "default.audio.source";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DefaultTarget {
    pub metadata_id: Option<u32>,
    pub node_id: Option<u32>,
    pub name: Option<String>,
    pub type_hint: Option<String>,
}

impl DefaultTarget {
    pub(crate) fn update(
        &mut self,
        metadata_id: u32,
        subject: u32,
        type_hint: Option<&str>,
        name: Option<&str>,
    ) -> bool {
        let new = Self {
            metadata_id: Some(metadata_id),
            node_id: (subject != 0).then_some(subject),
            type_hint: type_hint.map(str::to_string),
            name: name.map(str::to_string),
        };
        let changed = *self != new;
        if changed {
            *self = new;
        }
        changed
    }
}

pub(crate) fn parse_metadata_name(type_hint: Option<&str>, value: Option<&str>) -> Option<String> {
    use serde_json::Value;
    let trimmed = value?.trim();
    let is_json = matches!(type_hint, Some(h) if h.eq_ignore_ascii_case("Spa:String:JSON"))
        || trimmed.starts_with('{');
    match (is_json, serde_json::from_str::<Value>(trimmed)) {
        (true, Ok(Value::Object(map))) => {
            map.get("name").and_then(|n| n.as_str()).map(str::to_string)
        }
        (true, Ok(Value::String(s))) => Some(s),
        (true, _) => None,
        (false, _) if trimmed.is_empty() => None,
        (false, _) => Some(trimmed.to_string()),
    }
}

pub(crate) fn format_target_metadata(
    object_serial: Option<&str>,
    node_id: u32,
) -> (String, String) {
    let target_object = object_serial
        .and_then(|raw| {
            let trimmed = raw.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .unwrap_or_else(|| node_id.to_string());
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
        target_object: String,
        target_node: String,
    },
    ResetRoute {
        subject: u32,
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
            .map_or_else(|| raw.to_string(), |n| n.display_name());
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

    pub fn route_candidates(&self, sink: &NodeInfo) -> impl Iterator<Item = &NodeInfo> {
        self.nodes.iter().filter(|n| n.should_route_to(sink))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeInfo {
    pub id: u32,
    pub name: Option<String>,
    pub description: Option<String>,
    pub media_class: Option<String>,
    pub direction: NodeDirection,
    pub is_virtual: bool,
    pub properties: HashMap<String, String>,
    pub ports: Vec<GraphPort>,
}

impl NodeInfo {
    pub(crate) fn from_global(global: &GlobalObject<&DictRef>) -> Self {
        let props = dict_to_map(global.props.as_ref().copied());
        let name = props.get(*pw::keys::NODE_NAME).cloned();
        let description = props
            .get(*pw::keys::NODE_DESCRIPTION)
            .or_else(|| props.get("media.name"))
            .or(name.as_ref())
            .cloned();
        let media_class = props.get(*pw::keys::MEDIA_CLASS).cloned();
        let is_virtual = props
            .get("node.virtual")
            .map(|v| v == "true")
            .unwrap_or_else(|| name.as_deref() == Some("openmeters.sink"));

        Self {
            id: global.id,
            direction: derive_node_direction(media_class.as_deref(), &props),
            name,
            description,
            media_class,
            is_virtual,
            properties: props,
            ports: Vec::new(),
        }
    }

    pub fn display_name(&self) -> String {
        self.name
            .as_ref()
            .or(self.description.as_ref())
            .cloned()
            .unwrap_or_else(|| format!("node#{}", self.id))
    }

    pub fn app_name(&self) -> Option<&str> {
        self.properties.get(*pw::keys::APP_NAME).map(String::as_str)
    }

    pub fn object_serial(&self) -> Option<&str> {
        self.properties.get("object.serial").map(String::as_str)
    }

    pub fn matches_label(&self, label: &str) -> bool {
        [&self.name, &self.description].iter().any(|opt| {
            opt.as_deref()
                .is_some_and(|v| v.eq_ignore_ascii_case(label))
        })
    }

    pub fn should_route_to(&self, sink: &Self) -> bool {
        self.id != sink.id && self.is_audio_application_output()
    }

    fn is_audio_application_output(&self) -> bool {
        self.direction == NodeDirection::Output
            && self
                .media_class
                .as_deref()
                .is_some_and(|c| c.to_ascii_lowercase().contains("audio"))
            && self.app_name().is_some()
    }

    pub fn output_ports_for_loopback(&self) -> Vec<GraphPort> {
        self.ports_for_loopback(PortDirection::Output, true)
    }

    pub fn input_ports_for_loopback(&self) -> Vec<GraphPort> {
        self.ports_for_loopback(PortDirection::Input, false)
    }

    fn ports_for_loopback(&self, dir: PortDirection, prefer_monitor: bool) -> Vec<GraphPort> {
        let primary: Vec<_> = self
            .ports
            .iter()
            .filter(|p| p.direction == dir && p.is_monitor == prefer_monitor)
            .cloned()
            .collect();
        if !primary.is_empty() {
            return primary;
        }
        let secondary: Vec<_> = self
            .ports
            .iter()
            .filter(|p| p.direction == dir)
            .cloned()
            .collect();
        if !secondary.is_empty() {
            return secondary;
        }
        self.ports.clone()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetadataDefaults {
    pub audio_sink: Option<DefaultTarget>,
    pub audio_source: Option<DefaultTarget>,
}

impl MetadataDefaults {
    pub(crate) fn apply_update(
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
                let inserted = slot.is_none();
                let parsed_name = parse_metadata_name(type_hint, Some(val));
                let name_ref = parsed_name.as_deref().or(Some(val));

                let target = slot.get_or_insert_with(DefaultTarget::default);
                let updated = target.update(metadata_id, subject, type_hint, name_ref);
                inserted || updated
            }
            None => slot
                .as_ref()
                .is_some_and(|t| t.metadata_id == Some(metadata_id))
                .then(|| *slot = None)
                .is_some(),
        }
    }

    pub(crate) fn reconcile_with_nodes(&mut self, nodes: &HashMap<u32, NodeInfo>) {
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

    pub(crate) fn clear_metadata(&mut self, metadata_id: u32) -> bool {
        self.clear_slots(|t| t.metadata_id == Some(metadata_id), |_| {})
    }

    pub(crate) fn clear_node(&mut self, node_id: u32, fallback_name: Option<String>) -> bool {
        self.clear_slots(
            |t| t.node_id == Some(node_id),
            |t| {
                t.node_id = None;
                if t.name.is_none() {
                    t.name = fallback_name.clone();
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
