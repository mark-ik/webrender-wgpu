/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 7B receipt — per-tile rendering.
//!
//! 7B renders dirty tiles into per-tile `Arc<wgpu::Texture>`s using the
//! existing `brush_rect_solid` / `brush_image` pipelines plus a tile-local
//! orthographic projection. Composite-to-framebuffer is Phase 7C; for now,
//! these tests verify:
//!
//!   p7b_01_dirty_tiles_get_textures        — every dirty tile has a texture after render
//!   p7b_02_clean_tiles_keep_their_texture  — no-change re-render preserves Arc identity
//!   p7b_03_tile_pixels_match_world_position — read back tile (0,0); rect at world
//!                                              (10,10)-(30,30) appears at tile-local
//!                                              (10,10)-(30,30) as opaque red
//!   p7b_04_translation_invalidates_only_two_tiles — Arc identity preserved on clean tiles
//!                                                    after a 1-tile-crossing translation

use std::sync::Arc;

use netrender::{NetrenderOptions, Scene, TileCache, boot, create_netrender_instance};

const VIEWPORT: u32 = 128;
const TILE_SIZE: u32 = 64;

// Build a renderer for tile-cache tests.
fn make_renderer() -> netrender::Renderer {
    let handles = boot().expect("wgpu boot");
    create_netrender_instance(handles, NetrenderOptions::default())
        .expect("create_netrender_instance")
}

#[test]
fn p7b_01_dirty_tiles_get_textures() {
    let renderer = make_renderer();
    let mut tc = TileCache::new(TILE_SIZE);

    // 128×128 viewport / 64-pixel tiles → 2×2 = 4 tiles. First render
    // marks every tile dirty (all are new) and renders each into its
    // cached texture.
    let mut scene = Scene::new(VIEWPORT, VIEWPORT);
    scene.push_rect(10.0, 10.0, 30.0, 30.0, [1.0, 0.0, 0.0, 1.0]);

    let dirty = renderer.render_dirty_tiles(&scene, &mut tc);
    assert_eq!(dirty.len(), 4, "expected 4 dirty tiles on first render, got {}", dirty.len());

    for &coord in &dirty {
        assert!(
            tc.tile_texture(coord).is_some(),
            "tile {:?} should have a cached texture after render_dirty_tiles",
            coord
        );
    }
}

#[test]
fn p7b_02_clean_tiles_keep_their_texture() {
    let renderer = make_renderer();
    let mut tc = TileCache::new(TILE_SIZE);

    let mut scene = Scene::new(VIEWPORT, VIEWPORT);
    scene.push_rect(10.0, 10.0, 30.0, 30.0, [1.0, 0.0, 0.0, 1.0]);

    let _ = renderer.render_dirty_tiles(&scene, &mut tc);
    let tex_before = tc.tile_texture((1, 1)).expect("(1,1) should have a texture");

    // Re-render the same scene. No tile should be dirty, and the texture
    // for (1,1) (which holds nothing changed) must be preserved by Arc.
    let dirty2 = renderer.render_dirty_tiles(&scene, &mut tc);
    assert!(
        dirty2.is_empty(),
        "no-op re-render should mark zero tiles dirty, got {:?}",
        dirty2
    );

    let tex_after = tc.tile_texture((1, 1)).expect("(1,1) should still have its texture");
    assert!(
        Arc::ptr_eq(&tex_before, &tex_after),
        "clean tile must keep the same Arc<wgpu::Texture>"
    );
}

