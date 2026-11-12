// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use super::graph::{Graph, Node, NodeKind, Port};
use super::stream::StreamConfig;
use crate::domain::routing::{CaptureConfig, CaptureMode};
use crate::dsp::{ChannelLayout, ChannelPosition, MAX_AUDIO_CHANNELS};
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
                .map(|source| graph.output_ports(source).len())
                .map(|ports| ports.saturating_sub(MAX_AUDIO_CHANNELS))
                .sum();
            (
                ChannelLayout::SURROUND,
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
                let truncated = ports.len().saturating_sub(MAX_AUDIO_CHANNELS);
                let layout = port_layout(&ports).unwrap_or(ChannelLayout::STEREO);
                let target = device
                    .serial
                    .filter(|_| matches!(device.kind, NodeKind::Sink | NodeKind::Source));
                let (passive, sources) = if target.is_some() {
                    (device.kind == NodeKind::Sink, Vec::new())
                } else {
                    (ports.iter().all(|port| port.monitor), vec![device.id])
                };
                (layout, target, passive, sources, truncated)
            } else {
                (ChannelLayout::STEREO, None, true, Vec::new(), 0)
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

fn port_layout(ports: &[&Port]) -> Option<ChannelLayout> {
    let mut positions = [ChannelPosition::Unknown; MAX_AUDIO_CHANNELS];
    for (position, port) in positions.iter_mut().zip(ports) {
        *position = port.channel;
    }
    ChannelLayout::new(ports.len().min(MAX_AUDIO_CHANNELS), positions)
}

pub(super) fn desired_links(graph: &Graph, plan: &Plan, tap: &Node) -> Vec<LinkSpec> {
    if plan.sources.is_empty() {
        return Vec::new();
    }
    let tap_ports = graph.input_ports(tap);
    let mut claimed = HashSet::new();
    let tap_layout = plan.stream.layout.positions();
    let Some(targets) = tap_layout
        .iter()
        .enumerate()
        .map(|(ordinal, channel)| {
            tap_ports
                .iter()
                .copied()
                .find(|port| port.channel == *channel && claimed.insert(port.global_id))
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
        .collect::<Option<Vec<_>>>()
    else {
        return Vec::new();
    };

    let target_for = |channel: ChannelPosition| {
        tap_layout
            .iter()
            .position(|candidate| *candidate == channel)
            .map(|index| targets[index])
    };

    let mut links = Vec::new();
    for source in plan.sources.iter().filter_map(|id| graph.node(*id)) {
        let ports = graph.output_ports(source);
        let Some(layout) = port_layout(&ports) else {
            continue;
        };
        // Preserve exact port identities before inferring AUX speaker roles.
        if layout == plan.stream.layout {
            links.extend(
                ports
                    .into_iter()
                    .zip(&targets)
                    .map(|(output, input)| link(source, output, tap, input)),
            );
            continue;
        }
        let mut positions = [ChannelPosition::Unknown; MAX_AUDIO_CHANNELS];
        let positions = &mut positions[..layout.positions().len()];
        positions.copy_from_slice(layout.positions());
        let aux_channels = positions
            .iter()
            .filter_map(|channel| match channel {
                ChannelPosition::Aux(index) => Some(*index as usize + 1),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let aux_layout = ChannelPosition::fallback(aux_channels);
        for channel in &mut *positions {
            if let ChannelPosition::Aux(index) = *channel {
                *channel = match aux_layout.get(index as usize).copied() {
                    Some(ChannelPosition::Mono) => ChannelPosition::FrontLeft,
                    channel => channel.unwrap_or_default(),
                };
            }
        }
        ChannelPosition::resolve_surrounds(positions);
        for (output, &channel) in ports.into_iter().zip(&*positions) {
            // The tap's 7.1 layout lacks these, so fold them into its nearest speakers.
            let remixed = match channel {
                ChannelPosition::Mono => [
                    Some(ChannelPosition::FrontLeft),
                    Some(ChannelPosition::FrontRight),
                ],
                ChannelPosition::RearCenter => [
                    Some(ChannelPosition::RearLeft),
                    Some(ChannelPosition::RearRight),
                ],
                ChannelPosition::FrontLeftCenter => [Some(ChannelPosition::FrontLeft), None],
                ChannelPosition::FrontRightCenter => [Some(ChannelPosition::FrontRight), None],
                channel => [Some(channel), None],
            };
            for target in remixed.into_iter().flatten().filter_map(target_for) {
                links.push(link(source, output, tap, target));
            }
        }
    }
    links.sort_unstable();
    links.dedup();
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

#[cfg(test)]
mod tests {
    use super::super::graph::GraphLink;
    use super::*;
    use crate::domain::routing::StreamIdentity;
    use crate::dsp::ChannelPosition::{
        Aux, FrontCenter as FC, FrontLeft as FL, FrontLeftCenter as FLC, FrontRight as FR,
        FrontRightCenter as FRC, LowFrequency as LFE, RearLeft as RL, RearRight as RR,
        SideLeft as SL, SideRight as SR, Unknown,
    };
    use pipewire::spa::utils::Direction;

    const TAP: u32 = 100;
    const LAYOUTS: &[(&[ChannelPosition], &[u32])] = &[
        (&[FL, FR, RL, RR], &[0, 1, 6, 7]),
        (&[FL, FR, FC, RL, RR], &[0, 1, 2, 6, 7]),
        (&[FL, FR, FC, LFE, RL, RR], &[0, 1, 2, 3, 6, 7]),
        (&[FL, FR, FC, LFE, SL, SR], &[0, 1, 2, 3, 6, 7]),
        (
            &[FL, FR, FC, LFE, RL, RR, SL, SR],
            &[0, 1, 2, 3, 4, 5, 6, 7],
        ),
        (
            &[FL, FR, FC, LFE, RL, RR, Aux(6), Aux(7)],
            &[0, 1, 2, 3, 4, 5, 6, 7],
        ),
        (
            &[FL, FR, FC, LFE, Aux(4), Aux(5), SL, SR],
            &[0, 1, 2, 3, 4, 5, 6, 7],
        ),
        (
            &[Aux(0), Aux(1), Aux(2), Aux(3), Aux(4), Aux(5)],
            &[0, 1, 2, 3, 6, 7],
        ),
        (
            &[
                Aux(0),
                Aux(1),
                Aux(2),
                Aux(3),
                Aux(4),
                Aux(5),
                Aux(6),
                Aux(7),
            ],
            &[0, 1, 2, 3, 4, 5, 6, 7],
        ),
        (
            &[FL, FR, FC, LFE, SL, SR, FLC, FRC],
            &[0, 1, 2, 3, 6, 7, 0, 1],
        ),
        (&[Unknown; 6], &[0, 1, 2, 3, 6, 7]),
        (&[Unknown; 8], &[0, 1, 2, 3, 4, 5, 6, 7]),
        (
            &[SR, RL, FC, FL, SL, RR, FR, LFE],
            &[7, 4, 2, 0, 6, 5, 1, 3],
        ),
    ];

    fn add_node(graph: &mut Graph, id: u32, direction: Direction, positions: &[ChannelPosition]) {
        graph.upsert_node(Node {
            id,
            kind: if direction == Direction::Output {
                NodeKind::Playback
            } else {
                NodeKind::Other
            },
            identity: Some(StreamIdentity(format!("test:{id}").into())),
            ..Default::default()
        });
        for (index, &channel) in positions.iter().enumerate().rev() {
            graph.upsert_port(Port {
                global_id: id * 100 + index as u32,
                local_id: index as u32,
                node_id: id,
                channel,
                direction: Some(direction),
                ..Default::default()
            });
        }
        if direction == Direction::Output {
            graph.upsert_link(
                id + 9_000,
                GraphLink {
                    output_node: id,
                    input_node: 99,
                    active: true,
                },
            );
        }
    }

    fn expected_links(source: u32, inputs: impl Iterator<Item = u32>) -> Vec<LinkSpec> {
        inputs
            .enumerate()
            .map(|(output, input)| LinkSpec {
                output_node: source,
                output_port: source * 100 + output as u32,
                input_node: TAP,
                input_port: TAP * 100 + input,
            })
            .collect()
    }

    #[test]
    fn application_links_preserve_surround_roles_on_the_shared_bus() {
        let mut graph = Graph::default();
        let mut tap_layout = ChannelPosition::SURROUND;
        tap_layout.reverse();
        add_node(&mut graph, TAP, Direction::Input, &tap_layout);
        let mut expected = Vec::new();
        for (index, &(layout, destinations)) in LAYOUTS.iter().enumerate() {
            let id = index as u32 + 1;
            add_node(&mut graph, id, Direction::Output, layout);
            expected.extend(expected_links(id, destinations.iter().map(|port| 7 - port)));
        }
        let plan = plan(&graph, &CaptureConfig::default(), Some(TAP));
        assert_eq!(plan.stream.layout, ChannelLayout::SURROUND);
        expected.sort_unstable();
        assert_eq!(
            desired_links(&graph, &plan, graph.node(TAP).unwrap()),
            expected
        );
    }

    #[test]
    fn device_links_keep_negotiated_port_names_and_order() {
        for &(layout, _) in LAYOUTS
            .iter()
            .filter(|(layout, _)| !layout.contains(&Unknown))
        {
            let mut graph = Graph::default();
            add_node(&mut graph, 1, Direction::Output, layout);
            graph.upsert_node(Node {
                id: 1,
                device: true,
                ..Default::default()
            });
            let mut tap_layout = layout.to_vec();
            tap_layout.reverse();
            add_node(&mut graph, TAP, Direction::Input, &tap_layout);
            let plan = plan(
                &graph,
                &CaptureConfig {
                    mode: CaptureMode::Device,
                    device: Some("node#1".into()),
                    ..Default::default()
                },
                Some(TAP),
            );
            assert_eq!(plan.stream.layout.positions(), layout);
            assert_eq!(plan.sources, [1]);
            assert_eq!(
                desired_links(&graph, &plan, graph.node(TAP).unwrap()),
                expected_links(1, (0..layout.len() as u32).rev()),
            );
        }
    }
}
