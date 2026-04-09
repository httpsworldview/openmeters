const LOG10_E: f32 = 0.4342944819;

// Must match Rust-side Uniforms layout exactly.
struct Uniforms {
    freq_min_max: vec2<f32>,        // (min_hz, max_hz)
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
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) magnitude_db: f32,
    @location(1) freq_hz: f32,
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var palette_tex: texture_1d<f32>;
@group(0) @binding(2) var palette_sampler: sampler;

// ERB-rate: 21.4 * log10(1 + f/228.8)  (Glasberg & Moore 1990)
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

@vertex
fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) time_offset: f32,
    @location(2) freq_hz: f32,
    @location(3) magnitude_db: f32,
    @builtin(instance_index) inst: u32,
) -> VertexOutput {
    var out: VertexOutput;

    // Sentinel culling - degenerate vertex behind clip volume
    if magnitude_db < -900.0 {
        out.position = vec4<f32>(0.0, 0.0, 2.0, 1.0);
        out.magnitude_db = magnitude_db;
        out.freq_hz = freq_hz;
        return out;
    }

    let slot = inst / max(u.points_per_col, 1u);

    var age: u32; // 0 = newest
    if u.col_count == u.history_length {
        let newest = (u.write_slot + u.history_length - 1u) % max(u.history_length, 1u);
        age = (newest - slot + u.history_length) % max(u.history_length, 1u);
    } else {
        age = u.col_count - 1u - slot;
    }

    // Rotations 1/3 swap time and freq screen axes
    let swapped = u.rotation == 1u || u.rotation == 3u;
    let time_extent = select(u.bounds.z, u.bounds.w, swapped);
    let freq_extent = select(u.bounds.w, u.bounds.z, swapped);

    // Newest column at right edge
    let time_logical = time_extent - (f32(age) - time_offset) * u.scale_factor;

    let norm = freq_to_norm(freq_hz);

    let uv_range = max(u.uv_y_range.y - u.uv_y_range.x, 1e-12);
    let zoomed = (norm - u.uv_y_range.x) / uv_range;

    if zoomed < -0.01 || zoomed > 1.01 {
        out.position = vec4<f32>(0.0, 0.0, 2.0, 1.0);
        out.magnitude_db = magnitude_db;
        out.freq_hz = freq_hz;
        return out;
    }

    // High frequencies at top
    let freq_logical = (1.0 - zoomed) * freq_extent;

    var pos = vec2<f32>(time_logical, freq_logical) + corner * u.scale_factor;

    // pos.x = time axis, pos.y = freq axis (pre-rotation)
    // 0: time L->R, freq bottom->top
    // 1: time T->B, freq R->L
    // 2: time R->L, freq T->B
    // 3: time B->T, freq L->R
    var rotated: vec2<f32>;
    switch u.rotation {
        case 1u: {
            rotated = vec2<f32>(freq_extent - pos.y, pos.x) + u.bounds.xy;
        }
        case 2u: {
            rotated = vec2<f32>(time_extent - pos.x, freq_extent - pos.y) + u.bounds.xy;
        }
        case 3u: {
            rotated = vec2<f32>(pos.y, time_extent - pos.x) + u.bounds.xy;
        }
        default: {
            rotated = vec2<f32>(pos.x, pos.y) + u.bounds.xy;
        }
    }

    // Logical pixels -> NDC; y flipped because screen y points down
    out.position = vec4<f32>(
        rotated.x * u.clip_scale.x - 1.0,
        1.0 - rotated.y * u.clip_scale.y,
        0.0,
        1.0,
    );
    out.magnitude_db = magnitude_db;
    out.freq_hz = freq_hz;

    return out;
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
    let color = textureSampleLevel(palette_tex, palette_sampler, adjusted, 0.0);

    // Premultiplied alpha required by iced
    return vec4<f32>(color.rgb * color.a, color.a);
}
