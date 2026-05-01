/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 6 separable Gaussian blur.
//!
//! Applies a 5-tap kernel [1/16, 4/16, 6/16, 4/16, 1/16] along the
//! axis specified by `params.step`. Call twice (horizontal then vertical)
//! for a full 2-D blur. `step` is a texel-space direction vector:
//!   horizontal pass: step = vec2(1.0 / width,  0.0)
//!   vertical   pass: step = vec2(0.0,          1.0 / height)
//!
//! No vertex buffer required — the fullscreen quad is generated from
//! `vertex_index` 0..3 (TriangleStrip).

struct BlurParams {
    step: vec2<f32>,
    _pad: vec2<f32>,
}

@group(0) @binding(0) var input_texture: texture_2d<f32>;
@group(0) @binding(1) var input_sampler: sampler;
@group(0) @binding(2) var<uniform> params: BlurParams;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// Fullscreen quad via TriangleStrip from vertex_index 0..4.
// NDC y-up: (-1,-1) = bottom-left. UV y-down: (0,0) = top-left.
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0),  // bottom-left
        vec2<f32>( 1.0, -1.0),  // bottom-right
        vec2<f32>(-1.0,  1.0),  // top-left
        vec2<f32>( 1.0,  1.0),  // top-right
    );
    var uvs = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 1.0),    // bottom-left in UV
        vec2<f32>(1.0, 1.0),    // bottom-right in UV
        vec2<f32>(0.0, 0.0),    // top-left in UV
        vec2<f32>(1.0, 0.0),    // top-right in UV
    );
    var out: VertexOutput;
    out.position = vec4<f32>(positions[vi], 0.0, 1.0);
    out.uv = uvs[vi];
    return out;
}

const WEIGHTS: array<f32, 5> = array<f32, 5>(0.0625, 0.25, 0.375, 0.25, 0.0625);

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var color = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    for (var i: i32 = -2; i <= 2; i = i + 1) {
        let offset = params.step * f32(i);
        let w = WEIGHTS[u32(i + 2)];
        color = color + textureSample(input_texture, input_sampler, in.uv + offset) * w;
    }
    return color;
}
