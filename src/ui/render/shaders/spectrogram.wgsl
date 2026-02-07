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
}

struct MagnitudeParams {
    capacity: u32,
    wrap_mask: u32,
    oldest: u32,
    is_pow2: bool,
    is_full: bool,
}

const FLAG_CAPACITY_POW2: u32 = 0x1u;
const MAX_X_SAMPLES: u32 = 16u;

@group(0) @binding(0)
var<uniform> uniforms: SpectrogramUniforms;
@group(0) @binding(1)
var magnitudes: texture_2d<f32>;
@group(0) @binding(2)
var palette_tex: texture_1d<f32>;
@group(0) @binding(3)
var palette_sampler: sampler;

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

fn sample_magnitude(logical: u32, row: u32, params: MagnitudeParams) -> f32 {
    let physical = logical_to_physical(logical, params);
    return textureLoad(magnitudes, vec2<i32>(i32(row), i32(physical)), 0).x;
}

fn max_magnitude_for_column(logical: u32, row_lo: u32, row_hi: u32, params: MagnitudeParams) -> f32 {
    var val = sample_magnitude(logical, row_lo, params);
    for (var r = row_lo + 1u; r < min(row_hi, row_lo + 64u); r = r + 1u) {
        val = max(val, sample_magnitude(logical, r, params));
    }
    return val;
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

    let clamped_uv = clamp(input.tex_coords, vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0));
    let scroll_phase = bitcast<f32>(state.z);
    let screen_width = max(bitcast<f32>(state.w), 1.0);

    let x_pos = clamped_uv.x * f32(count - 1u) + scroll_phase;
    let x_lo = u32(max(floor(x_pos), 0.0));
    let x_hi = min(x_lo + 1u, count - 1u);
    let x_frac = fract(x_pos);
    let x_center = u32(clamp(floor(x_pos + 0.5), 0.0, f32(count - 1u)));

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
    let screen_height = max(uniforms.style.w, 1.0);
    let zoomed_y = uv_y_min + clamped_uv.y * (uv_y_max - uv_y_min);

    let params = MagnitudeParams(capacity, wrap_mask, oldest, is_pow2, is_full);

    // Max-pool across bins that map to this pixel to preserve peaks when downsampling
    let bins_per_pixel = f32(height) * (uv_y_max - uv_y_min) / screen_height;
    let half_span = bins_per_pixel * 0.5;
    let center_bin = zoomed_y * f32(height - 1u);
    let row_lo = u32(max(center_bin - half_span, 0.0));
    let row_hi = min(u32(center_bin + half_span) + 1u, height);

    let columns_per_pixel = f32(count) / screen_width;
    var center = 0.0;
    if columns_per_pixel <= 1.0 {
        let val_lo = max_magnitude_for_column(x_lo, row_lo, row_hi, params);
        let val_hi = max_magnitude_for_column(x_hi, row_lo, row_hi, params);
        center = mix(val_lo, val_hi, x_frac);
    } else {
        let half_cols = columns_per_pixel * 0.5;
        let col_lo = u32(max(x_pos - half_cols, 0.0));
        let col_hi = min(u32(x_pos + half_cols + 1.0), count);
        let span = max(col_hi - col_lo, 1u);
        let step = max(span / MAX_X_SAMPLES, 1u);
        var col = col_lo;
        for (var i = 0u; i < MAX_X_SAMPLES; i = i + 1u) {
            if col >= col_hi {
                break;
            }
            center = max(center, max_magnitude_for_column(col, row_lo, row_hi, params));
            col = col + step;
        }
    }
    let row = u32(clamp(center_bin + 0.5, 0.0, f32(height - 1u)));

    return sample_palette(center);
}
