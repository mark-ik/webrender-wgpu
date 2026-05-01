// brush_image.wgsl — Phase 5 textured-rect batch shader.
//
// Bind group (group 0):
//   0 — instances:      array<ImageInstance>  (storage, read-only)
//   1 — transforms:     array<Transform>      (storage, read-only)
//   2 — per_frame:      PerFrame              (uniform)
//   3 — image_texture:  texture_2d<f32>
//   4 — image_sampler:  sampler (non-filtering / nearest)
//
// Instance struct (80-byte stride, std430):
//   rect          vec4<f32>  offset  0 — local-space [x0,y0,x1,y1]
//   uv_rect       vec4<f32>  offset 16 — UV corners [u0,v0,u1,v1]
//   color         vec4<f32>  offset 32 — premultiplied RGBA tint
//   clip          vec4<f32>  offset 48 — device-space clip rect
//   transform_id  u32        offset 64
//   z_depth       f32        offset 68 — NDC depth in [0,1]; 0=near/front
//                              8 bytes implicit padding → stride 80

struct ImageInstance {
    rect: vec4<f32>,
    uv_rect: vec4<f32>,
    color: vec4<f32>,
    clip: vec4<f32>,
    transform_id: u32,
    z_depth: f32,
}

struct Transform {
    m: mat4x4<f32>,
}

@group(0) @binding(0)
var<storage, read> instances: array<ImageInstance>;

@group(0) @binding(1)
var<storage, read> transforms: array<Transform>;

struct PerFrame {
    // Orthographic projection: device pixels → NDC.
    u_transform: mat4x4<f32>,
}

@group(0) @binding(2)
var<uniform> per_frame: PerFrame;

@group(0) @binding(3)
var image_texture: texture_2d<f32>;

@group(0) @binding(4)
var image_sampler: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) clip: vec4<f32>,
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
    // Override clip-space Z with per-instance depth (ortho: W=1, so NDC_Z = z_depth).
    out.position.z = inst.z_depth;
    out.uv = mix(inst.uv_rect.xy, inst.uv_rect.zw, corner);
    out.color = inst.color;
    out.clip = inst.clip;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Sample texture in uniform control flow (before discard) for portability.
    let tex_color = textureSample(image_texture, image_sampler, in.uv);
    let p = in.position.xy;
    if (p.x < in.clip.x || p.y < in.clip.y || p.x >= in.clip.z || p.y >= in.clip.w) {
        discard;
    }
    // Premultiplied tint: tex_color.rgb already premultiplied; multiply
    // element-wise. White tint [1,1,1,1] is a no-op.
    return tex_color * in.color;
}
