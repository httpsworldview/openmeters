const LOG10_E: f32 = 0.4342944819;

// Classic storage domain — keep in sync with render.rs DB_STORE_*.
const DB_STORE_LO: f32 = -144.0;
const DB_STORE_HI: f32 = 12.0;
const DB_STORE_RANGE: f32 = DB_STORE_HI - DB_STORE_LO;

// Must match Rust-side Uniforms layout exactly.
// freq_min_max.x doubles as FFT bin spacing (sample_rate / fft_size).
struct Uniforms {
    freq_min_max: vec2<f32>,        // (bin_hz, max_hz)
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

    // Precomputed CPU-side; also fills the 12 B of pad before the stops block.
    newest_col: u32,
    inv_uv_range: f32,
    col_stride_u16: u32,

    // (pos1, pos2, pos3, spread0), (spread1, spread2, spread3, spread4).
    // Stops 0 and 4 are constant 0.0 / 1.0
    stops: array<vec4<f32>, 2>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) magnitude_db: f32,
    @location(1) freq_hz: f32,
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var palette_tex: texture_1d<f32>;
@group(0) @binding(2) var<storage, read> mags: array<u32>;

// (Glasberg & Moore 1990)
fn erb(f: f32) -> f32 {
    return 21.4 * log(1.0 + f / 228.8) * LOG10_E;
}

fn freq_to_norm(hz: f32) -> f32 {
    let lo = u.freq_min_max.x;
    let hi = u.freq_min_max.y;

    switch u.freq_scale {
        case 1u: {
            let ln_lo = log(max(lo, 1e-6));
            let ln_hi = log(max(hi, 1e-6));
            return (log(max(hz, 1e-6)) - ln_lo) / max(ln_hi - ln_lo, 1e-12);
        }
        case 2u: {
            let erb_lo = erb(lo);
            let erb_hi = erb(hi);
            return (erb(hz) - erb_lo) / max(erb_hi - erb_lo, 1e-12);
        }
        default: {
            return (hz - lo) / max(hi - lo, 1e-12);
        }
    }
}

struct PaletteSegment {
    lo: i32,
    hi: i32,
    f: f32,
}

fn find_segment(t: f32) -> PaletteSegment {
    let tc = clamp(t, 0.0, 1.0);
    let positions = array<f32, 5>(0.0, u.stops[0].x, u.stops[0].y, u.stops[0].z, 1.0);
    let spreads = array<f32, 5>(u.stops[0].w, u.stops[1].x, u.stops[1].y, u.stops[1].z, u.stops[1].w);
    var lo: i32 = 3;
    var hi: i32 = 4;
    var linear_t: f32 = 1.0;
    for (var i: i32 = 0; i < 4; i = i + 1) {
        let p_hi = positions[i + 1];
        if (tc <= p_hi) {
            let p_lo = positions[i];
            let span = max(p_hi - p_lo, 1e-6);
            lo = i;
            hi = i + 1;
            linear_t = clamp((tc - p_lo) / span, 0.0, 1.0);
            break;
        }
    }
    let sl = spreads[lo];
    let sr = spreads[hi];
    var f: f32;
    if (abs(sl - 1.0) < 1e-4 && abs(sr - 1.0) < 1e-4) {
        f = linear_t;
    } else {
        f = clamp(pow(linear_t, sl / sr), 0.0, 1.0);
    }
    return PaletteSegment(lo, hi, f);
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
fn vs_strip(
    @location(0) corner: vec2<f32>,
    @builtin(instance_index) inst: u32,
) -> VertexOutput {
    // Instance count = col_count * (points_per_col - 1); instance i encodes
    // (slot, bin_in_col) where bin_in_col is the lower of the two segment bins.
    let segs_per_col = max(u.points_per_col, 1u) - 1u;
    let slot = inst / max(segs_per_col, 1u);
    let bin_in_col = inst % max(segs_per_col, 1u);

    // Compute both endpoint freq positions so cull decisions are uniform across quad
    let bin_hz = u.freq_min_max.x;
    let zoomed_lo = (freq_to_norm(f32(bin_in_col) * bin_hz) - u.uv_y_range.x) * u.inv_uv_range;
    let zoomed_hi = (freq_to_norm(f32(bin_in_col + 1u) * bin_hz) - u.uv_y_range.x) * u.inv_uv_range;
    if max(zoomed_lo, zoomed_hi) < -0.01 || min(zoomed_lo, zoomed_hi) > 1.01 {
        return VertexOutput(CULL_POS, u.floor_db, 0.0);
    }

    // corner.y > 0 -> lower-freq edge
    // note: not exact. off by a fraction of a dB
    let use_lo = corner.y > 0.0;
    let bin_idx = select(bin_in_col + 1u, bin_in_col, use_lo);
    let mag_db = unpack_mag(slot, bin_idx);
    let freq_hz = f32(bin_idx) * bin_hz;
    let ext = extents();
    let zoomed = select(zoomed_hi, zoomed_lo, use_lo);
    // corner padding: 1 px time + 1 px freq so subpixel bins stay visible at high freq.
    let pos = vec2<f32>(ext.x - f32(compute_age(slot)) * u.scale_factor, (1.0 - zoomed) * ext.y)
        + corner * u.scale_factor;
    return VertexOutput(place(pos, ext), mag_db, freq_hz);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var mag = in.magnitude_db;

    // dB/decade tilt relative to 1 kHz
    if u.tilt_db != 0.0 && in.freq_hz > 0.0 {
        mag += u.tilt_db * log(in.freq_hz / 1000.0) * LOG10_E;
    }

    let range = max(u.ceiling_db - u.floor_db, 0.001);
    let normalized = clamp((mag - u.floor_db) / range, 0.0, 1.0);
    let adjusted = pow(normalized, max(u.contrast, 0.01));

    // Rgba8Unorm palette: raw sRGB stops, mix in sRGB space (web-colors pipeline).
    let seg = find_segment(adjusted);
    let stop_lo = textureLoad(palette_tex, seg.lo, 0);
    let stop_hi = textureLoad(palette_tex, seg.hi, 0);
    let color = mix(stop_lo, stop_hi, seg.f);

    // iced expects premultiplied alpha
    return vec4<f32>(color.rgb * color.a, color.a);
}
