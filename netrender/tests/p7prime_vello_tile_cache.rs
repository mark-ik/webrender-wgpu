/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 7' — VelloTileRasterizer receipts.
//!
//! - `p7prime_01_first_frame_all_tiles_dirty` — full-canvas red
//!   rect on a 4-tile (2×2) grid; first render marks all 4 tiles
//!   dirty, output renders correctly.
//! - `p7prime_02_unchanged_scene_no_dirty` — second render of the
//!   same scene reports 0 dirty tiles, output identical.
//! - `p7prime_03_localized_change` — modify only one rect's color;
//!   only its tile re-renders.
//! - `p7prime_04_spanning_primitive_no_double_render` — a half-
//!   alpha rect spanning multiple tiles must NOT double-blend at
//!   tile borders (per-tile clip layer prevents that).

use netrender::{
    Scene, TileCache, boot,
    vello_tile_rasterizer::VelloTileRasterizer,
};
use vello::peniko::Color;

const TRANSPARENT: Color = Color::new([0.0, 0.0, 0.0, 0.0]);

const VIEWPORT: u32 = 128;
const TILE_SIZE: u32 = 64;

fn make_target(device: &wgpu::Device) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("p7' target"),
        size: wgpu::Extent3d {
            width: VIEWPORT,
            height: VIEWPORT,
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
        label: Some("p7' storage view"),
        format: Some(wgpu::TextureFormat::Rgba8Unorm),
        ..Default::default()
    });
    (texture, view)
}

fn read_pixel(bytes: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * VIEWPORT + x) * 4) as usize;
    [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]
}

#[track_caller]
fn assert_within_tol(actual: [u8; 4], expected: [u8; 4], tol: u8, where_: &str) {
    let max = (0..4)
        .map(|i| (actual[i] as i16 - expected[i] as i16).unsigned_abs() as u8)
        .max()
        .unwrap();
    assert!(
        max <= tol,
        "{}: actual {:?}, expected {:?} (max channel diff = {}, tol = {})",
        where_, actual, expected, max, tol
    );
}

/// Full-canvas opaque red rect rendered through the tile cache.
/// First frame marks all 4 tiles dirty. Output is byte-exact red
/// over the entire viewport.
#[test]
fn p7prime_01_first_frame_all_tiles_dirty() {
    let handles = boot().expect("wgpu boot");
    let mut rasterizer = VelloTileRasterizer::new(handles.clone())
        .expect("VelloTileRasterizer::new");
    let mut tc = TileCache::new(TILE_SIZE);

    let mut scene = Scene::new(VIEWPORT, VIEWPORT);
    scene.push_rect(0.0, 0.0, VIEWPORT as f32, VIEWPORT as f32, [1.0, 0.0, 0.0, 1.0]);

    let (target, view) = make_target(&handles.device);
    rasterizer
        .render(&scene, &mut tc, &view, TRANSPARENT)
        .expect("render");

    assert_eq!(rasterizer.last_dirty_count(), 4, "first frame: all 4 tiles dirty");
    assert_eq!(rasterizer.cached_tile_count(), 4);

    let wgpu_device = netrender_device::WgpuDevice::with_external(handles.clone())
        .expect("WgpuDevice::with_external");
    let bytes = wgpu_device.read_rgba8_texture(&target, VIEWPORT, VIEWPORT);

    // Sample one interior pixel per tile + one corner.
    for &(x, y) in &[(16, 16), (96, 16), (16, 96), (96, 96), (64, 64)] {
        assert_within_tol(read_pixel(&bytes, x, y), [255, 0, 0, 255], 0, &format!("({}, {})", x, y));
    }
}

/// Re-render the same scene: zero tiles should be dirty, and the
/// output bytes must match the prior frame exactly.
#[test]
fn p7prime_02_unchanged_scene_no_dirty() {
    let handles = boot().expect("wgpu boot");
    let mut rasterizer = VelloTileRasterizer::new(handles.clone())
        .expect("VelloTileRasterizer::new");
    let mut tc = TileCache::new(TILE_SIZE);

    let mut scene = Scene::new(VIEWPORT, VIEWPORT);
    scene.push_rect(0.0, 0.0, VIEWPORT as f32, VIEWPORT as f32, [0.0, 1.0, 0.0, 1.0]);

    let (target_a, view_a) = make_target(&handles.device);
    rasterizer.render(&scene, &mut tc, &view_a, TRANSPARENT).expect("render 1");
    assert_eq!(rasterizer.last_dirty_count(), 4);

    let (target_b, view_b) = make_target(&handles.device);
    rasterizer.render(&scene, &mut tc, &view_b, TRANSPARENT).expect("render 2");
    assert_eq!(rasterizer.last_dirty_count(), 0, "second frame: no tiles dirty");

    let wgpu_device = netrender_device::WgpuDevice::with_external(handles.clone())
        .expect("WgpuDevice::with_external");
    let bytes_a = wgpu_device.read_rgba8_texture(&target_a, VIEWPORT, VIEWPORT);
    let bytes_b = wgpu_device.read_rgba8_texture(&target_b, VIEWPORT, VIEWPORT);
    assert_eq!(bytes_a, bytes_b, "second-frame output must match first-frame output");
}

