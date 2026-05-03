// ps_text_run_dual_source.wgsl — Phase 10a.4 subpixel-AA text shader.
//
// Requires the WGSL `dual_source_blending` extension. The pipeline
// factory (`build_brush_text_dual_source`) only attempts to build
// this module when `device.features()` contains
// `Features::DUAL_SOURCE_BLENDING`, so the enable directive below
// matches the consumer's runtime check.
//
// Same instance + binding shape as `ps_text_run.wgsl` (the grayscale
// path); the difference lives in the fragment outputs and the
// pipeline blend state. Two `@location(0)` outputs feed the
// dual-source blend equation:
//
//   dst.rgb = 1 * src.rgb + (1 - src1.rgb) * dst.rgb
//   dst.a   = 1 * src.a   + (1 - src1.a)   * dst.a
//
// where `src` is `out.color` and `src1` is `out.alpha`. The two
// outputs differ only by what they multiply the tint by:
//
//   color = (tint.rgb * cov_rgb, tint.a * cov_avg)   -- premultiplied
//   alpha = (tint.a   * cov_rgb, tint.a * cov_avg)   -- per-channel "alpha"
//
// For a per-channel coverage triple (cR, cG, cB), the framebuffer
// blend produces a per-subpixel anti-aliased result — each LCD
// sub-pixel sees its own coverage value, tripling effective
// horizontal resolution.
//
// Phase 10a.4 atlas note: today the glyph atlas is single-channel
// R8 (10a.1). The shader reads `.r` and broadcasts it to the
// `cov_rgb` triple, so output is bit-equivalent to the grayscale
// `ps_text_run.wgsl` path for the same input — proving the wiring
// without requiring a subpixel-rasterized atlas. A 10b sub-task
// will introduce a parallel RGB(A) atlas for `Format::Subpixel`
// rasters; the shader is already prepared to consume per-channel
// coverage when the atlas binding's sample yields three different
// values.

enable dual_source_blending;

struct GlyphInstance {
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

struct PerFrame {
    u_transform: mat4x4<f32>,
}

@group(0) @binding(0) var<storage, read> instances: array<GlyphInstance>;
@group(0) @binding(1) var<storage, read> transforms: array<Transform>;
@group(0) @binding(2) var<uniform> per_frame: PerFrame;
@group(0) @binding(3) var atlas_texture: texture_2d<f32>;
@group(0) @binding(4) var atlas_sampler: sampler;

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
    out.position.z = inst.z_depth;
    out.uv = mix(inst.uv_rect.xy, inst.uv_rect.zw, corner);
    out.color = inst.color;
    out.clip = inst.clip;
    return out;
}

struct FsOut {
    @location(0) @blend_src(0) color: vec4<f32>,
    @location(0) @blend_src(1) alpha: vec4<f32>,
}

@fragment
fn fs_main(in: VsOut) -> FsOut {
    let sample = textureSample(atlas_texture, atlas_sampler, in.uv);
    let p = in.position.xy;
    if (p.x < in.clip.x || p.y < in.clip.y || p.x >= in.clip.z || p.y >= in.clip.w) {
        discard;
    }

    // R8Unorm sample returns (r, 0, 0, 1) — .g and .b are zero, not
    // a copy of .r. Broadcast .r explicitly across all three channels
    // so 10a.4's grayscale-broadcast contract is bit-equivalent to
    // the single-source `ps_text_run` path. A future RGB(A) atlas
    // (10b subpixel-rasterizer sub-task) would substitute a per-
    // channel sample — likely behind an override or a sibling
    // shader — and route its own averaging rule (e.g. GDI's
    // `(r+g+b)/3`) into `cov_avg`.
    let coverage = sample.r;
    let cov_rgb = vec3<f32>(coverage);
    let cov_avg = coverage;

    var out: FsOut;
    out.color = vec4<f32>(in.color.rgb * cov_rgb, in.color.a * cov_avg);
    out.alpha = vec4<f32>(in.color.a   * cov_rgb, in.color.a * cov_avg);
    return out;
}
