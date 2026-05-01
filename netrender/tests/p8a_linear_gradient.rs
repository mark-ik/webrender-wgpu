/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 8A receipt — `brush_linear_gradient` (2-stop, analytic).
//!
//! `t` is computed at each rect corner in the vertex shader and linearly
//! interpolated across the primitive by the rasterizer; the fragment
//! shader does `mix(color0, color1, clamp(t, 0, 1))`. For axis-aligned
//! rects this is bit-exact equivalent to per-pixel computation, so a few
//! programmatic pixel checks (no golden file) suffice.
//!
//! Tests:
//!   p8a_01_horizontal_red_to_blue       — gradient along x; pixel at x=N
//!                                          matches `mix(red, blue, N/W)`
//!   p8a_02_vertical_alpha_fade          — top opaque white, bottom 0-alpha;
//!                                          alpha decays linearly down
//!   p8a_03_clamp_outside_gradient_line  — `t` is clamped to [0,1] when
//!                                          the rect extends past
//!                                          start_point / end_point
//!   p8a_04_with_phase4_rect_underneath  — gradient draws on top of an
//!                                          opaque rect via the unified
//!                                          z assignment

use netrender::{
    ColorLoad, FrameTarget, NetrenderOptions, Renderer, Scene, boot, create_netrender_instance,
};

const W: u32 = 64;
const H: u32 = 64;
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn make_renderer() -> Renderer {
    let handles = boot().expect("wgpu boot");
    create_netrender_instance(handles, NetrenderOptions::default())
        .expect("create_netrender_instance")
}

fn render_to_bytes(renderer: &Renderer, scene: &Scene) -> Vec<u8> {
    let device = renderer.wgpu_device.core.device.clone();
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("p8a target"),
        size: wgpu::Extent3d {
            width: scene.viewport_width,
            height: scene.viewport_height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TARGET_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let prepared = renderer.prepare(scene);
    renderer.render(
        &prepared,
        FrameTarget {
            view: &view,
            format: TARGET_FORMAT,
            width: scene.viewport_width,
            height: scene.viewport_height,
        },
        ColorLoad::Clear(wgpu::Color::BLACK),
    );
    renderer
        .wgpu_device
        .read_rgba8_texture(&target, scene.viewport_width, scene.viewport_height)
}

fn pixel(bytes: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]
}