/// Modify only the top-left rect's color: only its tile should be
/// dirty on the second render.
#[test]
fn p7prime_03_localized_change() {
    let handles = boot().expect("wgpu boot");
    let mut rasterizer = VelloTileRasterizer::new(handles.clone())
        .expect("VelloTileRasterizer::new");
    let mut tc = TileCache::new(TILE_SIZE);

    // Four small rects, one per tile, no overlap.
    fn build_scene(top_left_color: [f32; 4]) -> Scene {
        let mut s = Scene::new(VIEWPORT, VIEWPORT);
        s.push_rect(8.0,  8.0,   56.0, 56.0,  top_left_color);   // top-left tile
        s.push_rect(72.0, 8.0,   120.0, 56.0, [0.0, 1.0, 0.0, 1.0]);
        s.push_rect(8.0,  72.0,  56.0, 120.0, [0.0, 0.0, 1.0, 1.0]);
        s.push_rect(72.0, 72.0,  120.0, 120.0, [1.0, 1.0, 0.0, 1.0]);
        s
    }

    let scene_a = build_scene([1.0, 0.0, 0.0, 1.0]);  // red
    let (_t1, v1) = make_target(&handles.device);
    rasterizer.render(&scene_a, &mut tc, &v1, TRANSPARENT).expect("render 1");
    assert_eq!(rasterizer.last_dirty_count(), 4);

    // Now change ONLY the top-left rect to magenta. The other three
    // tiles' dependency hashes are unchanged, so the tile cache
    // marks only one tile dirty.
    let scene_b = build_scene([1.0, 0.0, 1.0, 1.0]);
    let (_t2, v2) = make_target(&handles.device);
    rasterizer.render(&scene_b, &mut tc, &v2, TRANSPARENT).expect("render 2");
    assert_eq!(
        rasterizer.last_dirty_count(),
        1,
        "single-rect color change should dirty only its tile"
    );
}

/// A half-alpha red rect spanning multiple tiles must not be
/// double-blended at the tile boundary. With straight-alpha storage
/// the rect should produce uniform `(255, 0, 0, 128)` everywhere it
/// covers, including pixels right next to the tile boundary.
#[test]
fn p7prime_04_spanning_primitive_no_double_render() {
    let handles = boot().expect("wgpu boot");
    let mut rasterizer = VelloTileRasterizer::new(handles.clone())
        .expect("VelloTileRasterizer::new");
    let mut tc = TileCache::new(TILE_SIZE);

    let mut scene = Scene::new(VIEWPORT, VIEWPORT);
    // Premultiplied half-alpha red is (0.5, 0, 0, 0.5). With straight-
    // alpha storage (per p1prime_02), output bytes are (255, 0, 0, 128).
    scene.push_rect(0.0, 0.0, VIEWPORT as f32, VIEWPORT as f32, [0.5, 0.0, 0.0, 0.5]);

    let (target, view) = make_target(&handles.device);
    rasterizer.render(&scene, &mut tc, &view, TRANSPARENT).expect("render");

    let wgpu_device = netrender_device::WgpuDevice::with_external(handles.clone())
        .expect("WgpuDevice::with_external");
    let bytes = wgpu_device.read_rgba8_texture(&target, VIEWPORT, VIEWPORT);

    // Sample on either side of the tile boundary at x = 64 and the
    // boundary itself. If the rect were double-rendered in
    // overlapping tiles, the border-adjacent pixels would have
    // different alpha than the interior. Per-tile clip layers
    // prevent that.
    for &(x, y) in &[(32, 32), (63, 32), (64, 32), (65, 32), (96, 32), (32, 64), (96, 96)] {
        assert_within_tol(
            read_pixel(&bytes, x, y),
            [255, 0, 0, 128],
            2,
            &format!("spanning rect at ({}, {})", x, y),
        );
    }
}
