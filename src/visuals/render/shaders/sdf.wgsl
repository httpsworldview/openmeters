// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Maika Namuo

struct InstanceInput {
    @location(0) p0: vec2<f32>,
    @location(1) p1: vec2<f32>,
    @location(2) color0: vec4<f32>,
    @location(3) color1: vec4<f32>,
    @location(4) params: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) params: vec4<f32>,
};

@vertex
fn vs_main(input: InstanceInput, @builtin(vertex_index) vertex: u32) -> VertexOutput {
    let endpoint = f32(vertex / 2u);
    let parity = f32(vertex % 2u);
    var position = input.p0;
    var color = input.color0;
    var sdf = vec4<f32>(0.0, 0.0, 1000.0, 0.0);

    switch u32(input.params.w) {
        case 0u: {
            let corner = vec2<f32>(endpoint, 1.0 - parity);
            position = mix(input.p0, input.p1, corner);
            color = mix(input.color0, input.color1, corner.y);
            sdf.w = input.params.x; // Preserve raw color for replacement blending.
        }
        case 1u: {
            let top_y = mix(max(input.p0.y, input.params.x), max(input.p1.y, input.params.x), endpoint);
            let bottom_y = mix(min(input.p0.y, input.params.x), min(input.p1.y, input.params.x), endpoint);
            position = vec2<f32>(mix(input.p0.x, input.p1.x, endpoint), mix(top_y, bottom_y, 1.0 - parity));
            color = mix(input.color0, input.color1, endpoint);
        }
        case 2u: {
            let side = 1.0 - 2.0 * parity;
            position = mix(input.p0, input.p1, endpoint) + input.params.xy * side;
            color = mix(input.color0, input.color1, endpoint);
            sdf = vec4<f32>(side * (input.params.z + 1.0), 0.0, input.params.z, 0.0);
        }
        case 4u: {
            let corner = vec2<f32>(endpoint, parity) * 2.0 - 1.0;
            var point = input.p0;
            if input.params.z < 0.0 {
                let squared = dot(point, point);
                if squared < 1.4210855e-14 {
                    point = vec2<f32>(0.0);
                } else if squared < 2.2387474 {
                    point *= 0.8861337 * pow(squared, -0.35);
                } else {
                    point *= inverseSqrt(squared);
                }
            } else {
                point *= input.params.z;
            }
            position = input.color1.xy + point * input.color1.zw
                + input.p1 * corner * (input.params.x + 1.0);
            sdf = vec4<f32>(corner * (input.params.x + 1.0), input.params.x, input.params.y);
        }
        default: {
            let corner = vec2<f32>(endpoint, parity) * 2.0 - 1.0;
            position = input.p0 + input.p1 * corner;
            sdf = vec4<f32>(corner * (input.params.x + 1.0), input.params.x, input.params.y);
        }
    }

    var output: VertexOutput;
    output.position = vec4<f32>(position, 0.0, 1.0);
    output.color = select(vec4<f32>(color.rgb * color.a, color.a), color, sdf.w > 0.5);
    output.params = sdf;
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
