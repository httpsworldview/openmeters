use crate::audio::pw_registry::{RegistrySnapshot, TargetDescription};

#[derive(Debug, Clone)]
pub(crate) struct HardwareSinkCache {
    visible: TargetDescription,
    last_known: Option<TargetDescription>,
}

impl HardwareSinkCache {
    pub(crate) fn new() -> Self {
        Self {
            visible: TargetDescription {
                display: String::from("(detecting hardware sink...)"),
                raw: String::from("(pending)"),
            },
            last_known: None,
        }
    }

    pub(crate) fn label(&self) -> &str {
        &self.visible.display
    }

    pub(crate) fn update(&mut self, snapshot: &RegistrySnapshot) {
        let summary = snapshot.describe_default_target(snapshot.defaults.audio_sink.as_ref());

        if summary.display != "(none)" || summary.raw != "(none)" {
            self.last_known = Some(summary.clone());
            self.visible = summary;
        } else if let Some(previous) = &self.last_known {
            self.visible = previous.clone();
        } else {
            self.visible = summary;
        }
    }
}
