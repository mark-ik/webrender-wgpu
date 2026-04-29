// brush_solid.wgsl — solid-coloured quad. Pipeline-first migration plan
// §6 P1.1, P1.2, P1.3, P1.4.
//
// Mirrors the GL `brush_solid.glsl` data contract via storage buffers
// (parent plan §4.6): a PrimitiveHeader table indexed by the instance's
// `a_data.x` attribute, a Transform table indexed by
// `header.transform_id`, plus a GpuBuffer table whose `vec4` slot at
// `specific_prim_address` holds the brush colour. Production has more
// inputs (PictureTask, ClipArea) — those land in subsequent P1
// sub-slices. The shape here exercises:
//
//   - Per-instance `a_data: vec4<i32>` vertex attribute (matches GL
//     `PER_INSTANCE in ivec4 aData` from `prim_shared.glsl`); decoded
//     fields: `prim_header_address` (`a_data.x`),
//     `clip_address` (`a_data.y`), `segment_index | flags` (`a_data.z`),
//     `resource_address | brush_kind` (`a_data.w`).
//     Only `prim_header_address` is consumed yet; the others land
//     with their consumers (P1.4 picture-task, P1.5 clip).
//   - PrimitiveHeader fetched by `a_data.x`.
//   - Transform fetched by the low 22 bits of `header.transform_id`
//     (the high bit encodes is_axis_aligned per GL `fetch_transform`;
//     not used here until P1.5's AA path).
//   - GpuBuffer is read at `header.specific_prim_address` to get the
//     `vec4` colour (matches GL `fetch_solid_primitive` →
//     `fetch_from_gpu_buffer_1f`).
//   - `local_rect` corner is multiplied by `transform.m` to reach
//     clip space. Production additionally applies `picture_task`
//     content-origin offset and `device_pixel_scale` (P1.4 wiring);
//     for the smoke, an identity transform plus a clip-space
//     `local_rect` keeps the rendered shape unchanged.
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

// Transform storage buffer. Mirrors `transform.glsl::Transform`:
// `m` (4×4 mat) and `inv_m` (4×4 mat) per entry, 128 bytes std430.
// `inv_m` isn't used by brush_solid's vertex path but is part of the
// production table shape — fragment paths (and other families) read
// it for untransform / fragment-side AA.
struct Transform {
    m: mat4x4<f32>,
    inv_m: mat4x4<f32>,
}

@group(0) @binding(1)
var<storage, read> transforms: array<Transform>;

// Mask matching GL `TRANSFORM_INDEX_MASK` in `transform.glsl`. The high
// bit of `header.transform_id` encodes is_axis_aligned (bit 23 set →
// non-axis-aligned). Production decodes this for AA path selection;
// brush_solid will use it in P1.5.
const TRANSFORM_INDEX_MASK: i32 = 0x003fffff;

// GpuBuffer storage buffer. Mirrors GL `fetch_from_gpu_buffer_1f` /
// `_2f` / `_4f`: a flat `vec4<f32>` array indexed by an integer
// "address." Brush-specific data lives at `header.specific_prim_address`
// for one to several `vec4` slots per primitive type. brush_solid reads
// one `vec4` (the colour) — `VECS_PER_SPECIFIC_BRUSH = 1` in
// `brush_solid.glsl`.
@group(0) @binding(2)
var<storage, read> gpu_buffer_f: array<vec4<f32>>;

// RenderTaskData storage buffer. Mirrors GL
// `render_task.glsl::RenderTaskData`: { task_rect: vec4, user_data: vec4 }.
// Both PictureTask and ClipArea are interpretations of this 32-byte
// struct — `user_data.x` = device_pixel_scale; `user_data.yz` =
// content_origin (PictureTask) or screen_origin (ClipArea). Indexed by
// `header.picture_task_address` for the picture task; later sub-slices
// will index by `instance.clip_address` for the clip-area read.
struct RenderTaskData {
    task_rect: vec4<f32>,
    user_data: vec4<f32>,
}

@group(0) @binding(3)
var<storage, read> render_tasks: array<RenderTaskData>;

// Per-frame uniform. `u_transform` is the orthographic projection
// matching GL `uTransform` in `shared.glsl` — converts device pixels
// to clip space. Per parent §4.7 tier 4 (static uniform buffer
// per-pass / per-frame).
struct PerFrame {
    u_transform: mat4x4<f32>,
}

@group(0) @binding(4)
var<uniform> per_frame: PerFrame;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) a_data: vec4<i32>,
) -> VsOut {
    // Decode the GL-shaped instance attributes from `a_data`. Only
    // `prim_header_address` is consumed yet; clip_address /
    // segment_index / flags / resource_address / brush_kind land with
    // their consumers in P1.5+.
    let prim_header_address = a_data.x;

    let header = prim_headers[prim_header_address];
    let transform = transforms[header.transform_id & TRANSFORM_INDEX_MASK];
    let task = render_tasks[header.picture_task_address];
    let color = gpu_buffer_f[header.specific_prim_address];

    // Decode PictureTask fields from `task.user_data` per GL
    // `fetch_picture_task` in `render_task.glsl`.
    let device_pixel_scale = task.user_data.x;
    let content_origin = task.user_data.yz;

    // Triangle strip: vertex_index 0..3 sweeps the four corners of
    // `local_rect`. Full GL vertex pipeline (see
    // `prim_shared.glsl::write_vertex`):
    //   world_pos    = transform.m * vec4(local_pos, 0, 1)
    //   device_pos   = world_pos.xy * task.device_pixel_scale
    //   final_offset = -task.content_origin + task.task_rect.p0
    //   final_pos    = device_pos + final_offset * world_pos.w
    //   clip_pos     = u_transform * vec4(final_pos, z * world_pos.w, world_pos.w)
    // Smoke feeds identity transform + identity u_transform + zero
    // content_origin/task_rect.p0 + device_pixel_scale=1 +
    // local_rect already in clip space, so the math collapses to
    // `clip_pos = vec4(local_pos, 0, 1)` — preserves the P1.3 receipt.
    let corner = vec2<f32>(
        f32(vertex_index & 1u),
        f32((vertex_index >> 1u) & 1u),
    );
    let p0 = header.local_rect.xy;
    let p1 = header.local_rect.zw;
    let local_pos = mix(p0, p1, corner);

    let world_pos = transform.m * vec4<f32>(local_pos, 0.0, 1.0);
    let device_pos = world_pos.xy * device_pixel_scale;
    let final_offset = -content_origin + task.task_rect.xy;
    let z = f32(header.z);
    let clip_pos = per_frame.u_transform * vec4<f32>(
        device_pos + final_offset * world_pos.w,
        z * world_pos.w,
        world_pos.w,
    );

    var out: VsOut;
    out.position = clip_pos;
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
