struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) tex_coords: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
};

struct SpectrogramUniforms {
    dims_wrap_flags: vec4<f32>,
    latest_and_count: vec4<u32>,
    style: vec4<f32>,
    background: vec4<f32>,
};

const FLAG_CAPACITY_POW2: u32 = 0x1u;

@group(0) @binding(0) var<uniform> uniforms: SpectrogramUniforms;
@group(0) @binding(1) var magnitudes: texture_2d<f32>;
@group(0) @binding(2) var palette_tex: texture_1d<f32>;
@group(0) @binding(3) var palette_sampler: sampler;

fn sample_palette(value: f32) -> vec4<f32> {
    let clamped = clamp(value, 0.0, 1.0);
    let contrast = max(uniforms.style.x, 0.01);
    let adjusted = pow(clamped, contrast);
    return textureSampleLevel(palette_tex, palette_sampler, adjusted, 0.0);
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4<f32>(input.position, 0.0, 1.0);
    output.tex_coords = input.tex_coords;
    return output;
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
        return uniforms.background;
    }

    let latest = min(state.x, capacity - 1u);

    let clamped_uv = clamp(input.tex_coords, vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0));
    let logical_width = count;

    var x_index: u32 = 0u;
    if logical_width > 1u {
        x_index = min(
            u32(clamped_uv.x * f32(logical_width - 1u) + 0.5),
            logical_width - 1u,
        );
    }

    var oldest: u32 = 0u;
    if count == capacity {
        if (flags & FLAG_CAPACITY_POW2) != 0u {
            oldest = (latest + 1u) & wrap_mask;
        } else {
            oldest = (latest + 1u) % capacity;
        }
    }

    var physical: u32 = x_index;
    if count == capacity {
        if (flags & FLAG_CAPACITY_POW2) != 0u {
            physical = (oldest + x_index) & wrap_mask;
        } else {
            physical = (oldest + x_index) % capacity;
        }
    }

    var row: u32 = 0u;
    if height > 1u {
        row = min(u32(clamped_uv.y * f32(height - 1u) + 0.5), height - 1u);
    }

    let sample = textureLoad(
        magnitudes,
        vec2<i32>(i32(row), i32(physical)),
        0,
    );
    return sample_palette(sample.x);
}
