mod runtime;
mod state;
mod types;

pub use runtime::{AudioRegistryHandle, spawn_registry};
pub use types::{GraphPort, LinkSpec, NodeInfo, RegistrySnapshot};

#[cfg(test)]
use types::{DefaultTarget, MetadataDefaults, PortDirection};

pub fn pair_ports_by_channel(
    mut sources: Vec<GraphPort>,
    mut targets: Vec<GraphPort>,
) -> Vec<(GraphPort, GraphPort)> {
    sources.sort_by_key(|p| p.port_id);
    targets.sort_by_key(|p| p.port_id);

    let is_known_channel = |c: &str| {
        c.eq_ignore_ascii_case("FL")
            || c.eq_ignore_ascii_case("FR")
            || c.eq_ignore_ascii_case("FC")
            || c.eq_ignore_ascii_case("LFE")
            || c.eq_ignore_ascii_case("RL")
            || c.eq_ignore_ascii_case("RR")
            || c.eq_ignore_ascii_case("SL")
            || c.eq_ignore_ascii_case("SR")
            || c.eq_ignore_ascii_case("MONO")
    };
    let valid_channel = |ch: Option<&str>| ch.is_some_and(is_known_channel);

    let use_channel = sources.iter().all(|p| valid_channel(p.channel.as_deref()))
        && targets.iter().all(|p| valid_channel(p.channel.as_deref()));

    let mut pairs = Vec::with_capacity(sources.len().min(targets.len()));
    let mut used: rustc_hash::FxHashSet<u32> = rustc_hash::FxHashSet::default();

    for src in &sources {
        let target = targets.iter().find(|t| {
            !used.contains(&t.port_id)
                && if use_channel {
                    match (src.channel.as_deref(), t.channel.as_deref()) {
                        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
                        _ => false,
                    }
                } else {
                    src.port_id == t.port_id
                }
        });

        if let Some(tgt) = target {
            used.insert(tgt.port_id);
            pairs.push((src.clone(), tgt.clone()));
        }
    }

    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(id: u32, channel: Option<&str>) -> GraphPort {
        GraphPort {
            global_id: 100 + id,
            port_id: id,
            node_id: 1,
            channel: channel.map(String::from),
            direction: PortDirection::Output,
            is_monitor: false,
        }
    }

    #[test]
    fn pair_ports_by_channel_behavior() {
        let p = pair_ports_by_channel;
        let ids = |pairs: &[(GraphPort, GraphPort)]| -> Vec<(u32, u32)> {
            pairs.iter().map(|(s, t)| (s.port_id, t.port_id)).collect()
        };

        assert_eq!(
            ids(&p(vec![port(0, Some("FL"))], vec![port(0, Some("FL"))])),
            [(0, 0)]
        );
        assert_eq!(
            ids(&p(vec![port(0, Some("FL"))], vec![port(1, Some("FL"))])),
            [(0, 1)]
        );
        assert_eq!(
            ids(&p(
                vec![port(1, Some("FR"))],
                vec![port(0, Some("FL")), port(1, Some("FR"))]
            )),
            [(1, 1)]
        );
        assert_eq!(
            ids(&p(
                vec![port(1, Some("FR")), port(0, Some("FL"))],
                vec![port(1, Some("FR")), port(0, Some("FL"))]
            )),
            [(0, 0), (1, 1)]
        );
        assert_eq!(
            ids(&p(
                vec![port(0, None), port(1, None)],
                vec![port(0, None), port(1, None)]
            )),
            [(0, 0), (1, 1)]
        );
        assert_eq!(
            ids(&p(vec![port(0, Some("UNK"))], vec![port(0, Some("FL"))])),
            [(0, 0)]
        );
        assert_eq!(
            ids(&p(vec![port(0, Some("FL"))], vec![port(0, Some("UNK"))])),
            [(0, 0)]
        );
        assert_eq!(
            ids(&p(
                vec![port(0, Some("FL")), port(1, Some("FR"))],
                vec![port(0, Some("FL"))]
            )),
            [(0, 0)]
        );

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
        assert!(p(src, tgt).iter().all(|(s, t)| s.channel == t.channel));
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
