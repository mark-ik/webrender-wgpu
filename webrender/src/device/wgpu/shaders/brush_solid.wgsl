// brush_solid.wgsl — solid-coloured quad. Pipeline-first migration plan
// §6 P1.1, P1.2, P1.3, P1.4, P1.5.
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

// Clip-mask texture. Mirrors GL `sampler2D sClipMask` in
// `prim_shared.glsl`; sampled by `textureLoad` (== `texelFetch`),
// no sampler needed. R-channel only (R8Unorm) holds the alpha mask.
// Always bound (the layout demands it); only read in the
// `ALPHA_PASS` fragment branch.
@group(0) @binding(5)
var clip_mask: texture_2d<f32>;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    // Clip-mask varyings, mirroring GL `vClipMaskUv` /
    // `vClipMaskUvBounds` in `prim_shared.glsl::write_clip`. Always
    // written (cheap) so the shader signature stays constant across
    // override variants; only the alpha-pass fragment reads them.
    @location(1) @interpolate(linear) clip_uv: vec2<f32>,
    @location(2) @interpolate(flat) clip_bounds: vec4<f32>,
}

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) a_data: vec4<i32>,
) -> VsOut {
    // Decode the GL-shaped instance attributes from `a_data` per
    // `prim_shared.glsl::decode_instance_attributes`. Field consumers
    // landing later: `segment_index|flags`, `resource_address|brush_kind`.
    let prim_header_address = a_data.x;
    let clip_address = a_data.y;

    let header = prim_headers[prim_header_address];
    let transform = transforms[header.transform_id & TRANSFORM_INDEX_MASK];
    let task = render_tasks[header.picture_task_address];
    let clip_area = render_tasks[clip_address];
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

    // ClipArea uses `user_data.yz` as `screen_origin` (vs. PictureTask's
    // `content_origin`). Per `prim_shared.glsl::write_clip`:
    //   uv = world_pos.xy * area.device_pixel_scale +
    //        world_pos.w * (area.task_rect.p0 - area.screen_origin)
    let area_dps = clip_area.user_data.x;
    let area_screen_origin = clip_area.user_data.yz;
    let clip_uv = world_pos.xy * area_dps
        + world_pos.w * (clip_area.task_rect.xy - area_screen_origin);
    let clip_bounds = clip_area.task_rect;

    var out: VsOut;
    out.position = clip_pos;
    out.color = color;
    out.clip_uv = clip_uv;
    out.clip_bounds = clip_bounds;
    return out;
}

// `do_clip()` mirrors GL `prim_shared.glsl::do_clip` for the
// non-SWGL_CLIP_MASK path. Returns the [0..1] alpha multiplier from
// the clip-mask texture, or 0.0 if outside the clip bounds, or 1.0 if
// the bounds are the dummy zero-rect (opaque-pass marker GL uses to
// signal "no clip needed").
//
// `position.w` is the per-fragment 1/clip_pos.w (matches GL
// `gl_FragCoord.w`); the multiply undoes the perspective divide on
// the linearly-interpolated `clip_uv`.
fn do_clip(in: VsOut) -> f32 {
    if (all(in.clip_bounds.xy == in.clip_bounds.zw)) {
        return 1.0;
    }
    let mask_uv = in.clip_uv * in.position.w;
    let left = all(in.clip_bounds.xy <= mask_uv);
    let right = all(in.clip_bounds.zw > mask_uv);
    if (!left || !right) {
        return 0.0;
    }
    return textureLoad(clip_mask, vec2<i32>(mask_uv), 0).r;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var color = in.color;
    if (ALPHA_PASS) {
        // Production also multiplies by `antialias_brush()` for edge
        // AA (driven by `WR_FEATURE_ANTIALIASING`); that's a separate
        // override we'll add when AA-aware rect computation lands.
        let clip_alpha = do_clip(in);
        color = color * clip_alpha;
    }
    return color;
}
