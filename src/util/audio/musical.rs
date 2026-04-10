// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

const A440_HZ: f32 = 440.0;
const A440_MIDI: i32 = 69;
const SEMITONES_PER_OCTAVE: i32 = 12;
const MIDI_OCTAVE_OFFSET: i32 = 1;

const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MusicalNote {
    pub midi_number: i32,
    pub name: &'static str,
    pub octave: i32,
}

fn freq_to_midi(freq_hz: f32) -> Option<f32> {
    if freq_hz <= 0.0 || !freq_hz.is_finite() {
        return None;
    }
    let m = A440_MIDI as f32 + SEMITONES_PER_OCTAVE as f32 * (freq_hz / A440_HZ).log2();
    m.is_finite().then_some(m)
}

impl MusicalNote {
    pub fn from_frequency(freq_hz: f32) -> Option<Self> {
        freq_to_midi(freq_hz).map(|m| Self::from_midi(m.round() as i32))
    }

    pub fn from_midi(midi_number: i32) -> Self {
        let note_index = ((midi_number % SEMITONES_PER_OCTAVE + SEMITONES_PER_OCTAVE)
            % SEMITONES_PER_OCTAVE) as usize;
        let octave = (midi_number / SEMITONES_PER_OCTAVE) - MIDI_OCTAVE_OFFSET;
        Self {
            midi_number,
            name: NOTE_NAMES[note_index],
            octave,
        }
    }

    pub fn to_frequency(self) -> f32 {
        A440_HZ * 2.0f32.powf((self.midi_number - A440_MIDI) as f32 / SEMITONES_PER_OCTAVE as f32)
    }

    pub fn is_black(self) -> bool {
        matches!(self.name, "C#" | "D#" | "F#" | "G#" | "A#")
    }

    pub fn format(&self) -> String {
        format!("{}{}", self.name, self.octave)
    }
}

/// Nearest note and cents deviation for a frequency.
#[derive(Debug, Clone, Copy)]
pub struct NoteInfo {
    pub note: MusicalNote,
    pub cents: i32,
}

impl NoteInfo {
    pub fn from_frequency(freq_hz: f32) -> Option<Self> {
        freq_to_midi(freq_hz).map(|midi| {
            let rounded = midi.round() as i32;
            let cents = ((midi - rounded as f32) * 100.0).round() as i32;
            Self {
                note: MusicalNote::from_midi(rounded),
                cents,
            }
        })
    }

    /// `"F4  + 42 Cents"`
    pub fn fmt_note_cents(&self) -> String {
        let sign = if self.cents >= 0 { '+' } else { '-' };
        format!("{:<4}{sign} {} Cents", self.note.format(), self.cents.abs())
    }
}
