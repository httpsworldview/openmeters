use crate::audio::VIRTUAL_SINK_NAME;
use crate::audio::pw_registry::RegistrySnapshot;
use crate::ui::application_row::ApplicationRow;
use crate::ui::hardware_sink::HardwareSinkCache;
use async_channel::Receiver as AsyncReceiver;
use iced::advanced::subscription::{EventStream, Hasher, Recipe, from_recipe};
use iced::futures::{self, StreamExt};
use iced::widget::{Column, checkbox, container, scrollable, text};
use iced::{Element, Length, Subscription, Task};
use std::collections::{HashMap, HashSet};
use std::hash::Hasher as _;
use std::sync::{Arc, mpsc};

#[derive(Debug, Clone)]
pub enum RoutingCommand {
    SetApplicationEnabled { node_id: u32, enabled: bool },
}

#[derive(Debug, Clone)]
pub enum ConfigMessage {
    RegistryUpdated(RegistrySnapshot),
    ToggleChanged { node_id: u32, enabled: bool },
}

#[derive(Debug)]
pub struct ConfigPage {
    routing_sender: mpsc::Sender<RoutingCommand>,
    registry_updates: Option<Arc<AsyncReceiver<RegistrySnapshot>>>,
    preferences: HashMap<u32, bool>,
    applications: Vec<ApplicationRow>,
    hardware_sink: HardwareSinkCache,
    registry_ready: bool,
}

impl ConfigPage {
    pub fn new(
        routing_sender: mpsc::Sender<RoutingCommand>,
        registry_updates: Option<Arc<AsyncReceiver<RegistrySnapshot>>>,
    ) -> Self {
        Self {
            routing_sender,
            registry_updates,
            preferences: HashMap::new(),
            applications: Vec::new(),
            hardware_sink: HardwareSinkCache::new(),
            registry_ready: false,
        }
    }

    pub fn subscription(&self) -> Subscription<ConfigMessage> {
        let mut subscriptions = Vec::new();

        if let Some(receiver) = &self.registry_updates {
            subscriptions.push(from_recipe(RegistrySubscription {
                receiver: Arc::clone(receiver),
            }));
        }

        match subscriptions.len() {
            0 => Subscription::none(),
            1 => subscriptions.into_iter().next().unwrap(),
            _ => Subscription::batch(subscriptions),
        }
    }

    pub fn update(&mut self, message: ConfigMessage) -> Task<ConfigMessage> {
        match message {
            ConfigMessage::RegistryUpdated(snapshot) => {
                self.registry_ready = true;
                self.apply_snapshot(snapshot);
            }
            ConfigMessage::ToggleChanged { node_id, enabled } => {
                self.preferences.insert(node_id, enabled);

                if let Some(entry) = self
                    .applications
                    .iter_mut()
                    .find(|entry| entry.node_id == node_id)
                {
                    entry.enabled = enabled;
                }

                if let Err(err) = self
                    .routing_sender
                    .send(RoutingCommand::SetApplicationEnabled { node_id, enabled })
                {
                    eprintln!("[ui] failed to send routing command: {err}");
                }
            }
        }

        Task::none()
    }

    pub fn view(&self) -> Element<'_, ConfigMessage> {
        let sink_label = format!("Hardware sink: {}", self.hardware_sink.label());

        let mut list = Column::new().spacing(8);

        if self.applications.is_empty() {
            let message = if self.registry_updates.is_some() {
                if self.registry_ready {
                    "No audio applications detected. Launch something to see it here."
                } else {
                    "Waiting for PipeWire registry snapshots..."
                }
            } else {
                "Registry unavailable; routing controls disabled."
            };

            list = list.push(text(message));
        } else {
            for entry in &self.applications {
                let node_id = entry.node_id;
                let label = entry.display_label();
                list =
                    list.push(checkbox(label, entry.enabled).on_toggle(move |enabled| {
                        ConfigMessage::ToggleChanged { node_id, enabled }
                    }));
            }
        }

        let content = Column::new()
            .spacing(16)
            .push(text(sink_label).size(14))
            .push(scrollable(list).height(Length::Fill));

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn apply_snapshot(&mut self, snapshot: RegistrySnapshot) {
        self.hardware_sink.update(&snapshot);

        let mut entries = Vec::new();
        let mut seen = HashSet::new();

        if let Some(sink) = snapshot.find_node_by_label(VIRTUAL_SINK_NAME) {
            for node in snapshot.route_candidates(sink) {
                let enabled = self.preferences.get(&node.id).copied().unwrap_or(true);
                entries.push(ApplicationRow::from_node(node, enabled));
                seen.insert(node.id);
            }
        }

        self.preferences.retain(|node_id, _| seen.contains(node_id));

        entries.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
        self.applications = entries;
    }
}

struct RegistrySubscription {
    receiver: Arc<AsyncReceiver<RegistrySnapshot>>,
}

impl Recipe for RegistrySubscription {
    type Output = ConfigMessage;

    fn hash(&self, state: &mut Hasher) {
        let ptr = Arc::as_ptr(&self.receiver) as usize;
        state.write(&ptr.to_ne_bytes());
    }

    fn stream(
        self: Box<Self>,
        _input: EventStream,
    ) -> futures::stream::BoxStream<'static, Self::Output> {
        futures::stream::unfold(self.receiver, |receiver| async move {
            match receiver.recv().await {
                Ok(snapshot) => Some((ConfigMessage::RegistryUpdated(snapshot), receiver)),
                Err(_) => None,
            }
        })
        .boxed()
    }
}
