use super::pw_virtual_sink;
use super::ring_buffer::RingBuffer;
use async_channel::{Receiver as AsyncReceiver, Sender as AsyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

/// Capacity of the channel forwarding audio frames to the UI.
const CHANNEL_CAPACITY: usize = 64;
/// Delay between polling attempts when no audio data is currently available.
const POLL_BACKOFF: Duration = Duration::from_millis(10);

static AUDIO_STREAM: OnceLock<Arc<AsyncReceiver<Vec<f32>>>> = OnceLock::new();

/// Obtain a shared receiver that yields captured audio frames suitable for UI visualisations.
///
/// The first caller spawns a lightweight worker thread that drains the virtual sink capture
/// buffer and forwards frames through the returned async channel. Subsequent callers reuse the
/// existing stream.
pub fn audio_sample_stream() -> Arc<AsyncReceiver<Vec<f32>>> {
    AUDIO_STREAM
        .get_or_init(|| {
            let (sender, receiver) = async_channel::bounded(CHANNEL_CAPACITY);
            spawn_forwarder(sender, pw_virtual_sink::capture_buffer_handle());
            Arc::new(receiver)
        })
        .clone()
}

fn spawn_forwarder(sender: AsyncSender<Vec<f32>>, buffer: Arc<Mutex<RingBuffer<Vec<f32>>>>) {
    thread::Builder::new()
        .name("openmeters-audio-meter-tap".into())
        .spawn(move || forward_loop(sender, buffer))
        .expect("failed to spawn audio meter tap thread");
}

fn forward_loop(sender: AsyncSender<Vec<f32>>, buffer: Arc<Mutex<RingBuffer<Vec<f32>>>>) {
    loop {
        let frame = {
            match buffer.lock() {
                Ok(mut guard) => guard.pop(),
                Err(_) => {
                    eprintln!("[meter-tap] capture buffer lock poisoned; stopping tap");
                    return;
                }
            }
        };

        if let Some(samples) = frame {
            if sender.send_blocking(samples).is_err() {
                break;
            }
        } else {
            thread::sleep(POLL_BACKOFF);
        }
    }

    println!("[meter-tap] audio channel closed; ending tap thread");
}
