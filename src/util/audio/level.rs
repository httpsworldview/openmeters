// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

pub const DB_FLOOR: f32 = -140.0;
pub const LN_TO_DB: f32 = 4.342_944_8;

const POWER_EPSILON: f32 = 1.0e-20;

pub fn sanitize_negative_db(db: f32, default: f32) -> f32 {
    if db.is_finite() && db < 0.0 {
        db
    } else {
        default
    }
}

pub fn power_to_db(power: f32, floor: f32) -> f32 {
    if power > POWER_EPSILON {
        (power.ln() * LN_TO_DB).max(floor)
    } else {
        floor
    }
}

pub fn db_to_power(db: f32) -> f32 {
    const DB_TO_LOG2: f32 = 0.1 * core::f32::consts::LOG2_10;
    (db * DB_TO_LOG2).exp2()
}
