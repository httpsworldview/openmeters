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

// Max-pool across the bins that map to this pixel's exclusive grid cell
fn peak_in_range(logical: u32, row_lo: u32, row_hi: u32, params: MagnitudeParams) -> f32 {
    var val = sample_magnitude(logical, row_lo, params);
    for (var r = row_lo + 1u; r < row_hi; r = r + 1u) {
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
    let screen_width = max(bitcast<f32>(state.w), 1.0);

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

    let params = MagnitudeParams(capacity, wrap_mask, oldest, is_pow2, is_full);

    let pixel_x = floor(clamped_uv.x * screen_width);
    let pixel_y = floor(clamped_uv.y * screen_height);

    let y_frac_lo = pixel_y / screen_height;
    let y_frac_hi = (pixel_y + 1.0) / screen_height;
    let bin_lo = (uv_y_min + y_frac_lo * (uv_y_max - uv_y_min)) * f32(height - 1u);
    let bin_hi = (uv_y_min + y_frac_hi * (uv_y_max - uv_y_min)) * f32(height - 1u);
    let row_lo = u32(max(floor(bin_lo), 0.0));
    let row_hi = min(u32(ceil(bin_hi)) + 1u, height);

    // column arrivals, then advances by whole columns. mostly eliminates
    // sub-pixel jitter, but it isn't perfect, and I'm out of ideas for
    // how to do better.
    let x_lo_f = pixel_x / screen_width * f32(count);
    let x_hi_f = (pixel_x + 1.0) / screen_width * f32(count);
    let col_lo = u32(clamp(floor(x_lo_f), 0.0, f32(count - 1u)));
    let col_hi = u32(clamp(ceil(x_hi_f), 0.0, f32(count)));
    let col_end = max(col_hi, col_lo + 1u);

    var magnitude = peak_in_range(col_lo, row_lo, row_hi, params);
    for (var c = col_lo + 1u; c < col_end; c = c + 1u) {
        magnitude = max(magnitude, peak_in_range(c, row_lo, row_hi, params));
    }

    return sample_palette(magnitude);
}
