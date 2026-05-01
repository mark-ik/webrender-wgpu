// brush_conic_gradient.wgsl — Phase 8C 2-stop analytic conic gradient.
//
// A conic gradient sweeps `t` around a center point. With y+ pointing
// downward (screen convention), `atan2(dy, dx)` increases clockwise:
// 0 = east, pi/2 = south, pi = west, -pi/2 = north. The gradient seam
// (where t wraps from 1 back to 0) sits at `start_angle` radians.
// Setting `start_angle = -pi/2` matches CSS `conic-gradient(from 0deg)`,
// which starts at the top (12 o'clock).
//
// Same bind-group shape and 96-byte instance stride as
// brush_linear_gradient and brush_radial_gradient. Only the
// fragment-shader math + the interpretation of `params` changes.
//
// Instance struct (96-byte stride, WGSL std430):
//   rect          vec4<f32>  offset  0 — local-space corners [x0,y0,x1,y1]
//   params        vec4<f32>  offset 16 — center.xy, start_angle, _pad
//   color0        vec4<f32>  offset 32 — premultiplied color at the seam
//                                         (t=0 just after start_angle)
//   color1        vec4<f32>  offset 48 — premultiplied color at the
//                                         seam (t=1 just before start_angle)
//   clip          vec4<f32>  offset 64
//   transform_id  u32        offset 80
//   z_depth       f32        offset 84
//                            8 bytes padding → stride 96

struct ConicGradientInstance {
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
var<storage, read> instances: array<ConicGradientInstance>;

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
    @location(2) @interpolate(flat) start_angle: f32,
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
    out.start_angle = inst.params.z;
    out.color0 = inst.color0;
    out.color1 = inst.color1;
    out.clip = inst.clip;
    return out;
}

const TWO_PI: f32 = 6.283185307179586;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let p = in.position.xy;
    if (p.x < in.clip.x || p.y < in.clip.y || p.x >= in.clip.z || p.y >= in.clip.w) {
        discard;
    }
    let d = in.local_pos - in.center;
    let raw_angle = atan2(d.y, d.x);
    // fract handles negative inputs by returning the positive fractional part,
    // collapsing wraparound at the seam without a branch.
    let t = fract((raw_angle - in.start_angle) / TWO_PI);
    return mix(in.color0, in.color1, t);
}
