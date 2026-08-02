// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::MAX_CAPTURE_CHANNELS;
use super::graph::{Channel, Graph, Node, NodeKind, Port, stereo_layout};
use super::stream::StreamConfig;
use crate::domain::routing::{CaptureConfig, CaptureMode};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct LinkSpec {
    pub output_node: u32,
    pub output_port: u32,
    pub input_node: u32,
    pub input_port: u32,
}

pub(super) struct Plan {
    pub stream: StreamConfig,
    pub sources: Vec<u32>,
    pub truncated: usize,
}

pub(super) fn plan(graph: &Graph, config: &CaptureConfig, tap_id: Option<u32>) -> Plan {
    let (layout, target, passive, sources, truncated) = match config.mode {
        CaptureMode::Applications => {
            let mut sources: Vec<_> = graph
                .nodes()
                .filter(|node| node.kind == NodeKind::Playback)
                .filter(|node| graph.external_routes(node.id, tap_id).next().is_some())
                .filter(|node| {
                    node.identity
                        .as_ref()
                        .is_some_and(|identity| !config.disabled_streams.contains(identity))
                })
                .collect();
            sources.sort_by_key(|node| node.id);
            let truncated = sources
                .iter()
                .map(|source| {
                    graph
                        .output_ports(source)
                        .len()
                        .saturating_sub(MAX_CAPTURE_CHANNELS)
                })
                .sum();
            (
                Channel::SURROUND.into(),
                None,
                true,
                sources.into_iter().map(|node| node.id).collect(),
                truncated,
            )
        }
        CaptureMode::Device => {
            let device = config
                .device
                .as_deref()
                .map_or_else(|| graph.default_sink(), |token| graph.find_device(token));
            if let Some(device) = device {
                let ports = graph.output_ports(device);
                let (layout, truncated) = if ports.is_empty() {
                    (stereo_layout(), 0)
                } else {
                    let (positions, truncated) = port_layout(&ports);
                    (
                        positions[..ports.len().min(MAX_CAPTURE_CHANNELS)].to_vec(),
                        truncated,
                    )
                };
                let target = matches!(device.kind, NodeKind::Sink | NodeKind::Source)
                    .then(|| device.target_object())
                    .flatten();
                let (passive, sources) = if target.is_some() {
                    (device.kind == NodeKind::Sink, Vec::new())
                } else {
                    (ports.iter().all(|port| port.monitor), vec![device.id])
                };
                (layout, target, passive, sources, truncated)
            } else {
                (stereo_layout(), None, true, Vec::new(), 0)
            }
        }
    };
    Plan {
        stream: StreamConfig {
            layout,
            target,
            passive,
        },
        sources,
        truncated,
    }
}

fn port_layout(ports: &[&Port]) -> ([Channel; MAX_CAPTURE_CHANNELS], usize) {
    let channels = ports.len().min(MAX_CAPTURE_CHANNELS);
    let mut positions = [Channel::Unknown; MAX_CAPTURE_CHANNELS];
    for (position, port) in positions.iter_mut().zip(ports) {
        *position = port.channel.unwrap_or_default();
    }
    (
        Channel::normalize(channels, positions),
        ports.len().saturating_sub(channels),
    )
}

pub(super) fn desired_links(graph: &Graph, plan: &Plan, tap: &Node) -> Vec<LinkSpec> {
    if plan.sources.is_empty() {
        return Vec::new();
    }
    let tap_ports = graph.input_ports(tap);
    let mut claimed = HashSet::new();
    let targets: Vec<_> = plan
        .stream
        .layout
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
            .iter()
            .position(|candidate| *candidate == channel)
            .and_then(|index| targets[index])
    };

    let mut links = HashSet::new();
    for source in plan.sources.iter().filter_map(|id| graph.node(*id)) {
        let ports = graph.output_ports(source);
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
    links.sort_unstable();
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
