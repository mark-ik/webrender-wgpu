// brush_solid.wgsl — solid-coloured quad. Pipeline-first migration plan
// §6 P1.1.
//
// Mirrors the GL `brush_solid.glsl` data contract via storage buffers
// (parent plan §4.6): a PrimitiveHeader table indexed by instance index,
// plus a GpuBuffer table whose `vec4` slot at `specific_prim_address`
// holds the brush colour. Production has many more inputs (Transform,
// PictureTask, ClipArea, per-instance `ivec4 aData`) — those land in
// subsequent P1 sub-slices. This shader handles the smallest end-to-end
// shape that exercises the production storage-buffer pattern:
//
//   - PrimitiveHeader is fetched by instance_index. Real production
//     fetches it by `aData.x = prim_header_address` from a vertex
//     attribute; we collapse to instance_index here so the smoke can
//     run without instance vertex buffers (P1.3 lands those).
//   - GpuBuffer is read at `header.specific_prim_address` to get the
//     `vec4` color (matches GL `fetch_solid_primitive` →
//     `fetch_from_gpu_buffer_1f`).
//   - `local_rect` is interpreted directly in clip space (-1..1).
//     Production transforms `local_rect` through Transform +
//     PictureTask to clip space; that wiring is P1.2 / P1.4.
//
// Override (§4.9): `ALPHA_PASS` selects between opaque-write and
// alpha-multiply specialisations of the same shader source, mirroring
// `WR_FEATURE_ALPHA_PASS` in the GL version. Default `false` keeps the
// pipeline at the opaque shape; the alpha pipeline is a second
// specialisation in a later sub-slice.

override ALPHA_PASS: bool = false;

// PrimitiveHeader storage buffer. Mirrors `prim_shared.glsl::PrimitiveHeader`
// (collapsed from sPrimitiveHeadersF + sPrimitiveHeadersI 2D textures into
// one struct per parent §4.6). 64-byte std430 layout:
//   - local_rect:      vec4<f32>  bytes 0..16
//   - local_clip_rect: vec4<f32>  bytes 16..32
//   - z:               i32        bytes 32..36
//   - specific_prim_address: i32  bytes 36..40
//   - transform_id:    i32        bytes 40..44
//   - picture_task_address: i32   bytes 44..48
//   - user_data:       vec4<i32>  bytes 48..64
struct PrimitiveHeader {
    local_rect: vec4<f32>,
    local_clip_rect: vec4<f32>,
    z: i32,
    specific_prim_address: i32,
    transform_id: i32,
    picture_task_address: i32,
    user_data: vec4<i32>,
}

@group(0) @binding(0)
var<storage, read> prim_headers: array<PrimitiveHeader>;

// GpuBuffer storage buffer. Mirrors GL `fetch_from_gpu_buffer_1f` /
// `_2f` / `_4f`: a flat `vec4<f32>` array indexed by an integer
// "address." Brush-specific data lives at `header.specific_prim_address`
// for one to several `vec4` slots per primitive type. brush_solid reads
// one `vec4` (the colour) — `VECS_PER_SPECIFIC_BRUSH = 1` in
// `brush_solid.glsl`.
@group(0) @binding(1)
var<storage, read> gpu_buffer_f: array<vec4<f32>>;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VsOut {
    let header = prim_headers[instance_index];
    let color = gpu_buffer_f[header.specific_prim_address];

    // Triangle strip: vertex_index 0..3 sweeps the four corners of the
    // local_rect. Production transforms through the picture-task and
    // device-pixel scale (see GL `write_vertex` in `prim_shared.glsl`);
    // the smoke treats `local_rect` as already in clip space so the
    // storage-buffer fetch path is what's exercised, not the transform
    // pipeline (P1.2 / P1.4).
    let corner = vec2<f32>(
        f32(vertex_index & 1u),
        f32((vertex_index >> 1u) & 1u),
    );
    let p0 = header.local_rect.xy;
    let p1 = header.local_rect.zw;
    let xy = mix(p0, p1, corner);

    var out: VsOut;
    out.position = vec4<f32>(xy, 0.0, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var color = in.color;
    if (ALPHA_PASS) {
        // Production multiplies by `antialias_brush()` and the clip
        // mask sample (`do_clip()`); P1.5 wires those. For now the
        // alpha-pass override is just a marker that the specialisation
        // path works.
        color = vec4<f32>(color.rgb, color.a);
    }
    return color;
}
