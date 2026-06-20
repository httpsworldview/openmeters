// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

pub fn fmt_freq(f: f32) -> String {
    match f {
        f if f >= 10_000.0 => format!("{:.1}kHz", f / 1000.0),
        f if f >= 1_000.0 => format!("{:.2}kHz", f / 1000.0),
        f if f >= 100.0 => format!("{f:.1}Hz"),
        _ => format!("{f:.2}Hz"),
    }
}

pub fn fmt_duration(secs: f32) -> String {
    if secs >= 60.0 {
        format!("{:.0}m {:.0}s", (secs / 60.0).floor(), secs % 60.0)
    } else {
        format!("{secs:.2}s")
    }
}
