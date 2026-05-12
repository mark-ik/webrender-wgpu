/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 11a' — stroked rect / rounded-rect (CSS borders).
//!
//! Receipts:
//!   p11a_01_sharp_rect_border           — sharp axis-aligned border
//!   p11a_02_rounded_rect_border         — CSS `border-radius` style
//!   p11a_03_border_under_transform      — transformed border
//!   p11a_04_inflated_aabb_for_tile_cache — strokes whose pen reaches
//!     across a tile boundary aren't dropped by the AABB filter.

use netrender::{Scene, TileCache, boot, vello_tile_rasterizer::VelloTileRasterizer};
use vello::peniko::Color;

const DIM: u32 = 64;
const TILE_SIZE: u32 = 32;
const TRANSPARENT: Color = Color::new([0.0, 0.0, 0.0, 0.0]);

fn make_target(device: &wgpu::Device, dim: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("p11a target"),
        size: wgpu::Extent3d {
            width: dim,
            height: dim,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("p11a view"),
        format: Some(wgpu::TextureFormat::Rgba8Unorm),
        ..Default::default()
    });
    (texture, view)
}

fn read_pixel(bytes: &[u8], stride_w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * stride_w + x) * 4) as usize;
    [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]
}

fn render_scene(scene: &Scene) -> Vec<u8> {
    let handles = boot().expect("wgpu boot");
    let mut rast = VelloTileRasterizer::new(handles.clone()).expect("rast");
    let mut tc = TileCache::new(TILE_SIZE);

    let (tex, view) = make_target(&handles.device, scene.viewport_width);
    rast.render(scene, &mut tc, &view, TRANSPARENT)
        .expect("render");

    let wgpu_device =
        netrender_device::WgpuDevice::with_external(handles.clone()).expect("wgpu device");
    wgpu_device.read_rgba8_texture(&tex, scene.viewport_width, scene.viewport_height)
}

/// Sharp 2-pixel red border around (16, 16)–(48, 48). Stroke is
/// centered on the path, so the painted band is at radius
/// 1px inside and 1px outside the path. Sample points:
///   - (16, 32): on the left edge of the path → red
///   - (32, 32): center of the box, inside the path → no paint
///   - (4, 4):   far outside → no paint
#[test]
fn p11a_01_sharp_rect_border() {
    let mut scene = Scene::new(DIM, DIM);
    scene.push_stroke(
        16.0,
        16.0,
        48.0,
        48.0,
        [1.0, 0.0, 0.0, 1.0], // opaque red
        2.0,
    );
    let bytes = render_scene(&scene);

    // On the left edge (x=16) — fully red.
    let edge = read_pixel(&bytes, DIM, 16, 32);
    assert!(
        edge[0] >= 240 && edge[3] >= 240,
        "left edge (16, 32): {:?} not near opaque red",
        edge
    );

    // Center of the box (interior, no fill, no stroke) — clear.
    let center = read_pixel(&bytes, DIM, 32, 32);
    assert!(
        center[3] < 8,
        "interior (32, 32): {:?} should be empty (border doesn't fill)",
        center
    );

    // Outside everything — clear.
    let outside = read_pixel(&bytes, DIM, 4, 4);
    assert!(
        outside[3] < 8,
        "outside (4, 4): {:?} should be empty",
        outside
    );
}

