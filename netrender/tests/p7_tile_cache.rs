/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 7A receipt — tile invalidation algorithm.
//!
//! These tests exercise the *algorithm* portion of Phase 7: dependency
//! hashing, frame stamps, dirty tracking. No GPU work — `TileCache` is
//! pure CPU state in this slice. Phase 7B + 7C add per-tile rendering
//! and composite integration; those receipts go in `p7b_*` / `p7c_*`.
//!
//! The receipt clauses from the design plan are:
//!   - "unchanged frame reuses 100% of tiles" → p7_01_identical_scene_zero_dirty
//!   - "small scroll only renders newly-exposed strips" → p7_02_translation_dirties_only_affected
//!   - "tile re-render count proportional to scroll delta, not viewport size" → p7_03_dirty_count_independent_of_viewport_size

use std::collections::HashSet;

use netrender::{Scene, TileCache, TileCoord};

// ── Helpers ────────────────────────────────────────────────────────────────

fn one_rect_scene(viewport: (u32, u32), rect: [f32; 4], color: [f32; 4]) -> Scene {
    let mut s = Scene::new(viewport.0, viewport.1);
    s.push_rect(rect[0], rect[1], rect[2], rect[3], color);
    s
}

fn dirty_set(dirty: Vec<TileCoord>) -> HashSet<TileCoord> {
    dirty.into_iter().collect()
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// First invalidate populates all tiles in the viewport grid (every tile
/// is "new" → marked dirty). A second invalidate against the *same* scene
/// reports zero dirty tiles — the receipt's "unchanged frame reuses 100%
/// of tiles" property.
#[test]
fn p7_01_identical_scene_zero_dirty() {
    // 256×256 viewport, 64-pixel tiles → 4×4 grid, 16 tiles total.
    let mut tc = TileCache::new(64);
    let scene = one_rect_scene((256, 256), [10.0, 10.0, 30.0, 30.0], [1.0, 0.0, 0.0, 1.0]);

    let dirty1 = tc.invalidate(&scene);
    assert_eq!(
        dirty1.len(),
        16,
        "first invalidate should mark every tile dirty (all are new); got {}",
        dirty1.len()
    );
    assert_eq!(tc.tile_count(), 16);
    assert_eq!(tc.current_frame(), 1);

    let dirty2 = tc.invalidate(&scene);
    assert_eq!(
        dirty2.len(),
        0,
        "identical scene must report zero dirty tiles, got {}: {:?}",
        dirty2.len(),
        dirty2
    );
    assert_eq!(tc.dirty_count_last_invalidate(), 0);
    assert_eq!(tc.current_frame(), 2);
}

/// Translating a small rect from tile (0,0) to tile (1,1) must dirty
/// exactly those two tiles — the rect *left* (0,0) and *entered* (1,1),
/// changing both tiles' dependency hashes. Every other tile's hash is
/// unchanged (still empty).
#[test]
fn p7_02_translation_dirties_only_affected_tiles() {
    let mut tc = TileCache::new(64);

    // Frame 1: rect inside tile (0,0).
    let s1 = one_rect_scene((256, 256), [10.0, 10.0, 30.0, 30.0], [1.0, 0.0, 0.0, 1.0]);
    let _ = tc.invalidate(&s1);

    // Frame 2: same rect translated into tile (1,1).
    let s2 = one_rect_scene((256, 256), [74.0, 74.0, 94.0, 94.0], [1.0, 0.0, 0.0, 1.0]);
    let dirty = tc.invalidate(&s2);

    let dirty = dirty_set(dirty);
    let expected: HashSet<TileCoord> = [(0, 0), (1, 1)].iter().copied().collect();
    assert_eq!(
        dirty, expected,
        "expected exactly tiles (0,0) and (1,1) dirty after translation; got {:?}",
        dirty
    );
}

/// "Tile re-render count proportional to scroll delta, not viewport size."
///
/// Scenes with a single small primitive: a tiny scroll within the same
/// tile dirties just that tile; a scroll that crosses one tile boundary
/// dirties exactly two tiles; scaling the viewport up by 4× does not
/// change those counts.
#[test]
fn p7_03_dirty_count_independent_of_viewport_size() {
    fn dirty_count_for(viewport: (u32, u32), s1_rect: [f32; 4], s2_rect: [f32; 4]) -> usize {
        let mut tc = TileCache::new(64);
        let s1 = one_rect_scene(viewport, s1_rect, [1.0, 0.0, 0.0, 1.0]);
        let _ = tc.invalidate(&s1);

        let s2 = one_rect_scene(viewport, s2_rect, [1.0, 0.0, 0.0, 1.0]);
        tc.invalidate(&s2).len()
    }

    // Tiny scroll: rect stays inside tile (0,0).
    let small_scroll_a = [10.0, 10.0, 30.0, 30.0];
    let small_scroll_b = [15.0, 15.0, 35.0, 35.0];

    // Cross-tile scroll: rect crosses from tile (0,0) into tile (1,1)
    // (touches both tiles in s2_rect, plus leaves a previously-touched tile).
    let big_scroll_a = [10.0, 10.0, 30.0, 30.0];
    let big_scroll_b = [74.0, 74.0, 94.0, 94.0];

    // Three viewports of increasing size, all using 64-pixel tiles:
    //   256×256   → 16 tiles
    //   1024×1024 → 256 tiles  (16× more tiles than the small viewport)
    //   2048×512  → 256 tiles  (skewed aspect ratio, same count)
    for viewport in [(256, 256), (1024, 1024), (2048, 512)] {
        let small = dirty_count_for(viewport, small_scroll_a, small_scroll_b);
        let big = dirty_count_for(viewport, big_scroll_a, big_scroll_b);
        assert_eq!(
            small, 1,
            "tiny scroll within one tile must dirty 1 tile (viewport {:?})",
            viewport
        );
        assert_eq!(
            big, 2,
            "cross-tile scroll must dirty exactly 2 tiles (viewport {:?})",
            viewport
        );
    }
}

/// Edge case: empty scene. Every tile in the viewport gets the same
/// (empty) hash. First call marks everything dirty (all new); second
/// call marks nothing dirty.
#[test]
fn p7_04_empty_scene_stable_after_first_frame() {
    let mut tc = TileCache::new(64);
    let scene = Scene::new(128, 128); // 2×2 = 4 tiles

    let dirty1 = tc.invalidate(&scene);
    assert_eq!(dirty1.len(), 4, "first invalidate marks all 4 tiles new");

    let dirty2 = tc.invalidate(&scene);
    assert_eq!(dirty2.len(), 0, "empty scene re-invalidate dirties nothing");
}

/// Color change on a primitive: only tiles overlapping that primitive
/// see a hash change. Tiles outside the prim's footprint stay clean.
#[test]
fn p7_05_color_change_dirties_only_affected_tiles() {
    let mut tc = TileCache::new(64);

    let s1 = one_rect_scene((256, 256), [10.0, 10.0, 30.0, 30.0], [1.0, 0.0, 0.0, 1.0]);
    let _ = tc.invalidate(&s1);

    // Same rect, different color → tile (0,0) hash flips, others unchanged.
    let s2 = one_rect_scene((256, 256), [10.0, 10.0, 30.0, 30.0], [0.0, 1.0, 0.0, 1.0]);
    let dirty = tc.invalidate(&s2);

    assert_eq!(dirty, vec![(0, 0)], "color change must dirty only the rect's tile");
}

/// Regression for the Phase 8A oversight: gradient primitives must
/// participate in the per-tile dependency hash. Pre-fix, a gradient
/// color change on an existing scene would return 0 dirty tiles
/// (because the hash only looked at rects + images), and the tile
/// cache would happily serve stale cached pixels.
#[test]
fn p7_07_gradient_change_dirties_only_its_tile() {
    let mut tc = TileCache::new(64);

    let mut s1 = Scene::new(256, 256);
    // Gradient confined to tile (0, 0) — world rect (0..32, 0..32).
    s1.push_linear_gradient(
        0.0, 0.0, 32.0, 32.0,
        [0.0, 0.0],
        [32.0, 0.0],
        [1.0, 0.0, 0.0, 1.0],
        [0.0, 0.0, 1.0, 1.0],
    );
    let _ = tc.invalidate(&s1);

    // Identical scene → 0 dirty.
    let dirty_unchanged = tc.invalidate(&s1);
    assert_eq!(
        dirty_unchanged.len(),
        0,
        "identical gradient scene must dirty 0 tiles, got {}",
        dirty_unchanged.len()
    );

    // Change one stop color. Pre-fix, this returned 0 (the bug);
    // post-fix, only tile (0, 0) is dirty.
    let mut s2 = Scene::new(256, 256);
    s2.push_linear_gradient(
        0.0, 0.0, 32.0, 32.0,
        [0.0, 0.0],
        [32.0, 0.0],
        [0.0, 1.0, 0.0, 1.0], // green instead of red
        [0.0, 0.0, 1.0, 1.0],
    );
    let dirty = tc.invalidate(&s2);
    assert_eq!(
        dirty,
        vec![(0, 0)],
        "gradient color change must dirty exactly tile (0, 0), got {:?}",
        dirty
    );
}

/// Adding a primitive only dirties tiles it touches; removing one
/// dirties exactly the tiles it used to touch.
#[test]
fn p7_06_add_remove_primitive_localizes_dirt() {
    let mut tc = TileCache::new(64);

    // Frame 1: empty scene.
    let s_empty = Scene::new(256, 256);
    let _ = tc.invalidate(&s_empty);

    // Frame 2: add a rect in tile (2, 1) world rect (128, 64) - (192, 128).
    let s_with_rect = one_rect_scene((256, 256), [140.0, 80.0, 180.0, 120.0], [1.0, 0.0, 0.0, 1.0]);
    let dirty_add = tc.invalidate(&s_with_rect);
    assert_eq!(dirty_add, vec![(2, 1)], "adding a rect dirties only its tile");

    // Frame 3: remove the rect (back to empty).
    let dirty_remove = tc.invalidate(&s_empty);
    assert_eq!(dirty_remove, vec![(2, 1)], "removing the rect dirties only that same tile");
}
