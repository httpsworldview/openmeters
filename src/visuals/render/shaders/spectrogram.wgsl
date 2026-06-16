const LOG10_E: f32 = 0.4342944819;
const LOG_KNEE_HZ: f32 = 20.0;

// Classic storage domain -- keep in sync with processor.rs CLASSIC_DB_STORE_*.
const DB_STORE_LO: f32 = -144.0;
const DB_STORE_HI: f32 = 12.0;
const DB_STORE_RANGE: f32 = DB_STORE_HI - DB_STORE_LO;

// Analysis floor -- keep in sync with util::audio::DB_FLOOR.
const DB_ANALYSIS_FLOOR: f32 = -140.0;
const DB_FLOOR_EPS: f32 = 0.01;

// Must match Rust-side Uniforms layout exactly.
struct Uniforms {
    freq_axis: vec2<f32>,           // (scaled_min, inverse scaled display span)
    freq_scale: u32,                // 0=linear, 1=log, 2=erb
    points_per_col: u32,

    history_length: u32,
    col_count: u32,
    write_slot: u32,
    rotation: u32,

    bounds: vec4<f32>,              // (x, y, w, h) logical pixels
    clip_scale: vec2<f32>,          // (2/viewport_w, 2/viewport_h)
    uv_y_range: vec2<f32>,          // zoom/pan window into [0,1] freq axis
    scale_factor: f32,

    floor_db: f32,
    ceiling_db: f32,
    contrast: f32,
    tilt_db: f32,

    newest_col: u32,
    inv_uv_range: f32,
    col_stride_u16: u32,
    // FFT bin spacing (sample_rate / fft_size); only used by classic sampling.
    bin_hz: f32,
    // Three u32 pads (vec3 would align to 16 B and desync the Rust layout).
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,

    // (pos1, pos2, pos3, spread0), (spread1, spread2, spread3, spread4).
    // Stops 0 and 4 are constant 0.0 / 1.0
    stops: array<vec4<f32>, 2>,
    // Quantized sRGB stops, matching the old Rgba8Unorm texture path.
    palette: array<vec4<f32>, 5>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) magnitude_db: f32,
    @location(1) freq_hz: f32,
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(2) var<storage, read> mags: array<u32>;

// (Glasberg & Moore 1990)
fn erb(f: f32) -> f32 {
    return 21.4 * log(1.0 + f / 228.8) * LOG10_E;
}

fn freq_to_norm(hz: f32) -> f32 {
    var scaled: f32;
    switch u.freq_scale {
        case 1u: { scaled = asinh(hz / LOG_KNEE_HZ); }
        case 2u: { scaled = erb(hz); }
        default: { scaled = hz; }
    }
    return (scaled - u.freq_axis.x) * u.freq_axis.y;
}

fn spread_t(linear_t: f32, sl: f32, sr: f32) -> f32 {
    if (abs(sl - 1.0) < 1e-4 && abs(sr - 1.0) < 1e-4) {
        return linear_t;
    }
    return clamp(pow(linear_t, sl / sr), 0.0, 1.0);
}

fn palette_color(t: f32) -> vec4<f32> {
    let tc = clamp(t, 0.0, 1.0);
    var lo = u.palette[3];
    var hi = u.palette[4];
    var p_lo = u.stops[0].z;
    var p_hi = 1.0;
    var sl = u.stops[1].z;
    var sr = u.stops[1].w;
    if (tc <= u.stops[0].x) {
        lo = u.palette[0];
        hi = u.palette[1];
        p_lo = 0.0;
        p_hi = u.stops[0].x;
        sl = u.stops[0].w;
        sr = u.stops[1].x;
    } else if (tc <= u.stops[0].y) {
        lo = u.palette[1];
        hi = u.palette[2];
        p_lo = u.stops[0].x;
        p_hi = u.stops[0].y;
        sl = u.stops[1].x;
        sr = u.stops[1].y;
    } else if (tc <= u.stops[0].z) {
        lo = u.palette[2];
        hi = u.palette[3];
        p_lo = u.stops[0].y;
        p_hi = u.stops[0].z;
        sl = u.stops[1].y;
        sr = u.stops[1].z;
    }
    let linear_t = clamp((tc - p_lo) / max(p_hi - p_lo, 1e-6), 0.0, 1.0);
    return mix(lo, hi, spread_t(linear_t, sl, sr));
}

// 0 = newest. Single formula handles both partial and full rings via newest_col.
fn compute_age(slot: u32) -> u32 {
    let hl = max(u.history_length, 1u);
    return (u.newest_col + hl - slot) % hl;
}

fn extents() -> vec2<f32> {
    let swapped = u.rotation == 1u || u.rotation == 3u;
    return vec2<f32>(
        select(u.bounds.z, u.bounds.w, swapped),
        select(u.bounds.w, u.bounds.z, swapped),
    );
}

// Pre-rotation (time, freq) pos -> clip-space NDC under u.rotation, u.bounds.
fn place(pos: vec2<f32>, ext: vec2<f32>) -> vec4<f32> {
    var rotated: vec2<f32>;
    switch u.rotation {
        case 1u: { rotated = vec2<f32>(ext.y - pos.y, pos.x); }
        case 2u: { rotated = vec2<f32>(ext.x - pos.x, ext.y - pos.y); }
        case 3u: { rotated = vec2<f32>(pos.y, ext.x - pos.x); }
        default: { rotated = pos; }
    }
    rotated += u.bounds.xy;
    return vec4<f32>(rotated.x * u.clip_scale.x - 1.0, 1.0 - rotated.y * u.clip_scale.y, 0.0, 1.0);
}

