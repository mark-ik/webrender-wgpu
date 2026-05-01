/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 8C receipt — `brush_conic_gradient` (2-stop, analytic).
//!
//! `t = fract((atan2(dy, dx) - start_angle) / 2π)`. Computed
//! per-fragment because angle is non-linear in pixel position.
//! With y+ down (screen convention), atan2 increases clockwise:
//! 0=east, π/2=south, π=west, -π/2=north.
//!
//! Tests:
//!   p8c_01_quarter_turns                   — pixels at the four cardinal
//!                                             directions hit t = 0, 0.25,
//!                                             0.5, 0.75
//!   p8c_02_seam_at_start_angle             — pixels just before/after
//!                                             the seam jump from color1
//!                                             to color0
//!   p8c_03_uniform_when_color0_eq_color1   — degenerate 2-stop with
//!                                             matching colors fills
//!                                             solid (no visible seam)
//!   p8c_04_paints_in_front_of_radial       — conic on top of radial in
//!                                             Phase 8C ordering

use std::f32::consts::PI;

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
        label: Some("p8c target"),
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

/// Compute the expected conic gradient color at pixel `(x, y)` for a
/// gradient centered at `c` with seam at `start_angle`, sweeping from
/// `color0` (linear premultiplied RGBA) to `color1`. Returns the
/// post-blend, post-sRGB-encode pixel value over an opaque-black
/// backdrop.
fn expected_conic(
    x: u32,
    y: u32,
    c: [f32; 2],
    start_angle: f32,
    color0: [f32; 4],
    color1: [f32; 4],
) -> [u8; 4] {
    let dx = (x as f32 + 0.5) - c[0];
    let dy = (y as f32 + 0.5) - c[1];
    let raw = dy.atan2(dx);
    let mut t = (raw - start_angle) / (2.0 * PI);
    t = t - t.floor(); // fract
    // Linear interpolation between premultiplied colors.
    let mix = [
        color0[0] * (1.0 - t) + color1[0] * t,
        color0[1] * (1.0 - t) + color1[1] * t,
        color0[2] * (1.0 - t) + color1[2] * t,
        color0[3] * (1.0 - t) + color1[3] * t,
    ];
    // Premultiplied-over-opaque-black: rgb passes through, alpha → 1.
    [
        srgb_encode(mix[0]),
        srgb_encode(mix[1]),
        srgb_encode(mix[2]),
        255,
    ]
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// Cardinal-direction sample. `start_angle = 0` puts the seam due
/// east. With y+ down, t increases clockwise: south = 0.25, west = 0.5,
/// north = 0.75.
#[test]
fn p8c_01_quarter_turns() {
    let renderer = make_renderer();
    let mut scene = Scene::new(W, H);
    let cx = W as f32 / 2.0;
    let cy = H as f32 / 2.0;
    let color0 = [1.0, 0.0, 0.0, 1.0]; // red at t=0
    let color1 = [0.0, 0.0, 1.0, 1.0]; // blue at t=1
    scene.push_conic_gradient(
        0.0, 0.0, W as f32, H as f32,
        [cx, cy],
        0.0,
        color0,
        color1,
    );

    let bytes = render_to_bytes(&renderer, &scene);

    // Sample on each cardinal axis well away from the center to avoid
    // the radius-zero atan2(0,0) edge case. Tolerance ±2/255 because
    // the actual t is determined by pixel-center geometry, which we
    // recompute precisely in `expected_conic`.
    for &(x, y) in &[
        (W - 1, 32), // far east — small dy/dx, t ≈ 0 (or wraps very near 1)
        (32, H - 1), // far south — t ≈ 0.25
        (0, 32),     // far west — t ≈ 0.5
        (32, 0),     // far north — t ≈ 0.75
    ] {
        let expected = expected_conic(x, y, [cx, cy], 0.0, color0, color1);
        assert_within_tol(
            pixel(&bytes, W, x, y),
            expected,
            2,
            &format!("({}, {})", x, y),
        );
    }
}

/// Two pixels sitting just on either side of the seam should differ
/// drastically: one is near `color1` (t ≈ 1), the other near `color0`
/// (t ≈ 0). Place the seam at `start_angle = 0` (due east) and sample
/// pixels at the eastern edge, just above and just below the y=cy line.
#[test]
fn p8c_02_seam_at_start_angle() {
    let renderer = make_renderer();
    let mut scene = Scene::new(W, H);
    let cx = W as f32 / 2.0;
    let cy = H as f32 / 2.0;
    let color0 = [1.0, 0.0, 0.0, 1.0]; // red
    let color1 = [0.0, 1.0, 0.0, 1.0]; // green
    scene.push_conic_gradient(
        0.0, 0.0, W as f32, H as f32,
        [cx, cy],
        0.0,
        color0,
        color1,
    );

    let bytes = render_to_bytes(&renderer, &scene);

    // At (W-1, 32): pixel-center (63.5, 32.5). dy = 0.5, dx = 31.5.
    // raw_angle = atan2(0.5, 31.5) ≈ +0.0159 rad. t = 0.0159 / (2π)
    //           ≈ 0.00253 → near color0 (red).
    let just_after = pixel(&bytes, W, W - 1, 32);
    // At (W-1, 31): pixel-center (63.5, 31.5). dy = -0.5, dx = 31.5.
    // raw_angle = atan2(-0.5, 31.5) ≈ -0.0159 rad. t = fract(-0.00253)
    //           = 0.997 → near color1 (green).
    let just_before = pixel(&bytes, W, W - 1, 31);

    // After the seam: red-dominant.
    assert!(
        just_after[0] > 240 && just_after[1] < 15,
        "just-after-seam pixel {:?} should be near-red",
        just_after
    );
    // Before the seam: green-dominant.
    assert!(
        just_before[1] > 240 && just_before[0] < 15,
        "just-before-seam pixel {:?} should be near-green",
        just_before
    );
}

/// `color0 == color1` collapses the gradient to a uniform fill — every
/// pixel is the same color, no visible seam.
#[test]
fn p8c_03_uniform_when_color0_eq_color1() {
    let renderer = make_renderer();
    let mut scene = Scene::new(W, H);
    let yellow = [1.0, 1.0, 0.0, 1.0];
    scene.push_conic_gradient(
        0.0, 0.0, W as f32, H as f32,
        [W as f32 / 2.0, H as f32 / 2.0],
        0.0,
        yellow,
        yellow,
    );

    let bytes = render_to_bytes(&renderer, &scene);

    let expected = [255, 255, 0, 255];
    // Sample 9 pixels, including the center (where atan2(0,0) is
    // implementation-defined but mix is identity since both colors match).
    for &(x, y) in &[(0_u32, 0_u32), (32, 32), (W - 1, H - 1), (10, 50), (50, 10)] {
        assert_within_tol(
            pixel(&bytes, W, x, y),
            expected,
            2,
            &format!("uniform pixel ({}, {})", x, y),
        );
    }
}

/// Conic over a radial in the same scene. Per the Phase 8C family
/// painter ordering (radial < conic), the conic must overwrite the
/// radial at every pixel of overlap.
#[test]
fn p8c_04_paints_in_front_of_radial() {
    let renderer = make_renderer();
    let mut scene = Scene::new(W, H);
    let cx = W as f32 / 2.0;
    let cy = H as f32 / 2.0;
    // Radial backdrop: solid green-to-green (uniform green).
    scene.push_radial_gradient(
        0.0, 0.0, W as f32, H as f32,
        [cx, cy],
        [32.0, 32.0],
        [0.0, 1.0, 0.0, 1.0],
        [0.0, 1.0, 0.0, 1.0],
    );
    let color0 = [1.0, 0.0, 0.0, 1.0];
    let color1 = [0.0, 0.0, 1.0, 1.0];
    scene.push_conic_gradient(
        0.0, 0.0, W as f32, H as f32,
        [cx, cy],
        0.0,
        color0,
        color1,
    );

    let bytes = render_to_bytes(&renderer, &scene);

    // Every visible pixel must be the conic value (red↔blue), not the
    // radial green.
    for &(x, y) in &[(W - 1, 32), (32, H - 1), (0, 32), (32, 0), (10, 10)] {
        let expected = expected_conic(x, y, [cx, cy], 0.0, color0, color1);
        let actual = pixel(&bytes, W, x, y);
        assert_within_tol(actual, expected, 2, &format!("({}, {}) is conic, not green", x, y));
        assert!(
            actual[1] < 5,
            "({}, {}) green channel {} suggests radial bleeding through",
            x, y, actual[1]
        );
    }
}
