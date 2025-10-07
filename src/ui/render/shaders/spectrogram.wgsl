struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) tex_coords: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
};

struct SpectrogramUniforms {
    dimensions: vec2<f32>,
    latest_column: u32,
    column_count: u32,
    background: vec4<f32>,
    palette: array<vec4<f32>, 5>,
};

struct MagnitudeBuffer {
    values: array<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: SpectrogramUniforms;
@group(0) @binding(1) var<storage, read> magnitudes: MagnitudeBuffer;

fn sample_palette(value: f32) -> vec4<f32> {
    let clamped = clamp(value, 0.0, 1.0);
    let segments = 5u - 1u;
    let scaled = clamped * f32(segments);
    let index = min(u32(scaled), segments);
    let next = min(index + 1u, segments);
    let frac = scaled - f32(index);
    let start = uniforms.palette[index];
    let stop = uniforms.palette[next];
    return start + (stop - start) * frac;
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
    let capacity = u32(uniforms.dimensions.x);
    let height = u32(uniforms.dimensions.y);
    let count = uniforms.column_count;

    if capacity == 0u || height == 0u || count == 0u {
        return uniforms.background;
    }

    let clamped_uv = clamp(input.tex_coords, vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0));
    let logical_width = count;
    let latest = min(uniforms.latest_column, capacity - 1u);

    var x_index: u32 = 0u;
    if logical_width > 1u {
        x_index = min(
            u32(clamped_uv.x * f32(logical_width - 1u) + 0.5),
            logical_width - 1u,
        );
    }

    var oldest: u32 = 0u;
    if count == capacity {
        oldest = (latest + 1u) % capacity;
    }

    var physical: u32 = x_index;
    if count == capacity {
        physical = (oldest + x_index) % capacity;
    }

    var row: u32 = 0u;
    if height > 1u {
        row = min(u32(clamped_uv.y * f32(height - 1u) + 0.5), height - 1u);
    }

    let index = physical * height + row;
    let value = magnitudes.values[index];

    if value <= 0.0 {
        return uniforms.background;
    }

    return sample_palette(value);
}