// `col_stride_u16` rounds ppc up to even so u16 pairs never straddle u32 words.
fn unpack_mag(slot: u32, bin_in_col: u32) -> f32 {
    let idx = slot * u.col_stride_u16 + bin_in_col;
    let pair = unpack2x16unorm(mags[idx / 2u]);
    return select(pair.y, pair.x, (idx & 1u) == 0u) * DB_STORE_RANGE + DB_STORE_LO;
}

const CULL_POS: vec4<f32> = vec4<f32>(0.0, 0.0, 2.0, 1.0);
const CLASSIC_SENTINEL_DB: f32 = -10000.0;

@vertex
fn vs_splat(
    @location(0) corner: vec2<f32>,
    @location(1) time_offset: f32,
    @location(2) freq_hz: f32,
    @location(3) magnitude_db: f32,
    @builtin(instance_index) inst: u32,
) -> VertexOutput {
    let zoomed = (freq_to_norm(freq_hz) - u.uv_y_range.x) * u.inv_uv_range;
    if magnitude_db < -900.0 || zoomed < -0.01 || zoomed > 1.01 {
        return VertexOutput(CULL_POS, magnitude_db, freq_hz);
    }
    let ext = extents();
    let age = compute_age(inst / max(u.points_per_col, 1u));
    let pos = vec2<f32>(ext.x - (f32(age) - time_offset) * u.scale_factor, (1.0 - zoomed) * ext.y)
        + corner * u.scale_factor;
    return VertexOutput(place(pos, ext), magnitude_db, freq_hz);
}

@vertex
fn vs_classic(@location(0) corner: vec2<f32>) -> VertexOutput {
    let px = u.bounds.xy + (corner + vec2<f32>(0.5)) * u.bounds.zw;
    let clip = vec4<f32>(px.x * u.clip_scale.x - 1.0, 1.0 - px.y * u.clip_scale.y, 0.0, 1.0);
    return VertexOutput(clip, CLASSIC_SENTINEL_DB, 0.0);
}

fn norm_to_freq(norm: f32) -> f32 {
    let scaled = u.freq_axis.x + norm / max(u.freq_axis.y, 1e-12);
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
    if local.x < 0.0 || local.y < 0.0 || local.x > u.bounds.z || local.y > u.bounds.w {
        return vec2<f32>(CLASSIC_SENTINEL_DB, 0.0);
    }

    let ext = extents();
    let pos = unrotate(local, ext);
    if pos.x < 0.0 || pos.y < 0.0 || pos.x > ext.x || pos.y > ext.y {
        return vec2<f32>(CLASSIC_SENTINEL_DB, 0.0);
    }

    let age_f = floor((ext.x - pos.x) / max(u.scale_factor, 1e-6));
    if age_f < 0.0 || age_f >= f32(u.col_count) {
        return vec2<f32>(CLASSIC_SENTINEL_DB, 0.0);
    }
    let hl = max(u.history_length, 1u);
    let slot = (u.newest_col + hl - u32(age_f)) % hl;

    let zoomed = 1.0 - pos.y / max(ext.y, 1.0);
    if zoomed < 0.0 || zoomed > 1.0 {
        return vec2<f32>(CLASSIC_SENTINEL_DB, 0.0);
    }
    let freq_norm = u.uv_y_range.x + zoomed / u.inv_uv_range;
    let freq_hz = norm_to_freq(freq_norm);
    let max_bin = max(u.points_per_col, 1u) - 1u;
    let bin_f = freq_hz / max(u.bin_hz, 1e-12);
    if bin_f < 0.0 || bin_f > f32(max_bin) {
        return vec2<f32>(CLASSIC_SENTINEL_DB, 0.0);
    }

    let bin0 = min(u32(floor(bin_f)), max_bin);
    let bin1 = min(bin0 + 1u, max_bin);
    let mag = mix(unpack_mag(slot, bin0), unpack_mag(slot, bin1), fract(bin_f));
    return vec2<f32>(mag, freq_hz);
}

fn shade(mut_mag: f32, freq_hz: f32) -> vec4<f32> {
    var mag = mut_mag;

    // dB/octave tilt relative to 1 kHz. Do not lift sentinels/floor bins.
    if u.tilt_db != 0.0 {
        if !(mag > DB_ANALYSIS_FLOOR + DB_FLOOR_EPS) {
            return vec4<f32>(0.0);
        }
        if freq_hz > 0.0 {
            mag += u.tilt_db * log2(freq_hz / 1000.0);
        }
    }

    let range = max(u.ceiling_db - u.floor_db, 0.001);
    let normalized = clamp((mag - u.floor_db) / range, 0.0, 1.0);
    let adjusted = pow(normalized, max(u.contrast, 0.01));

    // Quantized sRGB stops, mix in sRGB space (web-colors pipeline).
    let color = palette_color(adjusted);

    // iced expects premultiplied alpha
    return vec4<f32>(color.rgb * color.a, color.a);
}

@fragment
fn fs_splat(in: VertexOutput) -> @location(0) vec4<f32> {
    return shade(in.magnitude_db, in.freq_hz);
}

@fragment
fn fs_classic(in: VertexOutput) -> @location(0) vec4<f32> {
    let sample = classic_sample(in.position.xy);
    if sample.x < -9000.0 {
        return vec4<f32>(0.0);
    }
    return shade(sample.x, sample.y);
}