/// Rounded 4-pixel green border with corner radius 8 around
/// (12, 12)–(52, 52). The corner arc passes through (20, 12)
/// → (12, 20); pixel (12, 12) is *outside* the arc, so the
/// border doesn't reach there. Pixel (12, 32) is on the straight
/// portion of the left edge → red.
#[test]
fn p11a_02_rounded_rect_border() {
    let mut scene = Scene::new(DIM, DIM);
    scene.push_stroke_rounded(
        12.0,
        12.0,
        52.0,
        52.0,
        [0.0, 1.0, 0.0, 1.0], // opaque green
        4.0,
        [8.0, 8.0, 8.0, 8.0], // radius 8 on every corner
    );
    let bytes = render_scene(&scene);

    // Mid-left edge — on the straight portion, fully green.
    let edge = read_pixel(&bytes, DIM, 12, 32);
    assert!(
        edge[1] >= 200 && edge[3] >= 200,
        "rounded mid-edge (12, 32): {:?} not near opaque green",
        edge
    );

    // Far corner pixel — outside the rounded arc.
    let corner = read_pixel(&bytes, DIM, 12, 12);
    assert!(
        corner[3] < 50,
        "rounded outer-corner (12, 12): {:?} should be near-clear (radius cuts it)",
        corner
    );

    // Interior — no fill.
    let center = read_pixel(&bytes, DIM, 32, 32);
    assert!(
        center[3] < 8,
        "rounded interior (32, 32): {:?} should be empty",
        center
    );
}

/// Border under a translate transform — verifies stroke
/// `transform_id` plumbing.
#[test]
fn p11a_03_border_under_transform() {
    use netrender::Transform;
    let mut scene = Scene::new(DIM, DIM);
    let xform = scene.push_transform(Transform::translate_2d(10.0, 10.0));
    scene.push_stroke_full(
        0.0,
        0.0,
        32.0,
        32.0,
        [0.0, 0.0, 1.0, 1.0], // opaque blue
        2.0,
        netrender::SHARP_CLIP,
        xform,
        netrender::NO_CLIP,
        netrender::SHARP_CLIP,
    );
    let bytes = render_scene(&scene);

    // After translation, path is at (10, 10)–(42, 42).
    // Sample on the left edge at y=20.
    let translated_edge = read_pixel(&bytes, DIM, 10, 20);
    assert!(
        translated_edge[2] >= 240 && translated_edge[3] >= 240,
        "translated edge (10, 20): {:?} not near opaque blue",
        translated_edge
    );

    // The original (untranslated) path location should be empty.
    let untranslated = read_pixel(&bytes, DIM, 0, 16);
    assert!(
        untranslated[3] < 8,
        "untranslated location (0, 16): {:?} should be empty",
        untranslated
    );
}

/// A stroke whose path bbox falls entirely on a tile boundary.
/// With AABB inflation by `width / 2`, the stroke's painted band
/// reaches into the neighboring tile. The tile cache's filter has
/// to include both tiles or the painted band gets dropped at the
/// boundary.
#[test]
fn p11a_04_inflated_aabb_for_tile_cache() {
    // Use a 64×64 viewport with 32×32 tiles → 2×2 grid. Path at
    // exactly the tile boundary x = 32 with stroke width 4 means
    // the pen reaches from x = 30 to x = 34 — 2 pixels into the
    // right tile and 2 pixels into the left.
    let mut scene = Scene::new(DIM, DIM);
    scene.push_stroke(
        32.0,
        16.0,
        32.0001,
        48.0,                 // a vertical segment at x = 32
        [1.0, 1.0, 0.0, 1.0], // yellow
        4.0,
    );
    let bytes = render_scene(&scene);

    // The vertical stroke should produce paint in BOTH tiles.
    // Left tile (x = 30) — yellow pixels.
    let left_pen = read_pixel(&bytes, DIM, 30, 32);
    assert!(
        left_pen[0] >= 200 && left_pen[1] >= 200 && left_pen[3] >= 200,
        "left-tile pen reach (30, 32): {:?} not yellow — AABB inflation likely missing",
        left_pen
    );
    // Right tile (x = 33) — yellow pixels.
    let right_pen = read_pixel(&bytes, DIM, 33, 32);
    assert!(
        right_pen[0] >= 200 && right_pen[1] >= 200 && right_pen[3] >= 200,
        "right-tile pen reach (33, 32): {:?} not yellow — AABB inflation likely missing",
        right_pen
    );
}