/// sRGB encode a linear value in [0, 1] to [0, 255]. The framebuffer
/// applies this when an `Rgba8UnormSrgb` color is written.
fn srgb_encode(linear: f32) -> u8 {
    let l = linear.clamp(0.0, 1.0);
    let v = if l <= 0.0031308 {
        12.92 * l
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

fn channel_diff(a: u8, b: u8) -> u8 {
    (a as i16 - b as i16).unsigned_abs() as u8
}

fn assert_within_tol(actual: [u8; 4], expected: [u8; 4], tol: u8, where_: &str) {
    let diff = [
        channel_diff(actual[0], expected[0]),
        channel_diff(actual[1], expected[1]),
        channel_diff(actual[2], expected[2]),
        channel_diff(actual[3], expected[3]),
    ];
    let max = *diff.iter().max().unwrap();
    assert!(
        max <= tol,
        "{}: actual {:?}, expected {:?} (max channel diff = {}, tol = {})",
        where_, actual, expected, max, tol
    );
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// Full-canvas horizontal gradient: red on the left, blue on the right.
/// Sampled pixel `(x, _)` should match `mix(red, blue, t)` with
/// `t = (x + 0.5 - start_x) / (end_x - start_x)`. The gradient line spans
/// the full canvas width, so endpoints map to corners of the rect.
#[test]
fn p8a_01_horizontal_red_to_blue() {
    let renderer = make_renderer();
    let mut scene = Scene::new(W, H);
    scene.push_linear_gradient(
        0.0, 0.0, W as f32, H as f32,
        [0.0, 0.0],
        [W as f32, 0.0],
        [1.0, 0.0, 0.0, 1.0], // red
        [0.0, 0.0, 1.0, 1.0], // blue
    );

    let bytes = render_to_bytes(&renderer, &scene);

    // Sample several columns across the gradient. Row is irrelevant
    // because the gradient is purely horizontal.
    for &col in &[0_u32, 16, 32, 48, 63] {
        let t = (col as f32 + 0.5) / W as f32;
        let r = srgb_encode(1.0 - t);
        let b = srgb_encode(t);
        let actual = pixel(&bytes, W, col, 32);
        // Per-channel tolerance ±2/255: covers sRGB rounding through
        // the encode + interpolation path on different GPU adapters.
        assert_within_tol(actual, [r, 0, b, 255], 2, &format!("column {}", col));
    }
}

/// Vertical alpha fade: top opaque white, bottom fully transparent.
/// Premultiplied source `[a, a, a, a]` over opaque black backdrop →
/// framebuffer pixel `[a, a, a, 1]` (in linear), then sRGB-encoded on
/// write. Framebuffer alpha is always 255 because the blend's alpha
/// equation is `src.a + dst.a*(1 - src.a)` and `dst.a == 1`. The
/// alpha-fade signal lives in the RGB channels.
#[test]
fn p8a_02_vertical_alpha_fade() {
    let renderer = make_renderer();
    let mut scene = Scene::new(W, H);
    scene.push_linear_gradient(
        0.0, 0.0, W as f32, H as f32,
        [0.0, 0.0],
        [0.0, H as f32],
        [1.0, 1.0, 1.0, 1.0], // opaque white at top
        [0.0, 0.0, 0.0, 0.0], // fully transparent at bottom (premultiplied)
    );

    let bytes = render_to_bytes(&renderer, &scene);

    for &row in &[0_u32, 16, 32, 48, 63] {
        let t = (row as f32 + 0.5) / H as f32;
        let alpha_linear = 1.0 - t;
        let rgb = srgb_encode(alpha_linear);
        let actual = pixel(&bytes, W, 32, row);
        assert_within_tol(actual, [rgb, rgb, rgb, 255], 2, &format!("row {}", row));
    }
}

/// `t` is clamped to [0, 1] when the rect extends beyond the gradient
/// line. A 64×64 rect with the gradient line spanning only x ∈ [16, 48]
/// must show solid color0 in x ∈ [0, 16] and solid color1 in x ∈ [48, 64].
#[test]
fn p8a_03_clamp_outside_gradient_line() {
    let renderer = make_renderer();
    let mut scene = Scene::new(W, H);
    // Green-to-yellow gradient over the middle 32 columns; rect is full.
    scene.push_linear_gradient(
        0.0, 0.0, W as f32, H as f32,
        [16.0, 0.0],
        [48.0, 0.0],
        [0.0, 1.0, 0.0, 1.0], // green
        [1.0, 1.0, 0.0, 1.0], // yellow
    );

    let bytes = render_to_bytes(&renderer, &scene);

    // Left edge: solid green (clamped t = 0).
    assert_within_tol(pixel(&bytes, W, 5, 32), [0, 255, 0, 255], 2, "left clamp");
    assert_within_tol(pixel(&bytes, W, 0, 0), [0, 255, 0, 255], 2, "top-left clamp");
    // Right edge: solid yellow (clamped t = 1).
    assert_within_tol(pixel(&bytes, W, 60, 32), [255, 255, 0, 255], 2, "right clamp");
    assert_within_tol(pixel(&bytes, W, 63, 63), [255, 255, 0, 255], 2, "bottom-right clamp");
}

/// Gradient draws on top of a previously-pushed opaque rect (later in
/// painter order = nearer in z). Receipt: pixel under the gradient
/// shows the gradient color, not the underlying rect.
#[test]
fn p8a_04_with_phase4_rect_underneath() {
    let renderer = make_renderer();
    let mut scene = Scene::new(W, H);
    // Painter order: rect first (farthest z), gradient second (nearer z).
    scene.push_rect(0.0, 0.0, W as f32, H as f32, [0.0, 1.0, 0.0, 1.0]); // green base
    scene.push_linear_gradient(
        0.0, 0.0, W as f32, H as f32,
        [0.0, 0.0],
        [W as f32, 0.0],
        [1.0, 0.0, 0.0, 1.0], // red
        [0.0, 0.0, 1.0, 1.0], // blue
    );

    let bytes = render_to_bytes(&renderer, &scene);

    // Center pixel: should be the gradient mid-point (≈ purple), not green.
    let mid = pixel(&bytes, W, 32, 32);
    let t = 32.5 / W as f32;
    let expected = [srgb_encode(1.0 - t), 0, srgb_encode(t), 255];
    assert_within_tol(mid, expected, 2, "gradient overrides underlying rect");
}
