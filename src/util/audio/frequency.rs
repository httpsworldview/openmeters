// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

use crate::util::lerp;

crate::macros::choice_enum!(all pub enum FrequencyScale {
    Linear => "Linear",
    #[default] Logarithmic => "Logarithmic",
    #[serde(alias = "mel")] Erb => "Erb",
});

// Mirrored in visuals/render/shaders/spectrogram.wgsl.
const LOG_KNEE_HZ: f32 = 20.0;

impl FrequencyScale {
    pub fn freq_at(self, min: f32, max: f32, t: f32) -> f32 {
        self.unscale(lerp(self.scale(min), self.scale(max), t))
    }

    pub fn pos_of(self, min: f32, max: f32, freq: f32) -> f32 {
        let (lo, hi) = (self.scale(min), self.scale(max));
        (self.scale(freq) - lo) / (hi - lo).max(1e-6)
    }

    pub(crate) fn scale(self, hz: f32) -> f32 {
        match self {
            Self::Linear => hz,
            Self::Logarithmic => (hz / LOG_KNEE_HZ).asinh(),
            Self::Erb => hz_to_erb_rate(hz),
        }
    }

    fn unscale(self, x: f32) -> f32 {
        match self {
            Self::Linear => x,
            Self::Logarithmic => LOG_KNEE_HZ * x.sinh(),
            Self::Erb => erb_rate_to_hz(x),
        }
    }
}

fn hz_to_erb_rate(hz: f32) -> f32 {
    21.4 * (1.0 + hz / 228.8).log10()
}

fn erb_rate_to_hz(erb: f32) -> f32 {
    228.8 * (10.0f32.powf(erb / 21.4) - 1.0)
}
