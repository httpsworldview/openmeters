// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use std::collections::BTreeMap;

#[derive(Clone, PartialEq, Eq)]
pub(super) enum BarMonitorOption {
    Automatic,
    Output(String),
    Disconnected(String),
}

impl BarMonitorOption {
    pub(super) fn monitor(self) -> Option<String> {
        match self {
            Self::Automatic => None,
            Self::Output(name) | Self::Disconnected(name) => Some(name),
        }
    }
}

impl std::fmt::Display for BarMonitorOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Automatic => f.write_str("Automatic"),
            Self::Output(name) => f.write_str(name),
            Self::Disconnected(name) => write!(f, "{name} (disconnected)"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BarOutputEvent {
    Added,
    Updated,
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BarOutputChange {
    Unchanged,
    Changed,
    Retarget,
}

#[derive(Default)]
pub(super) struct BarOutputs {
    names: BTreeMap<u32, Option<String>>,
    current: Option<u32>,
}

impl BarOutputs {
    pub(super) fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    fn name(&self, id: u32) -> Option<&str> {
        self.names.get(&id)?.as_deref()
    }

    pub(super) fn current_name(&self) -> Option<&str> {
        let current = self.current?;
        Some(self.name(current).unwrap_or("unnamed monitor"))
    }

    fn has_output(&self, name: &str) -> bool {
        self.names
            .values()
            .any(|output| output.as_deref() == Some(name))
    }

    fn selected_is_elsewhere(&self, selected: &str) -> bool {
        self.current.is_some_and(|current| {
            self.name(current) != Some(selected) && self.has_output(selected)
        })
    }

    pub(super) fn sync(
        &mut self,
        id: u32,
        name: Option<String>,
        event: BarOutputEvent,
        selected: Option<&str>,
    ) -> BarOutputChange {
        use BarOutputEvent::{Added, Removed, Updated};

        if event == Removed && self.current == Some(id) {
            self.current = None;
        }
        let name = name.filter(|name| !name.is_empty());
        let previous = if event == Removed {
            self.names.remove(&id)
        } else {
            self.names.insert(id, name.clone())
        };

        let topology_changed = event != Updated;
        let changed = match event {
            Removed => previous.is_some(),
            Added | Updated => previous.as_ref() != Some(&name),
        };
        let route_changed = changed
            && match selected {
                None => match event {
                    Added => self.current != Some(id),
                    Removed => self.current.is_none(),
                    Updated => false,
                },
                Some(selected) => {
                    self.current == Some(id)
                        || previous.as_ref().and_then(|name| name.as_deref()) == Some(selected)
                        || name.as_deref() == Some(selected)
                        || topology_changed && self.current.is_none()
                }
            };
        let retarget = route_changed
            && match selected {
                None => event == Added && self.current.is_some_and(|current| current != id),
                Some(selected) => self.selected_is_elsewhere(selected),
            };
        if retarget {
            BarOutputChange::Retarget
        } else if route_changed {
            BarOutputChange::Changed
        } else {
            BarOutputChange::Unchanged
        }
    }

    // Whether the bar should move to `selected`, which is connected elsewhere.
    pub(super) fn set_current(&mut self, output: Option<u32>, selected: Option<&str>) -> bool {
        self.current = output.filter(|id| self.names.contains_key(id));
        selected.is_some_and(|selected| self.selected_is_elsewhere(selected))
    }

    pub(super) fn choices(
        &self,
        selected: Option<&str>,
    ) -> (Vec<BarMonitorOption>, BarMonitorOption) {
        use BarMonitorOption::{Automatic, Disconnected, Output};
        let selected = match selected {
            None => Automatic,
            Some(name) if self.has_output(name) => Output(name.to_owned()),
            Some(name) => Disconnected(name.to_owned()),
        };
        let disconnected = matches!(selected, Disconnected(_)).then(|| selected.clone());
        let options = std::iter::once(Automatic)
            .chain(self.names.values().flatten().cloned().map(Output))
            .chain(disconnected)
            .collect();
        (options, selected)
    }
}

#[cfg(test)]
mod tests {
    use super::{BarOutputChange::*, BarOutputEvent::*, BarOutputs};

    #[test]
    fn bar_retargeting_follows_output_routes() {
        let mut outputs = BarOutputs {
            current: Some(1),
            ..Default::default()
        };
        outputs.names.insert(1, Some("HDMI".into()));

        assert_eq!(outputs.sync(2, Some("DP".into()), Added, None), Retarget);
        assert_eq!(outputs.sync(2, Some("DP".into()), Added, None), Unchanged);
        assert_eq!(outputs.sync(1, Some("HDMI".into()), Removed, None), Changed);

        assert_eq!(outputs.current, None);
        assert_eq!(
            outputs.sync(1, Some("HDMI".into()), Removed, None),
            Unchanged
        );
        outputs.names.insert(1, Some("HDMI".into()));
        assert!(outputs.set_current(Some(1), Some("DP")));
        assert!(!outputs.set_current(Some(1), Some("missing")));

        outputs.sync(3, None, Added, None);
        assert_eq!(outputs.names.get(&3), Some(&None));
        assert_eq!(outputs.sync(3, Some(String::new()), Added, None), Unchanged);
        let (choices, _) = outputs.choices(None);
        assert_eq!(
            choices.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["Automatic", "HDMI", "DP"]
        );
        assert!(!outputs.set_current(Some(99), None));
        assert_eq!(outputs.current, None);
    }
}
