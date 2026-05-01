// brush_linear_gradient.wgsl — Phase 8A 2-stop analytic linear gradient.
//
// Per-instance gradient defined by two endpoints (`start_point`,
// `end_point`) in local space and two premultiplied colors. The
// gradient parameter `t = dot(local_pos - start, dir) / |dir|^2`
// is computed at each corner; the rasterizer's linear interpolation
// across the rect produces per-pixel `t`, and the fragment shader
// mixes the two flat-interpolated colors. For axis-aligned rects
// this is bit-exact equivalent to per-pixel computation.
//
// Bind group (identical shape to brush_rect_solid):
//   0 — instances:  array<GradientInstance>  (storage, read-only)
//   1 — transforms: array<Transform>         (storage, read-only)
//   2 — per_frame:  PerFrame                 (uniform)
//
// Instance struct (96-byte stride, WGSL std430):
//   rect          vec4<f32>  offset  0 — local-space corners [x0,y0,x1,y1]
//   line          vec4<f32>  offset 16 — gradient endpoints [sx,sy,ex,ey]
//   color0        vec4<f32>  offset 32 — premultiplied start color
//   color1        vec4<f32>  offset 48 — premultiplied end color
//   clip          vec4<f32>  offset 64 — device-space clip [x0,y0,x1,y1]
//   transform_id  u32        offset 80 — index into transforms[]
//   z_depth       f32        offset 84 — NDC depth in [0,1]; 0=near/front
//                            8 bytes implicit padding → stride 96

struct GradientInstance {
    rect: vec4<f32>,
    line: vec4<f32>,
    color0: vec4<f32>,
    color1: vec4<f32>,
    clip: vec4<f32>,
    transform_id: u32,
    z_depth: f32,
}

struct Transform {
    m: mat4x4<f32>,
}

@group(0) @binding(0)
var<storage, read> instances: array<GradientInstance>;

@group(0) @binding(1)
var<storage, read> transforms: array<Transform>;

struct PerFrame {
    u_transform: mat4x4<f32>,
}

@group(0) @binding(2)
var<uniform> per_frame: PerFrame;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) gradient_t: f32,
    @location(1) @interpolate(flat) color0: vec4<f32>,
    @location(2) @interpolate(flat) color1: vec4<f32>,
    @location(3) @interpolate(flat) clip: vec4<f32>,
}

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VsOut {
    let inst = instances[instance_index];
    let corner = vec2<f32>(
        f32(vertex_index & 1u),
        f32((vertex_index >> 1u) & 1u),
    );
    let local_pos = mix(inst.rect.xy, inst.rect.zw, corner);
    let world_pos = transforms[inst.transform_id].m * vec4<f32>(local_pos, 0.0, 1.0);

    // Gradient parameter at this corner. Linearly interpolated across
    // the rect by the rasterizer.
    let p0 = inst.line.xy;
    let p1 = inst.line.zw;
    let dir = p1 - p0;
    let len_sq = max(dot(dir, dir), 1e-9);
    let t = dot(local_pos - p0, dir) / len_sq;

    var out: VsOut;
    out.position = per_frame.u_transform * world_pos;
    // Override clip-space Z with per-instance depth (matches brush_rect_solid).
    out.position.z = inst.z_depth;
    out.gradient_t = t;
    out.color0 = inst.color0;
    out.color1 = inst.color1;
    out.clip = inst.clip;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let p = in.position.xy;
    if (p.x < in.clip.x || p.y < in.clip.y || p.x >= in.clip.z || p.y >= in.clip.w) {
        discard;
    }
    let t = clamp(in.gradient_t, 0.0, 1.0);
    return mix(in.color0, in.color1, t);
}
