// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::MAX_CAPTURE_CHANNELS;
use super::graph::{CaptureLayout, Channel, Graph, Node, NodeKind, Port};
use super::stream::StreamConfig;
use crate::domain::routing::{CaptureConfig, CaptureMode, DeviceSelection};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct LinkSpec {
    pub output_node: u32,
    pub output_port: u32,
    pub input_node: u32,
    pub input_port: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Plan {
    pub stream: StreamConfig,
    pub sources: Vec<u32>,
    pub truncated: usize,
}

pub(super) fn plan(graph: &Graph, config: &CaptureConfig, tap_id: Option<u32>) -> Plan {
    match config.mode {
        CaptureMode::Applications => {
            let mut sources: Vec<_> = graph
                .nodes()
                .filter(|node| node.kind == NodeKind::Playback)
                .filter(|node| graph.has_external_route(node.id, tap_id))
                .filter(|node| {
                    node.identity
                        .as_ref()
                        .is_some_and(|identity| !config.disabled_streams.contains(identity))
                })
                .collect();
            sources.sort_by_key(|node| node.id);
            application_plan(sources)
        }
        CaptureMode::Device => {
            let device = match &config.device {
                DeviceSelection::Default => graph.default_sink(),
                DeviceSelection::Device(token) => graph.find_device(token),
            };
            device.map_or_else(idle_plan, device_plan)
        }
    }
}

fn idle_plan() -> Plan {
    Plan {
        stream: StreamConfig::idle(),
        sources: Vec::new(),
        truncated: 0,
    }
}

fn application_plan(sources: Vec<&Node>) -> Plan {
    let truncated = sources
        .iter()
        .map(|source| {
            source
                .output_ports()
                .len()
                .saturating_sub(MAX_CAPTURE_CHANNELS)
        })
        .sum();
    Plan {
        stream: StreamConfig {
            layout: CaptureLayout::surround(),
            target: None,
            passive: true,
        },
        sources: sources.into_iter().map(|node| node.id).collect(),
        truncated,
    }
}

fn device_plan(device: &Node) -> Plan {
    let (layout, truncated) = capture_layout(device);
    if matches!(device.kind, NodeKind::Sink | NodeKind::Source)
        && let Some(object) = device.target_object()
    {
        return Plan {
            stream: StreamConfig {
                layout,
                target: Some(object),
                passive: device.kind == NodeKind::Sink,
            },
            sources: Vec::new(),
            truncated,
        };
    }

    let passive = device.output_ports().iter().all(|port| port.monitor);
    Plan {
        stream: StreamConfig {
            layout,
            target: None,
            passive,
        },
        sources: vec![device.id],
        truncated,
    }
}

fn port_layout(ports: &[&Port]) -> ([Channel; MAX_CAPTURE_CHANNELS], usize) {
    let channels = ports.len().min(MAX_CAPTURE_CHANNELS);
    let mut positions = [Channel::Unknown; MAX_CAPTURE_CHANNELS];
    for (position, port) in positions.iter_mut().zip(ports).take(channels) {
        *position = port.channel.unwrap_or(Channel::Unknown);
    }
    (
        Channel::normalize(channels, positions),
        ports.len().saturating_sub(channels),
    )
}

fn capture_layout(source: &Node) -> (CaptureLayout, usize) {
    let ports = source.output_ports();
    if ports.is_empty() {
        return (CaptureLayout::stereo(), 0);
    }
    let (positions, truncated) = port_layout(&ports);
    let layout = CaptureLayout {
        channels: positions
            .into_iter()
            .take(ports.len().min(MAX_CAPTURE_CHANNELS))
            .collect(),
    };
    (layout, truncated)
}

pub(super) fn desired_links(graph: &Graph, plan: &Plan, tap: &Node) -> Vec<LinkSpec> {
    if plan.sources.is_empty() {
        return Vec::new();
    }
    let tap_ports = tap.input_ports();
    let mut claimed = HashSet::new();
    let targets: Vec<_> = plan
        .stream
        .layout
        .channels
        .iter()
        .enumerate()
        .map(|(ordinal, channel)| {
            tap_ports
                .iter()
                .copied()
                .find(|port| port.channel == Some(*channel) && claimed.insert(port.global_id))
                .or_else(|| {
                    tap_ports
                        .get(ordinal)
                        .copied()
                        .filter(|port| claimed.insert(port.global_id))
                })
                .or_else(|| {
                    tap_ports
                        .iter()
                        .copied()
                        .find(|port| claimed.insert(port.global_id))
                })
        })
        .collect();
    if targets.iter().any(Option::is_none) {
        return Vec::new();
    }

    let target_for = |channel: Channel| {
        plan.stream
            .layout
            .channels
            .iter()
            .position(|candidate| *candidate == channel)
            .and_then(|index| targets[index])
    };

    let mut links = HashSet::new();
    for source in plan.sources.iter().filter_map(|id| graph.node(*id)) {
        let ports = source.output_ports();
        let (positions, _) = port_layout(&ports);
        let aux_channels = positions
            .iter()
            .filter_map(|channel| match channel {
                Channel::Aux(index) => Some(*index as usize + 1),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let aux_layout = Channel::fallback(aux_channels);
        let aux_target = |index: u8| {
            aux_layout
                .get(index as usize)
                .copied()
                .map(|channel| match channel {
                    Channel::Mono => Channel::FrontLeft,
                    channel => channel,
                })
        };
        for (output, channel) in ports.into_iter().zip(positions).take(MAX_CAPTURE_CHANNELS) {
            if let Some(target) = target_for(channel) {
                links.insert(link(source, output, tap, target));
                continue;
            }
            let remixed = match channel {
                Channel::Mono => [Some(Channel::FrontLeft), Some(Channel::FrontRight)],
                Channel::Aux(index) => [aux_target(index), None],
                _ => [None; 2],
            };
            for target in remixed.into_iter().flatten().filter_map(target_for) {
                links.insert(link(source, output, tap, target));
            }
        }
    }
    let mut links: Vec<_> = links.into_iter().collect();
    links.sort_by_key(|link| (link.output_node, link.output_port, link.input_port));
    links
}

fn link(source: &Node, output: &Port, tap: &Node, input: &Port) -> LinkSpec {
    LinkSpec {
        output_node: source.id,
        output_port: output.global_id,
        input_node: tap.id,
        input_port: input.global_id,
    }
}
