/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 7C receipt — composite integration.
//!
//! 7C wires the tile cache into `Renderer::prepare()`: when the renderer
//! was constructed with `NetrenderOptions::tile_cache_size = Some(_)`,
//! `prepare()` invalidates, re-renders dirty tiles, and returns one
//! `brush_image_alpha` composite draw per tile.
//!
//! Receipt:
//!   p7c_01_pixel_equivalence_solid_rects — same scene, direct vs. tiled
//!                                          paths produce framebuffers
//!                                          equal within ±2/255.
//!   p7c_02_unchanged_frame_zero_dirty    — `prepare()` twice on the
//!                                          same scene leaves dirty=0.
//!   p7c_03_translation_dirties_two_tiles — small scroll dirties exactly
//!                                          two tiles regardless of
//!                                          viewport size.

use netrender::{
    ColorLoad, FrameTarget, NetrenderOptions, Renderer, Scene, boot, create_netrender_instance,
};

const VIEWPORT: u32 = 128;
const TILE_SIZE: u32 = 64;
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

// ── Helpers ────────────────────────────────────────────────────────────────

fn make_renderer(tile_cache_size: Option<u32>) -> Renderer {
    let handles = boot().expect("wgpu boot");
    create_netrender_instance(
        handles,
        NetrenderOptions {
            tile_cache_size,
            ..Default::default()
        },
    )
    .expect("create_netrender_instance")
}

/// Render `scene` through `renderer` to a fresh `Rgba8UnormSrgb` target;
/// return the readback bytes.
fn render_to_bytes(renderer: &Renderer, scene: &Scene) -> Vec<u8> {
    let device = renderer.wgpu_device.core.device.clone();
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("p7c target"),
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

fn dirty_count_after_prepare(renderer: &Renderer) -> usize {
    renderer
        .tile_cache()
        .expect("tile cache enabled")
        .lock()
        .expect("tile_cache lock")
        .dirty_count_last_invalidate()
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// Pixel equivalence: rendering a scene through the tile cache must
/// match rendering it directly, within ±2/255 per channel. The tolerance
/// covers any sRGB round-trip rounding (tile texture is `Rgba8Unorm`
/// linear; framebuffer is `Rgba8UnormSrgb`).
#[test]
fn p7c_01_pixel_equivalence_solid_rects() {
    let mut scene = Scene::new(VIEWPORT, VIEWPORT);
    // A red opaque rect, a green opaque rect, and a 50%-alpha blue rect
    // overlapping both — exercises opaque depth + alpha blending across
    // multiple tiles.
    scene.push_rect(8.0, 8.0, 56.0, 56.0, [1.0, 0.0, 0.0, 1.0]);
    scene.push_rect(70.0, 70.0, 120.0, 120.0, [0.0, 1.0, 0.0, 1.0]);
    // Premultiplied 50% blue: [0, 0, 0.5, 0.5]
    scene.push_rect(40.0, 40.0, 90.0, 90.0, [0.0, 0.0, 0.5, 0.5]);

    let direct = render_to_bytes(&make_renderer(None), &scene);
    let tiled = render_to_bytes(&make_renderer(Some(TILE_SIZE)), &scene);

    assert_eq!(direct.len(), tiled.len());
    let mut max_diff: u8 = 0;
    let mut diff_count = 0usize;
    for (a, b) in direct.iter().zip(tiled.iter()) {
        let d = (*a as i16 - *b as i16).unsigned_abs() as u8;
        if d > 2 {
            diff_count += 1;
        }
        max_diff = max_diff.max(d);
    }
    assert_eq!(
        diff_count, 0,
        "pixel equivalence failed: {} channels diverged by >2/255 (max diff = {})",
        diff_count, max_diff
    );
}

/// Receipt clause: "unchanged frame reuses 100% of tiles." After a
/// second `prepare()` on the same scene, dirty count is 0.
#[test]
fn p7c_02_unchanged_frame_zero_dirty() {
    let renderer = make_renderer(Some(TILE_SIZE));
    let mut scene = Scene::new(VIEWPORT, VIEWPORT);
    scene.push_rect(10.0, 10.0, 30.0, 30.0, [1.0, 0.0, 0.0, 1.0]);

    let _ = render_to_bytes(&renderer, &scene);
    let dirty1 = dirty_count_after_prepare(&renderer);
    assert!(dirty1 > 0, "first prepare should dirty some tiles, got {}", dirty1);

    let _ = render_to_bytes(&renderer, &scene);
    let dirty2 = dirty_count_after_prepare(&renderer);
    assert_eq!(
        dirty2, 0,
        "unchanged scene must dirty zero tiles on re-prepare, got {}",
        dirty2
    );
}

/// Receipt clause: "tile re-render count proportional to scroll delta,
/// not viewport size." Single rect that crosses one tile boundary
/// dirties exactly two tiles, regardless of how many tiles the viewport
/// contains.
#[test]
fn p7c_03_translation_dirties_two_tiles() {
    fn dirty_count_for(viewport_w: u32, viewport_h: u32) -> usize {
        let renderer = make_renderer(Some(TILE_SIZE));

        // Frame 1: rect inside tile (0, 0).
        let mut s1 = Scene::new(viewport_w, viewport_h);
        s1.push_rect(10.0, 10.0, 30.0, 30.0, [1.0, 0.0, 0.0, 1.0]);
        let _ = render_to_bytes(&renderer, &s1);

        // Frame 2: rect translated into tile (1, 1).
        let mut s2 = Scene::new(viewport_w, viewport_h);
        s2.push_rect(74.0, 74.0, 94.0, 94.0, [1.0, 0.0, 0.0, 1.0]);
        let _ = render_to_bytes(&renderer, &s2);

        dirty_count_after_prepare(&renderer)
    }

    // The same scroll delta dirties the same number of tiles regardless
    // of viewport size — that's the whole point of picture caching.
    for (w, h) in [(128_u32, 128_u32), (512, 512), (1024, 256)] {
        let d = dirty_count_for(w, h);
        assert_eq!(
            d, 2,
            "small scroll must dirty exactly 2 tiles regardless of viewport ({} x {}), got {}",
            w, h, d
        );
    }
}
