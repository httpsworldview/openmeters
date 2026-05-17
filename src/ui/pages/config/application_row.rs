// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::infra::pipewire::registry::NodeInfo;

#[derive(Clone, Debug)]
pub(crate) struct ApplicationRow {
    pub(crate) node_id: u32,
    pub(crate) label: String,
    sort_key: (String, String, u32),
    pub(crate) enabled: bool,
}

impl ApplicationRow {
    pub(crate) fn from_node(node: &NodeInfo, enabled: bool) -> Self {
        let primary = node
            .app_name()
            .map(str::to_owned)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| node.display_name());
        let node_label = node.display_name();
        let secondary = (!primary.eq_ignore_ascii_case(&node_label)).then_some(node_label);
        let label = secondary
            .as_ref()
            .map_or_else(|| primary.clone(), |s| format!("{primary} ({s})"));

        Self {
            node_id: node.id,
            label,
            sort_key: (
                primary.to_ascii_lowercase(),
                secondary.unwrap_or_default().to_ascii_lowercase(),
                node.id,
            ),
            enabled,
        }
    }

    pub(crate) fn sort_key(&self) -> &(String, String, u32) {
        &self.sort_key
    }
}
