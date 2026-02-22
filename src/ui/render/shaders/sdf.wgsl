// SDF shader - antialiasing via screen-space derivatives.
//
// params: [dist_x, dist_y, radius, _unused]
// - Solid: (0, 0, large, 0)
// - Line: (+-outer, 0, half_width, 0)
// - Dot: (ox, oy, radius, 0)

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) params: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) params: vec4<f32>,
};

fn premultiply(color: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(color.rgb * color.a, color.a);
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4<f32>(input.position, 0.0, 1.0);
    output.color = premultiply(input.color);
    output.params = input.params;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let d = input.params.xy;
    let dist = length(d);
    let radius = input.params.z;
    let aa = max(length(vec2<f32>(fwidth(d.x), fwidth(d.y))), 1e-4);
    let coverage = clamp((radius - dist) / aa + 0.5, 0.0, 1.0);
    return input.color * coverage;
}
