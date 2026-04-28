// brush_solid.wgsl — solid-coloured quad. Plan §6 S2.
//
// Exercises the §4.6–4.9 architectural patterns from line one:
//   - WGSL `override` (§4.9): MAX_PALETTE_ENTRIES is set at pipeline
//     compile time so future variant collapse uses one shader source.
//   - Storage buffer (§4.6): the colour palette is read from a
//     storage-buffer-bound table, not a data texture.
//   - Dynamic uniform (§4.7): per-draw rect bounds come from a
//     dynamic-offset uniform sub-allocated from a single arena.
//   - Push constant / immediate (§4.7): the palette index is a
//     push-constant per draw.

override MAX_PALETTE_ENTRIES: u32 = 16u;

struct DrawUniform {
    rect: vec4<f32>,  // x0, y0, w, h in clip space
}
@group(0) @binding(0) var<uniform> draw: DrawUniform;

// Runtime-sized storage array: WGSL requires storage-binding types to
// be CREATION_RESOLVED at module creation, so override-sized arrays
// cannot be the top-level binding type. The MAX_PALETTE_ENTRIES
// override is used below to clamp the index (still exercises §4.9).
@group(0) @binding(1)
var<storage, read> palette: array<vec4<f32>>;

struct PushConstants {
    palette_index: u32,
}
// `var<immediate>` is wgpu 29 / naga 29's rename of the push-constant
// address space — same GPU primitive (SPIR-V `PushConstant`), per the
// idiomatic-wgsl plan §4.7 and §4.10.
var<immediate> pc: PushConstants;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VsOut {
    // Triangle strip: vertex_index 0..3 sweeps the four corners of a
    // unit quad in (x, y) ∈ {0, 1}².
    let corner = vec2<f32>(
        f32(vertex_index & 1u),
        f32((vertex_index >> 1u) & 1u),
    );
    let xy = draw.rect.xy + draw.rect.zw * corner;

    // Clamp the index against the override-specialized maximum
    // (§4.9 — exercises pipeline-time constant specialization).
    let idx = min(pc.palette_index, MAX_PALETTE_ENTRIES - 1u);

    var out: VsOut;
    out.position = vec4<f32>(xy, 0.0, 1.0);
    out.color = palette[idx];
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
