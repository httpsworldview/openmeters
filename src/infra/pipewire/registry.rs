// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

mod runtime;
mod state;
mod types;

pub use runtime::{AudioRegistryHandle, spawn_registry};
pub use types::{GraphPort, LinkSpec, NodeInfo, RegistrySnapshot};

#[cfg(test)]
use types::{AudioChannel, DefaultTarget, Direction, MetadataDefaults};

pub fn pair_ports_by_channel<'a>(
    sources: impl IntoIterator<Item = &'a GraphPort>,
    targets: impl IntoIterator<Item = &'a GraphPort>,
) -> Vec<(&'a GraphPort, &'a GraphPort)> {
    let mut sources: Vec<_> = sources.into_iter().collect();
    let mut targets: Vec<_> = targets.into_iter().collect();
    sources.sort_by_key(|p| p.port_id);
    targets.sort_by_key(|p| p.port_id);

    let use_channel = sources.iter().chain(&targets).all(|p| p.channel.is_some());

    let matches = |src: &GraphPort, target: &GraphPort| {
        (use_channel && src.channel == target.channel)
            || (!use_channel && src.port_id == target.port_id)
    };

    sources
        .into_iter()
        .filter_map(|src| {
            let idx = targets.iter().position(|&target| matches(src, target))?;
            Some((src, targets.remove(idx)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(id: u32, channel: Option<&str>) -> GraphPort {
        GraphPort {
            global_id: 100 + id,
            port_id: id,
            node_id: 1,
            channel: channel.and_then(AudioChannel::parse),
            direction: Direction::Output,
            is_monitor: false,
        }
    }

    fn ports(items: &[(u32, Option<&str>)]) -> Vec<GraphPort> {
        items.iter().map(|&(id, ch)| port(id, ch)).collect()
    }

    #[test]
    fn pair_ports_by_channel_behavior() {
        let ids = |sources, targets| -> Vec<(u32, u32)> {
            pair_ports_by_channel(&ports(sources), &ports(targets))
                .iter()
                .map(|(s, t)| (s.port_id, t.port_id))
                .collect()
        };

        for (sources, targets, expected) in [
            (
                &[(0, Some("FL"))][..],
                &[(0, Some("FL"))][..],
                &[(0, 0)][..],
            ),
            (&[(0, Some("FL"))], &[(1, Some("FL"))], &[(0, 1)]),
            (
                &[(1, Some("FR"))],
                &[(0, Some("FL")), (1, Some("FR"))],
                &[(1, 1)],
            ),
            (
                &[(1, Some("FR")), (0, Some("FL"))],
                &[(1, Some("FR")), (0, Some("FL"))],
                &[(0, 0), (1, 1)],
            ),
            (
                &[(0, None), (1, None)],
                &[(0, None), (1, None)],
                &[(0, 0), (1, 1)],
            ),
            (&[(0, Some("UNK"))], &[(0, Some("FL"))], &[(0, 0)]),
            (&[(0, Some("FL"))], &[(0, Some("UNK"))], &[(0, 0)]),
            (
                &[(0, Some("FL")), (1, Some("FR"))],
                &[(0, Some("FL"))],
                &[(0, 0)],
            ),
        ] {
            assert_eq!(ids(sources, targets), expected);
        }

        let ch51 = ["FL", "FR", "FC", "LFE", "RL", "RR"];
        let src: Vec<_> = ch51
            .iter()
            .enumerate()
            .map(|(i, c)| port(i as u32, Some(c)))
            .collect();
        let tgt: Vec<_> = ch51
            .iter()
            .enumerate()
            .rev()
            .map(|(i, c)| port(i as u32, Some(c)))
            .collect();
        assert!(
            pair_ports_by_channel(&src, &tgt)
                .iter()
                .all(|(s, t)| s.channel == t.channel)
        );
    }

    #[test]
    fn capture_device_tokens_prefer_names_then_descriptions_and_fallbacks() {
        let snapshot = RegistrySnapshot {
            nodes: vec![
                NodeInfo {
                    id: 7,
                    name: Some("alsa_output.usb".into()),
                    description: Some("External DAC".into()),
                    media_class: Some("Audio/Sink".into()),
                    ..Default::default()
                },
                NodeInfo {
                    id: 8,
                    name: Some("external dac".into()),
                    description: Some("Desk speakers".into()),
                    media_class: Some("Audio/Sink".into()),
                    ..Default::default()
                },
                NodeInfo {
                    id: 9,
                    media_class: Some("Audio/Source".into()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let id = |token| snapshot.find_capture_device_by_token(token).map(|n| n.id);

        assert_eq!(id("alsa_output.usb"), Some(7));
        assert_eq!(id("External DAC"), Some(8));
        assert_eq!(id("Desk speakers"), Some(8));
        assert_eq!(id("NODE#9"), Some(9));
        assert_eq!(id("missing"), None);
    }

    #[test]
    fn metadata_defaults_reconcile_matches_by_name() {
        use std::collections::HashMap;

        let mut defaults = MetadataDefaults {
            audio_sink: Some(DefaultTarget {
                metadata_id: Some(7),
                node_id: None,
                name: Some("node.main".into()),
                type_hint: None,
            }),
            audio_source: None,
        };

        let mut nodes = HashMap::new();
        nodes.insert(
            42,
            NodeInfo {
                id: 42,
                name: Some("node.main".into()),
                ..Default::default()
            },
        );

        defaults.reconcile_with_nodes(&nodes);
        assert_eq!(
            defaults.audio_sink.as_ref().and_then(|t| t.node_id),
            Some(42)
        );
    }
}
