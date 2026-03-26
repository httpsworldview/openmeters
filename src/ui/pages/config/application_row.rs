// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::infra::pipewire::registry::NodeInfo;

#[derive(Clone, Debug)]
pub(crate) struct ApplicationRow {
    pub(crate) node_id: u32,
    pub(crate) primary: String,
    pub(crate) secondary: Option<String>,
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
        let secondary = if primary.eq_ignore_ascii_case(&node_label) {
            None
        } else {
            Some(node_label)
        };

        Self {
            node_id: node.id,
            primary,
            secondary,
            enabled,
        }
    }

    pub(crate) fn display_label(&self) -> String {
        match &self.secondary {
            Some(secondary) => format!("{} ({})", self.primary, secondary),
            None => self.primary.clone(),
        }
    }
}
