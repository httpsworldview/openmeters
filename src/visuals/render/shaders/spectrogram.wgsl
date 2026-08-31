// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

const LOG10_E: f32 = 0.4342944819;
const LN_TO_DB: f32 = 4.342944819;
const DB_TO_LOG2: f32 = 0.3321928095;
// Rg16Float cannot hold -140 dB power in one channel; G is a scaled fallback.
const LOW_POWER_SCALE: f32 = 16777216.0;
const INV_LOW_POWER_SCALE: f32 = 0.000000059604644775390625;
const F16_MAX: f32 = 65504.0;
const LOG_KNEE_HZ: f32 = 20.0;

// Classic storage domain -- keep in sync with processor.rs CLASSIC_DB_STORE_*.
const DB_STORE_LO: f32 = -144.0;
const DB_STORE_HI: f32 = 12.0;
const DB_STORE_RANGE: f32 = DB_STORE_HI - DB_STORE_LO;

// Analysis floor -- keep in sync with util::audio::DB_FLOOR.
const DB_ANALYSIS_FLOOR: f32 = -140.0;
const DB_FLOOR_EPS: f32 = 0.01;
// DB_ANALYSIS_FLOOR + DB_FLOOR_EPS in linear power.
const ANALYSIS_POWER_EPS: f32 = 1.0023052e-14;

// Must match Rust-side Uniforms layout exactly.
struct Uniforms {
    freq_axis: vec2<f32>,           // (scaled_min, inverse scaled display span)
    freq_scale: u32,                // 0=linear, 1=log, 2=erb
    points_per_col: u32,            // reassigned slot stride, or classic FFT bins

    history_length: u32,
    col_count: u32,
    rotation: u32,

    bounds: vec4<f32>,              // (x, y, w, h) physical pixels
    clip_scale: vec2<f32>,          // (2/viewport_w, 2/viewport_h)
    uv_y_range: vec2<f32>,          // zoom/pan window into [0,1] freq axis
    scale_factor: f32,

    floor_db: f32,
    tilt_db: f32,

    newest_col: u32,
    inv_uv_range: f32,
    // FFT bin spacing (sample_rate / fft_size); only used by classic sampling.
    bin_hz: f32,
    reassigned_power_scale: f32,

    // (pos1, pos2, pos3, spread0), (spread1, spread2, spread3, spread4).
    // Stops 0 and 4 are constant 0.0 / 1.0
    stops: array<vec4<f32>, 2>,
    palette: array<vec4<f32>, 5>,
}

struct AccumOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) power: f32,
    @location(1) @interpolate(flat) freq_hz: f32,
}

struct ResolveOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) accum_pos: vec2<f32>,
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var accum_tex: texture_2d<f32>;
@group(0) @binding(2) var<storage, read> mags: array<u32>;

fn freq_to_norm(hz: f32) -> f32 {
    var scaled: f32;
    switch u.freq_scale {
        case 1u: { scaled = asinh(hz / LOG_KNEE_HZ); }
        // Glasberg & Moore (1990)
        case 2u: { scaled = 21.4 * log(1.0 + hz / 228.8) * LOG10_E; }
        default: { scaled = hz; }
    }
    return (scaled - u.freq_axis.x) * u.freq_axis.y;
}

fn palette_color(t: f32) -> vec4<f32> {
    let tc = clamp(t, 0.0, 1.0);
    let segment = select(0u, 1u, tc > u.stops[0].x)
        + select(0u, 1u, tc > u.stops[0].y)
        + select(0u, 1u, tc > u.stops[0].z);
    let i = select(3u, segment, tc <= 1.0);
    let lo_positions = vec4<f32>(0.0, u.stops[0].xyz);
    let hi_positions = vec4<f32>(u.stops[0].xyz, 1.0);
    let left_spreads = vec4<f32>(u.stops[0].w, u.stops[1].xyz);
    let linear_t = clamp(
        (tc - lo_positions[i]) / max(hi_positions[i] - lo_positions[i], 1e-6),
        0.0,
        1.0,
    );
    let sl = left_spreads[i];
    let sr = u.stops[1][i];
    var blend = linear_t;
    if !(abs(sl - 1.0) < 1e-4 && abs(sr - 1.0) < 1e-4) {
        blend = clamp(pow(linear_t, sl / sr), 0.0, 1.0);
    }
    return mix(u.palette[i], u.palette[i + 1u], blend);
}

fn extents() -> vec2<f32> {
    let swapped = u.rotation == 1u || u.rotation == 3u;
    return vec2<f32>(
        select(u.bounds.z, u.bounds.w, swapped),
        select(u.bounds.w, u.bounds.z, swapped),
    );
}

fn unpack_mag(slot: u32, bin_in_col: u32) -> f32 {
    let idx = slot * ((u.points_per_col + 1u) & ~1u) + bin_in_col;
    let pair = unpack2x16unorm(mags[idx / 2u]);
    return select(pair.y, pair.x, (idx & 1u) == 0u) * DB_STORE_RANGE + DB_STORE_LO;
}

const CULL_POS: vec4<f32> = vec4<f32>(0.0, 0.0, 2.0, 1.0);
const CLASSIC_SENTINEL_DB: f32 = -10000.0;

fn quad_corner(vertex: u32) -> vec2<f32> {
    return vec2<f32>(f32(vertex / 2u) - 0.5, 0.5 - f32(vertex % 2u));
}

fn quad_clip(local: vec2<f32>) -> vec4<f32> {
    let px = u.bounds.xy + local;
    return vec4<f32>(px.x * u.clip_scale.x - 1.0, 1.0 - px.y * u.clip_scale.y, 0.0, 1.0);
}

