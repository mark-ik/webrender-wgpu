// brush_radial_gradient.wgsl — Phase 8B 2-stop analytic radial gradient.
//
// Same bind-group shape and 96-byte instance stride as
// brush_linear_gradient; the fragment shader's `t` formula is the only
// real difference. Linear computes `t` per vertex (via dot product
// onto the gradient line) and lets the rasterizer interpolate; radial
// can't do that — `t = length((pixel - center) / radii)` is non-linear
// in pixel position, so the local position is interpolated and `t` is
// computed per-fragment.
//
// Bind group (shared `brush_gradient_layout`):
//   0 — instances:  array<RadialGradientInstance> (storage, read-only)
//   1 — transforms: array<Transform>              (storage, read-only)
//   2 — per_frame:  PerFrame                      (uniform)
//
// Instance struct (96-byte stride, WGSL std430). The 16-byte
// `params` slot replaces brush_linear_gradient's `line` slot:
//   rect          vec4<f32>  offset  0 — local-space corners [x0,y0,x1,y1]
//   params        vec4<f32>  offset 16 — center.xy, radii.xy
//   color0        vec4<f32>  offset 32 — premultiplied color at center
//   color1        vec4<f32>  offset 48 — premultiplied color at boundary
//   clip          vec4<f32>  offset 64 — device-space clip [x0,y0,x1,y1]
//   transform_id  u32        offset 80 — index into transforms[]
//   z_depth       f32        offset 84 — NDC depth in [0,1]; 0=near/front
//                            8 bytes implicit padding → stride 96

struct RadialGradientInstance {
    rect: vec4<f32>,
    params: vec4<f32>,
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
var<storage, read> instances: array<RadialGradientInstance>;

@group(0) @binding(1)
var<storage, read> transforms: array<Transform>;

struct PerFrame {
    u_transform: mat4x4<f32>,
}

@group(0) @binding(2)
var<uniform> per_frame: PerFrame;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) local_pos: vec2<f32>,
    @location(1) @interpolate(flat) center: vec2<f32>,
    @location(2) @interpolate(flat) radii: vec2<f32>,
    @location(3) @interpolate(flat) color0: vec4<f32>,
    @location(4) @interpolate(flat) color1: vec4<f32>,
    @location(5) @interpolate(flat) clip: vec4<f32>,
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
    out.position.z = inst.z_depth;
    out.local_pos = local_pos;
    out.center = inst.params.xy;
    out.radii = inst.params.zw;
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
    let safe_radii = max(in.radii, vec2<f32>(1e-9, 1e-9));
    let d = (in.local_pos - in.center) / safe_radii;
    let t = clamp(length(d), 0.0, 1.0);
    return mix(in.color0, in.color1, t);
}
