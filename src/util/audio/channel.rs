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

    let frame_count = frames.min(interleaved.len() / channels);
    output.reserve(frame_count);
    let chunks = interleaved.chunks_exact(channels).take(frame_count);
    let right = |frame: &[f32]| frame.get(1).copied().unwrap_or(frame[0]);
    match channel {
        Channel::Left => output.extend(chunks.map(|frame| frame[0])),
        Channel::Right => output.extend(chunks.map(right)),
        Channel::Mid => match channels {
            1 => output.extend(chunks.map(|frame| frame[0])),
            2 => output.extend(chunks.map(|frame| (frame[0] + frame[1]) * 0.5)),
            _ => {
                let gain = 1.0 / channels as f32;
                output.extend(chunks.map(|frame| frame.iter().sum::<f32>() * gain));
            }
        },
        Channel::Side => output.extend(chunks.map(|frame| (frame[0] - right(frame)) * 0.5)),
        Channel::None => unreachable!(),
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
