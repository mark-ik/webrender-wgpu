// brush_rect_solid.wgsl — Phase 4 solid-rect batch shader.
//
// Phase 2: per-instance storage buffer, inlined color, ortho projection.
// Phase 3: clip rect + transform palette.
// Phase 4: per-instance z_depth for front-to-back opaque sort + depth test.
//
// Bind group:
//   0 — instances:  array<RectInstance>  (storage, read-only)
//   1 — transforms: array<Transform>     (storage, read-only)
//   2 — per_frame:  PerFrame             (uniform)
//
// No vertex attributes. Corner coordinates are derived from
// @builtin(vertex_index) 0..3 (triangle-strip). Instance data is
// indexed by @builtin(instance_index). One draw call covers all
// rects in the batch (instance_range = 0..N).
//
// Instance struct (64-byte stride, WGSL std430):
//   rect          vec4<f32>   offset  0 — local-space corners [x0,y0,x1,y1]
//   color         vec4<f32>   offset 16 — premultiplied RGBA
//   clip          vec4<f32>   offset 32 — device-space clip rect [x0,y0,x1,y1]
//   transform_id  u32         offset 48 — index into transforms[]
//   z_depth       f32         offset 52 — NDC depth in [0,1]; 0=near/front
//                              8 bytes implicit padding → stride 64

struct RectInstance {
    rect: vec4<f32>,
    color: vec4<f32>,
    clip: vec4<f32>,
    transform_id: u32,
    z_depth: f32,
    // 8 bytes implicit WGSL padding to reach alignment 16 → stride 64
}

struct Transform {
    m: mat4x4<f32>,
}

@group(0) @binding(0)
var<storage, read> instances: array<RectInstance>;

@group(0) @binding(1)
var<storage, read> transforms: array<Transform>;

struct PerFrame {
    // Orthographic projection: device pixels → NDC.
    // NDC_x = 2*px/W - 1;  NDC_y = 1 - 2*py/H  (y+ down in device, y+ up in NDC)
    u_transform: mat4x4<f32>,
}

@group(0) @binding(2)
var<uniform> per_frame: PerFrame;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) clip: vec4<f32>,
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

    var out: VsOut;
    out.position = per_frame.u_transform * world_pos;
    // Override clip-space Z with per-instance depth.
    // With ortho projection W=1, NDC_Z = clip_Z / W = z_depth.
    // Depth buffer range [0,1]: 0=near (front), 1=far (back).
    out.position.z = inst.z_depth;
    out.color = inst.color;
    out.clip = inst.clip;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let p = in.position.xy;
    if (p.x < in.clip.x || p.y < in.clip.y || p.x >= in.clip.z || p.y >= in.clip.w) {
        discard;
    }
    return in.color;
}
