/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 8B receipt — `brush_radial_gradient` (2-stop, analytic).
//!
//! `t = clamp(length((pixel - center) / radii), 0, 1)`. Computed
//! per-fragment because the formula is non-linear in pixel position
//! (no per-vertex shortcut like brush_linear_gradient gets).
//!
//! Tests:
//!   p8b_01_circular_center_to_boundary    — pixel at radius `r/2` from
//!                                            center matches mix(c0, c1, 0.5)
//!   p8b_02_outside_radius_clamps_to_color1 — corners of a rect that
//!                                            extend past the elliptical
//!                                            boundary are solid color1
//!   p8b_03_elliptical_radii                — distinct rx, ry: pixel
//!                                            (cx + rx/2, cy) and
//!                                            (cx, cy + ry/2) both at t=0.5
//!   p8b_04_paints_in_front_of_linear       — radial paints in front of
//!                                            linear (Phase 8B family
//!                                            ordering: linear < radial)

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
        label: Some("p8b target"),
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

// ── Tests ──────────────────────────────────────────────────────────────────

/// Circular gradient centered in a 64×64 canvas, radius = 32. A pixel
/// at distance r/2 from the center has t=0.5 and should match
/// `mix(red, blue, 0.5)`. Sample on the cardinal axes for clarity.
#[test]
fn p8b_01_circular_center_to_boundary() {
    let renderer = make_renderer();
    let mut scene = Scene::new(W, H);
    let cx = W as f32 / 2.0;
    let cy = H as f32 / 2.0;
    let r = 32.0_f32;
    scene.push_radial_gradient(
        0.0, 0.0, W as f32, H as f32,
        [cx, cy],
        [r, r],
        [1.0, 0.0, 0.0, 1.0], // red at center
        [0.0, 0.0, 1.0, 1.0], // blue at boundary
    );

    let bytes = render_to_bytes(&renderer, &scene);

    // Center pixel (32, 32) — pixel-center is at (32.5, 32.5), so the
    // sampled distance from center is sqrt(0.5) ≈ 0.707, t ≈ 0.022.
    // For the dead-center cardinal-axis tests we use offsets where the
    // distance is exactly the desired multiple of r within sub-pixel
    // tolerance.
    //
    // Pixel (16, 32): distance = (32.5 - 16.5, 32.5 - 32.5) = (16, 0) → r/2
    // Pixel (48, 32): distance = (15, 0) → very close to r/2 (within ½ px)
    // Pixel (32, 16): same as (16, 32) but vertical.
    for &(x, y) in &[(16_u32, 32_u32), (32, 16)] {
        let dx = (x as f32 + 0.5) - (cx);
        let dy = (y as f32 + 0.5) - (cy);
        let t = ((dx * dx + dy * dy).sqrt() / r).clamp(0.0, 1.0);
        let r_lin = (1.0 - t) * 1.0;
        let b_lin = t * 1.0;
        let expected = [srgb_encode(r_lin), 0, srgb_encode(b_lin), 255];
        assert_within_tol(pixel(&bytes, W, x, y), expected, 2, &format!("({}, {})", x, y));
    }
}

/// Pixels outside the elliptical boundary clamp to color1. With
/// `radii = [16, 16]` and center at (32, 32), the corners of a 64×64
/// rect lie at distance ≈ 32 — well outside r=16. They must be solid
/// color1, not extrapolated.
#[test]
fn p8b_02_outside_radius_clamps_to_color1() {
    let renderer = make_renderer();
    let mut scene = Scene::new(W, H);
    scene.push_radial_gradient(
        0.0, 0.0, W as f32, H as f32,
        [32.0, 32.0],
        [16.0, 16.0],
        [1.0, 1.0, 1.0, 1.0], // white at center
        [0.0, 1.0, 0.0, 1.0], // green at boundary
    );

    let bytes = render_to_bytes(&renderer, &scene);

    // Corners: way outside r=16. Should be pure green.
    for &(x, y) in &[(0_u32, 0_u32), (63, 0), (0, 63), (63, 63)] {
        assert_within_tol(
            pixel(&bytes, W, x, y),
            [0, 255, 0, 255],
            2,
            &format!("corner ({}, {}) clamped to color1", x, y),
        );
    }
    // Dead center should be near-white (small t close to 0).
    let center = pixel(&bytes, W, 32, 32);
    assert!(
        center[0] > 230 && center[1] > 230 && center[2] > 230,
        "center pixel {:?} should be near-white",
        center
    );
}

