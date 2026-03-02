use super::types::{GraphPort, MetadataDefaults, NodeInfo, RegistrySnapshot};
use std::collections::HashMap;

#[derive(Debug, Default)]
pub(crate) struct RegistryState {
    serial: u64,
    nodes: HashMap<u32, NodeInfo>,
    device_count: usize,
    port_index: HashMap<u32, (u32, u32)>,
    metadata_defaults: MetadataDefaults,
}

impl RegistryState {
    pub(crate) fn snapshot(&self) -> RegistrySnapshot {
        let mut nodes: Vec<_> = self.nodes.values().cloned().collect();
        nodes.sort_by_key(|node| node.id);

        RegistrySnapshot {
            serial: self.serial,
            nodes,
            device_count: self.device_count,
            defaults: self.metadata_defaults.clone(),
        }
    }

    pub(crate) fn upsert_node(&mut self, info: NodeInfo) -> bool {
        if self.nodes.get(&info.id) == Some(&info) {
            return false;
        }
        self.nodes.insert(info.id, info);
        self.metadata_defaults.reconcile_with_nodes(&self.nodes);
        self.bump_serial();
        true
    }

    pub(crate) fn remove_node(&mut self, id: u32) -> bool {
        if let Some(info) = self.nodes.remove(&id) {
            let fallback = info.name.or(info.description);
            if self.metadata_defaults.clear_node(id, fallback) {
                self.metadata_defaults.reconcile_with_nodes(&self.nodes);
            }
            self.bump_serial();
            true
        } else {
            false
        }
    }

    pub(crate) fn add_device(&mut self) {
        self.device_count += 1;
        self.bump_serial();
    }

    pub(crate) fn upsert_port(&mut self, port: GraphPort) -> bool {
        let (node_id, port_id, global_id) = (port.node_id, port.port_id, port.global_id);
        let Some(node) = self.nodes.get_mut(&node_id) else {
            return false;
        };

        let changed = match node.ports.iter().position(|p| p.port_id == port_id) {
            Some(idx) if node.ports[idx] != port => {
                node.ports[idx] = port;
                true
            }
            Some(_) => false,
            None => {
                node.ports.push(port);
                true
            }
        };

        if changed {
            self.port_index.insert(global_id, (node_id, port_id));
            self.bump_serial();
        }
        changed
    }

    pub(crate) fn remove_port(&mut self, global_id: u32) -> bool {
        let Some((node_id, port_id)) = self.port_index.remove(&global_id) else {
            return false;
        };
        let Some(node) = self.nodes.get_mut(&node_id) else {
            return false;
        };
        node.ports.retain(|p| p.port_id != port_id);
        self.bump_serial();
        true
    }

    pub(crate) fn apply_metadata_property(
        &mut self,
        metadata_id: u32,
        subject: u32,
        key: Option<&str>,
        type_hint: Option<&str>,
        value: Option<&str>,
    ) -> bool {
        let changed = match key {
            Some(key) => {
                self.metadata_defaults
                    .apply_update(metadata_id, subject, key, type_hint, value)
            }
            None => self.metadata_defaults.clear_metadata(metadata_id),
        };

        if changed {
            self.metadata_defaults.reconcile_with_nodes(&self.nodes);
            self.bump_serial();
        }

        changed
    }

    fn bump_serial(&mut self) {
        self.serial = self.serial.wrapping_add(1);
    }
}
