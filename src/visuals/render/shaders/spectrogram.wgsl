struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) tex_coords: vec2<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
}

struct SpectrogramUniforms {
    dims_wrap_flags: vec4<f32>,
    latest_and_count: vec4<u32>,
    style: vec4<f32>,
    background: vec4<f32>,
    floor_ceil: vec4<f32>,
}

struct MagnitudeParams {
    capacity: u32,
    wrap_mask: u32,
    oldest: u32,
    is_pow2: bool,
    is_full: bool,
}

const FLAG_CAPACITY_POW2: u32 = 0x1u;

@group(0) @binding(0)
var<uniform> uniforms: SpectrogramUniforms;
@group(0) @binding(1)
var magnitudes: texture_2d<f32>;
@group(0) @binding(2)
var palette_tex: texture_1d<f32>;
@group(0) @binding(3)
var palette_sampler: sampler;
@group(0) @binding(4)
var tilt_tex: texture_1d<f32>;

// Premultiply alpha to match iced's color pipeline
fn premultiply(color: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(color.rgb * color.a, color.a);
}

fn sample_palette(value: f32) -> vec4<f32> {
    let clamped = clamp(value, 0.0, 1.0);
    let contrast = max(uniforms.style.x, 0.01);
    let adjusted = pow(clamped, contrast);
    let color = textureSampleLevel(palette_tex, palette_sampler, adjusted, 0.0);
    return premultiply(color);
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4<f32>(input.position, 0.0, 1.0);
    output.tex_coords = input.tex_coords;
    return output;
}

fn logical_to_physical(logical: u32, params: MagnitudeParams) -> u32 {
    if params.is_full {
        if params.is_pow2 {
            return (params.oldest + logical) & params.wrap_mask;
        }
        return (params.oldest + logical) % params.capacity;
    }
    return logical;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let dims = uniforms.dims_wrap_flags;
    let capacity = u32(dims.x);
    let height = u32(dims.y);
    let wrap_mask = bitcast<u32>(dims.z);
    let flags = bitcast<u32>(dims.w);

    let state = uniforms.latest_and_count;
    let count = state.y;

    if capacity == 0u || height == 0u || count == 0u {
        return premultiply(uniforms.background);
    }

    let latest = min(state.x, capacity - 1u);
    let rotation = state.z;

    let is_pow2 = (flags & FLAG_CAPACITY_POW2) != 0u;
    let is_full = count == capacity;

    var oldest = 0u;
    if is_full {
        let next = latest + 1u;
        if is_pow2 {
            oldest = next & wrap_mask;
        }
        else {
            oldest = next % capacity;
        }
    }

    let uv_y_min = uniforms.style.y;
    let uv_y_max = uniforms.style.z;

    let params = MagnitudeParams(capacity, wrap_mask, oldest, is_pow2, is_full);

    let origin = vec2<f32>(bitcast<f32>(state.w), uniforms.style.w);
    let sf = max(uniforms.floor_ceil.w, 1.0);
    let local = (floor(input.position.xy) - origin) / sf;

    var time_f: f32;
    var freq_f: f32;
    switch rotation {
        case 1u: {
            time_f = local.y;
            freq_f = f32(height - 1u) - local.x;
        }
        case 2u: {
            time_f = f32(capacity - 1u) - local.x;
            freq_f = f32(height - 1u) - local.y;
        }
        case 3u: {
            time_f = f32(capacity - 1u) - local.y;
            freq_f = local.x;
        }
        default: {
            time_f = local.x;
            freq_f = local.y;
        }
    }

    let time_px = u32(clamp(time_f, 0.0, f32(capacity - 1u)));
    let freq_px = u32(clamp(freq_f, 0.0, f32(height - 1u)));

    let empty = capacity - count;
    if !is_full && time_px < empty {
        return premultiply(uniforms.background);
    }

    var col: u32;
    if is_full {
        col = time_px;
    } else {
        col = time_px - empty;
    }

    var row: u32;
    if uv_y_min == 0.0 && uv_y_max == 1.0 {
        row = freq_px;
    } else {
        let freq_frac = f32(freq_px) / max(f32(height - 1u), 1.0);
        let tex_v = uv_y_min + freq_frac * (uv_y_max - uv_y_min);
        row = u32(clamp(tex_v * f32(height - 1u), 0.0, f32(height - 1u)));
    }

    let physical = logical_to_physical(col, params);
    var magnitude = textureLoad(magnitudes, vec2<i32>(i32(row), i32(physical)), 0).x;

    let tf = uniforms.floor_ceil.z;
    if tf != 0.0 {
        magnitude = magnitude + textureLoad(tilt_tex, i32(row), 0).x * tf;
    }

    let floor_db = uniforms.floor_ceil.x;
    let range = max(uniforms.floor_ceil.y - floor_db, 0.001);
    return sample_palette(clamp((magnitude - floor_db) / range, 0.0, 1.0));
}