#[test]
fn p7b_03_tile_pixels_match_world_position() {
    let renderer = make_renderer();
    let mut tc = TileCache::new(TILE_SIZE);

    // Single opaque red rect in tile (0,0)'s region only.
    let mut scene = Scene::new(VIEWPORT, VIEWPORT);
    scene.push_rect(10.0, 10.0, 30.0, 30.0, [1.0, 0.0, 0.0, 1.0]);

    let _ = renderer.render_dirty_tiles(&scene, &mut tc);

    let tile00 = tc.tile_texture((0, 0)).expect("(0,0) tile texture");
    let pixels = renderer
        .wgpu_device
        .read_rgba8_texture(&tile00, TILE_SIZE, TILE_SIZE);
    assert_eq!(pixels.len(), (TILE_SIZE * TILE_SIZE * 4) as usize);

    // Pixels inside the rect should be opaque red; outside should be
    // transparent (clear color). Sample a few specific points.
    let pixel = |x: u32, y: u32| -> [u8; 4] {
        let i = ((y * TILE_SIZE + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    };

    // Inside the rect: world (15, 15) maps to tile-local (15, 15).
    assert_eq!(pixel(15, 15), [255, 0, 0, 255], "pixel inside rect should be opaque red");
    // Just inside the rect's right edge (rect ends at x=30, exclusive).
    assert_eq!(pixel(29, 15), [255, 0, 0, 255], "pixel at (29,15) inside rect");
    // Outside the rect (top-right of tile).
    assert_eq!(pixel(50, 15), [0, 0, 0, 0], "pixel outside rect should be transparent");
    // Just past the rect's right edge.
    assert_eq!(pixel(30, 15), [0, 0, 0, 0], "pixel at (30,15) is just past rect.x1");

    // Tile (1, 1) covers world (64,64)-(128,128); the rect doesn't reach it.
    let tile11 = tc.tile_texture((1, 1)).expect("(1,1) tile texture");
    let pixels11 = renderer
        .wgpu_device
        .read_rgba8_texture(&tile11, TILE_SIZE, TILE_SIZE);
    assert!(
        pixels11.chunks_exact(4).all(|p| p == [0, 0, 0, 0]),
        "tile (1,1) should be entirely transparent (no primitives in its world rect)"
    );
}

#[test]
fn p7b_04_translation_invalidates_only_two_tiles() {
    let renderer = make_renderer();
    let mut tc = TileCache::new(TILE_SIZE);

    // Frame 1: rect in tile (0,0).
    let mut s1 = Scene::new(VIEWPORT, VIEWPORT);
    s1.push_rect(10.0, 10.0, 30.0, 30.0, [1.0, 0.0, 0.0, 1.0]);
    let _ = renderer.render_dirty_tiles(&s1, &mut tc);

    // Snapshot tile-(0,1) and tile-(1,0) Arcs — these don't intersect
    // the rect in either frame, so their textures must be preserved.
    let tex_01_before = tc.tile_texture((0, 1)).expect("(0,1) texture");
    let tex_10_before = tc.tile_texture((1, 0)).expect("(1,0) texture");

    // Frame 2: rect translated into tile (1,1). Dirty: (0,0) (rect left)
    // and (1,1) (rect entered). (0,1) and (1,0) stay clean.
    let mut s2 = Scene::new(VIEWPORT, VIEWPORT);
    s2.push_rect(74.0, 74.0, 94.0, 94.0, [1.0, 0.0, 0.0, 1.0]);
    let dirty = renderer.render_dirty_tiles(&s2, &mut tc);
    assert_eq!(dirty.len(), 2, "expected exactly 2 dirty tiles, got {:?}", dirty);

    let tex_01_after = tc.tile_texture((0, 1)).expect("(0,1) texture");
    let tex_10_after = tc.tile_texture((1, 0)).expect("(1,0) texture");
    assert!(
        Arc::ptr_eq(&tex_01_before, &tex_01_after),
        "(0,1) untouched by translation must keep its texture"
    );
    assert!(
        Arc::ptr_eq(&tex_10_before, &tex_10_after),
        "(1,0) untouched by translation must keep its texture"
    );

    // The dirty tiles got fresh textures.
    let tile00 = tc.tile_texture((0, 0)).expect("(0,0) re-rendered");
    let pixels00 = renderer
        .wgpu_device
        .read_rgba8_texture(&tile00, TILE_SIZE, TILE_SIZE);
    assert!(
        pixels00.chunks_exact(4).all(|p| p == [0, 0, 0, 0]),
        "(0,0) post-translation should be empty (rect left)"
    );
    let tile11 = tc.tile_texture((1, 1)).expect("(1,1) re-rendered");
    let pixels11 = renderer
        .wgpu_device
        .read_rgba8_texture(&tile11, TILE_SIZE, TILE_SIZE);
    // Rect (74,74)-(94,94) → tile-(1,1)-local (10,10)-(30,30).
    let pixel11 = |x: u32, y: u32| -> [u8; 4] {
        let i = ((y * TILE_SIZE + x) * 4) as usize;
        [pixels11[i], pixels11[i + 1], pixels11[i + 2], pixels11[i + 3]]
    };
    assert_eq!(pixel11(15, 15), [255, 0, 0, 255], "rect should appear in tile (1,1)");
}