@vertex
fn vs_accum_splat(
    @location(1) time_offset: f32,
    @location(2) freq_hz: f32,
    @location(3) power: f32,
    @builtin(vertex_index) vertex: u32,
    @builtin(instance_index) inst: u32,
) -> AccumOutput {
    let corner = quad_corner(vertex);
    let zoomed = (freq_to_norm(freq_hz) - u.uv_y_range.x) * u.inv_uv_range;
    if !(power > 0.0) || zoomed < -0.01 || zoomed > 1.01 {
        return AccumOutput(CULL_POS, power, freq_hz);
    }
    let ext = extents();
    let hl = u.history_length;
    let age = (u.newest_col + hl - inst / u.points_per_col) % hl;
    let pos = vec2<f32>(ext.x - (f32(age) - time_offset) * u.scale_factor, (1.0 - zoomed) * ext.y)
        + corner * u.scale_factor;
    let size = ceil(max(ext, vec2<f32>(1.0)));
    let clip = vec4<f32>(pos.x / size.x * 2.0 - 1.0, 1.0 - pos.y / size.y * 2.0, 0.0, 1.0);
    return AccumOutput(clip, power, freq_hz);
}

@vertex
fn vs_classic(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    return quad_clip((quad_corner(vertex) + vec2<f32>(0.5)) * u.bounds.zw);
}

@vertex
fn vs_resolve(@builtin(vertex_index) vertex: u32) -> ResolveOutput {
    let local = (quad_corner(vertex) + vec2<f32>(0.5)) * u.bounds.zw;
    return ResolveOutput(quad_clip(local), unrotate(local, extents()));
}

fn norm_to_freq(norm: f32) -> f32 {
    let scaled = u.freq_axis.x + norm / u.freq_axis.y;
    switch u.freq_scale {
        case 1u: { return LOG_KNEE_HZ * sinh(scaled); }
        case 2u: { return 228.8 * (pow(10.0, scaled / 21.4) - 1.0); }
        default: { return scaled; }
    }
}

fn unrotate(local: vec2<f32>, ext: vec2<f32>) -> vec2<f32> {
    switch u.rotation {
        case 1u: { return vec2<f32>(local.y, ext.y - local.x); }
        case 2u: { return vec2<f32>(ext.x - local.x, ext.y - local.y); }
        case 3u: { return vec2<f32>(ext.x - local.y, local.x); }
        default: { return local; }
    }
}

fn classic_sample(frag_xy: vec2<f32>) -> vec2<f32> {
    let local = frag_xy - u.bounds.xy;
    let ext = extents();
    let pos = unrotate(local, ext);
    let age_f = floor((ext.x - pos.x) / u.scale_factor);
    if age_f < 0.0 || age_f >= f32(u.col_count) {
        return vec2<f32>(CLASSIC_SENTINEL_DB, 0.0);
    }
    let hl = u.history_length;
    let slot = (u.newest_col + hl - u32(age_f)) % hl;

    let zoomed = 1.0 - pos.y / ext.y;
    let freq_norm = u.uv_y_range.x + zoomed / u.inv_uv_range;
    let freq_hz = norm_to_freq(freq_norm);
    let max_bin = u.points_per_col - 1u;
    let bin_f = freq_hz / u.bin_hz;
    if bin_f > f32(max_bin) {
        return vec2<f32>(CLASSIC_SENTINEL_DB, 0.0);
    }

    let bin0 = u32(floor(bin_f));
    let bin1 = min(bin0 + 1u, max_bin);
    let mag = mix(unpack_mag(slot, bin0), unpack_mag(slot, bin1), fract(bin_f));
    return vec2<f32>(mag, freq_hz);
}

fn shade_db(mag: f32) -> vec4<f32> {
    let range = max(-u.floor_db, 0.001);
    let level = clamp((mag - u.floor_db) / range, 0.0, 1.0);

    // Mix palette stops in sRGB space (web-colors pipeline).
    let color = palette_color(level);

    // iced expects premultiplied alpha
    return vec4<f32>(color.rgb * color.a, color.a);
}

@fragment
fn fs_accum(in: AccumOutput) -> @location(0) vec2<f32> {
    var power = in.power;
    if u.tilt_db != 0.0 && !(power > ANALYSIS_POWER_EPS) {
        return vec2<f32>(0.0);
    }
    if u.tilt_db != 0.0 && in.freq_hz > 0.0 {
        power *= exp2(u.tilt_db * log2(in.freq_hz / 1000.0) * DB_TO_LOG2);
    }
    return vec2<f32>(power, power * LOW_POWER_SCALE);
}

@fragment
fn fs_resolve(in: ResolveOutput) -> @location(0) vec4<f32> {
    let scaled = textureLoad(accum_tex, vec2<i32>(in.accum_pos), 0).rg;
    let scaled_low_power = scaled.g > 0.0 && scaled.g < F16_MAX;
    let power = select(scaled.r, scaled.g * INV_LOW_POWER_SCALE, scaled_low_power)
        * u.reassigned_power_scale;
    if power <= 0.0 {
        return vec4<f32>(0.0);
    }
    return shade_db(max(log(max(power, 1e-20)) * LN_TO_DB, DB_ANALYSIS_FLOOR));
}

@fragment
fn fs_classic(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let sample = classic_sample(position.xy);
    var mag = sample.x;
    // dB/octave tilt relative to 1 kHz. Do not lift sentinels/floor bins.
    if mag < -9000.0 || (u.tilt_db != 0.0 && !(mag > DB_ANALYSIS_FLOOR + DB_FLOOR_EPS)) {
        return vec4<f32>(0.0);
    }
    if u.tilt_db != 0.0 && sample.y > 0.0 {
        mag += u.tilt_db * log2(sample.y / 1000.0);
    }
    return shade_db(mag);
}
