// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

crate::macros::choice_enum!(no_default all pub enum Channel {
    Left => "Left",
    Right => "Right",
    Mid => "Mid",
    Side => "Side",
    None => "None",
});

pub(crate) fn mix_stereo(frame: &[f32], matrix: &[[f32; 2]]) -> [f32; 2] {
    frame
        .iter()
        .zip(matrix)
        .fold([0.0; 2], |[left, right], (&sample, weights)| {
            [left + sample * weights[0], right + sample * weights[1]]
        })
}

pub(crate) fn project_interleaved_channel_into(
    output: &mut Vec<f32>,
    interleaved: &[f32],
    channels: usize,
    frames: usize,
    matrix: &[[f32; 2]],
    channel: Channel,
) -> bool {
    output.clear();
    if channels == 0 || channel == Channel::None {
        return false;
    }

    let frame_count = frames.min(interleaved.len() / channels);
    output.reserve(frame_count);
    output.extend(
        interleaved
            .chunks_exact(channels)
            .take(frame_count)
            .map(|frame| {
                let [left, right] = mix_stereo(frame, matrix);
                match channel {
                    Channel::Left => left,
                    Channel::Right => right,
                    Channel::Mid => (left + right) * 0.5,
                    Channel::Side => (left - right) * 0.5,
                    Channel::None => unreachable!(),
                }
            }),
    );
    !output.is_empty()
}
