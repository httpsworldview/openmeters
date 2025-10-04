use async_channel::Receiver as AsyncReceiver;
use iced::advanced::subscription::{EventStream, Hasher, Recipe};
use iced::futures::{self, StreamExt};
use std::hash::Hasher as _;
use std::sync::Arc;

/// Subscription recipe that forwards captured audio frames to the UI thread.
#[derive(Clone, Debug)]
pub struct AudioStreamSubscription {
    receiver: Arc<AsyncReceiver<Vec<f32>>>,
}

impl AudioStreamSubscription {
    pub fn new(receiver: Arc<AsyncReceiver<Vec<f32>>>) -> Self {
        Self { receiver }
    }
}

impl Recipe for AudioStreamSubscription {
    type Output = Vec<f32>;

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
                Ok(samples) => Some((samples, receiver)),
                Err(_) => None,
            }
        })
        .boxed()
    }
}
