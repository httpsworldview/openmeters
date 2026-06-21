// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use std::collections::VecDeque;

crate::macros::choice_enum!(no_default all pub enum Channel {
    Left => "Left",
    Right => "Right",
    Mid => "Mid",
    Side => "Side",
    None => "None",
});

#[inline]
fn project_interleaved_frame(frame: &[f32], channels: usize, channel: Channel) -> Option<f32> {
    if channels == 0 || channel == Channel::None {
        return None;
    }
    let len = channels.min(frame.len());
    let left = *frame.first()?;
    let right = if channels > 1 {
        frame.get(1).copied().unwrap_or(left)
    } else {
        left
    };
    match channel {
        Channel::Left => Some(left),
        Channel::Right => Some(right),
        Channel::Mid => Some(frame[..len].iter().sum::<f32>() / len as f32),
        Channel::Side => Some((left - right) * 0.5),
        Channel::None => None,
    }
}

pub(crate) fn project_interleaved_channel_into(
    output: &mut Vec<f32>,
    interleaved: &[f32],
    channels: usize,
    frames: usize,
    channel: Channel,
) -> bool {
    output.clear();
    if channels == 0 || channel == Channel::None {
        return false;
    }
    output.reserve(frames);
    for frame in interleaved.chunks_exact(channels).take(frames) {
        if let Some(sample) = project_interleaved_frame(frame, channels, channel) {
            output.push(sample);
        }
    }
    !output.is_empty()
}

pub fn extend_interleaved_history(
    history: &mut VecDeque<f32>,
    samples: &[f32],
    capacity: usize,
    channels: usize,
) {
    let capacity = capacity / channels.max(1) * channels;
    if capacity == 0 || channels == 0 {
        history.clear();
        return;
    }
    let samples = &samples[..samples.len() / channels * channels];
    if samples.is_empty() {
        return;
    }

    if samples.len() >= capacity {
        history.clear();
        history.extend(&samples[samples.len() - capacity..]);
        return;
    }

    let overflow = history.len() + samples.len();
    if overflow > capacity {
        let drain = (overflow - capacity).div_ceil(channels) * channels;
        history.drain(..drain.min(history.len()));
    }
    history.extend(samples);
}