/// Elliptical: rx = 24, ry = 12, centered at (32, 32). Verifies that
/// the per-axis normalization works: a pixel at offset (rx/2, 0) and
/// one at (0, ry/2) — different geometric distances — produce the
/// same `t` and therefore the same mixed color.
#[test]
fn p8b_03_elliptical_radii() {
    let renderer = make_renderer();
    let mut scene = Scene::new(W, H);
    let cx = 32.0_f32;
    let cy = 32.0_f32;
    let rx = 24.0_f32;
    let ry = 12.0_f32;
    scene.push_radial_gradient(
        0.0, 0.0, W as f32, H as f32,
        [cx, cy],
        [rx, ry],
        [1.0, 0.0, 0.0, 1.0],
        [0.0, 0.0, 1.0, 1.0],
    );

    let bytes = render_to_bytes(&renderer, &scene);

    // Compute the expected color for a pixel at integer (x, y) with
    // pixel-center at (x+0.5, y+0.5).
    let expected_at = |x: u32, y: u32| -> [u8; 4] {
        let dx = (x as f32 + 0.5 - cx) / rx;
        let dy = (y as f32 + 0.5 - cy) / ry;
        let t = (dx * dx + dy * dy).sqrt().clamp(0.0, 1.0);
        [srgb_encode(1.0 - t), 0, srgb_encode(t), 255]
    };

    // Sample one pixel along each axis and a diagonal pixel to confirm
    // the elliptical normalization (not just circular by accident).
    for &(x, y) in &[(44_u32, 32_u32), (32, 38), (40, 36)] {
        assert_within_tol(
            pixel(&bytes, W, x, y),
            expected_at(x, y),
            2,
            &format!("pixel ({}, {})", x, y),
        );
    }
}

/// Linear gradient pushed first, radial gradient pushed second.
/// Per the Phase 8B family ordering (linear < radial), the radial
/// must overwrite the linear at pixels inside its boundary.
#[test]
fn p8b_04_paints_in_front_of_linear() {
    let renderer = make_renderer();
    let mut scene = Scene::new(W, H);
    // Linear backdrop: solid green-to-green (constant green).
    scene.push_linear_gradient(
        0.0, 0.0, W as f32, H as f32,
        [0.0, 0.0],
        [W as f32, 0.0],
        [0.0, 1.0, 0.0, 1.0],
        [0.0, 1.0, 0.0, 1.0],
    );
    // Radial overlay: red center, blue boundary, full coverage.
    scene.push_radial_gradient(
        0.0, 0.0, W as f32, H as f32,
        [32.0, 32.0],
        [32.0, 32.0],
        [1.0, 0.0, 0.0, 1.0],
        [0.0, 0.0, 1.0, 1.0],
    );

    let bytes = render_to_bytes(&renderer, &scene);

    // The radial gradient with depth-write opaque overlays the linear
    // gradient underneath. At every visible pixel we must see the
    // radial's color (red→blue), not the linear's green. The exact
    // radial color depends on the pixel's distance from center.
    let radial_expected = |x: u32, y: u32| -> [u8; 4] {
        let dx = (x as f32 + 0.5) - 32.0;
        let dy = (y as f32 + 0.5) - 32.0;
        let t = ((dx * dx + dy * dy).sqrt() / 32.0).clamp(0.0, 1.0);
        [srgb_encode(1.0 - t), 0, srgb_encode(t), 255]
    };

    // Sample the dead center, an axial point, and a corner.
    for &(x, y) in &[(32_u32, 32_u32), (0, 32), (0, 0), (63, 63)] {
        let expected = radial_expected(x, y);
        let actual = pixel(&bytes, W, x, y);
        assert_within_tol(actual, expected, 2, &format!("({}, {}) is radial, not green", x, y));
        // Also check green channel is essentially zero — proves the
        // linear backdrop doesn't bleed through.
        assert!(
            actual[1] < 5,
            "({}, {}): green channel {} suggests linear backdrop bleeding through",
            x, y, actual[1]
        );
    }
}
