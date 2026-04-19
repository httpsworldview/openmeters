// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

crate::settings_enum!(no_default
    #[derive(Hash)]
    pub enum VisualKind {
        Loudness => "Loudness",
        Oscilloscope => "Oscilloscope",
        Waveform => "Waveform",
        Spectrogram => "Spectrogram",
        Spectrum => "Spectrum",
        Stereometer => "Stereometer",
    }
);
