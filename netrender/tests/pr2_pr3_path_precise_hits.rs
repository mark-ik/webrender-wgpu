//! Roadmap R2 + R3 — path-precise hit testing receipts.
//!
//! R2: `SceneOp::Shape` hits respect the path interior, not just the
//! path's AABB. A point inside the AABB but outside the path doesn't
//! hit.
//!
//! R3: `SceneClip::Path` and rounded-rect `SceneClip::Rect` clips
//! gate hit results by the actual clip shape, not just its AABB.
//!
//! Both share the `kurbo::Shape::contains` machinery; tests are
//! grouped here for cohesion.

use netrender::scene::{Scene, SceneClip, ScenePath, SHARP_CLIP, Transform};
use netrender::{hit_test, hit_test_topmost};

// ── R2: Shape op path-precise ────────────────────────────────────────

/// Build a closed triangle path with vertices (0,0), (100,0),
/// (50,100). It's downward-pointing: the wide base sits along
/// `y=0` and the apex is at the bottom of the AABB.
///
/// At `y` in `[0, 100]` the triangle interior spans
/// `x ∈ [y/2, 100-y/2]`. So at `y=95` the triangle is only
/// `x ∈ [47.5, 52.5]` — a narrow band near the apex. Test points
/// like `(95, 95)` sit far outside the triangle while still being
/// inside the AABB `[0..100, 0..100]`.
fn triangle_path() -> ScenePath {
    let mut p = ScenePath::new();
    p.move_to(0.0, 0.0);
    p.line_to(100.0, 0.0);
    p.line_to(50.0, 100.0);
    p.close();
    p
}

#[test]
fn r2_triangle_centroid_hits() {
    let mut scene = Scene::new(200, 200);
    scene.push_shape_filled(triangle_path(), [1.0, 0.0, 0.0, 1.0]);

    let hit = hit_test_topmost(&scene, [50.0, 33.0]);
    assert!(hit.is_some(), "centroid of triangle hits the shape");
}

#[test]
fn r2_triangle_aabb_corner_misses() {
    // AABB is [0..100, 0..100]. At y=95 the triangle interior is
    // x ∈ [47.5, 52.5]; (95, 95) sits inside the AABB but well
    // outside the triangle (near the bottom-right AABB corner).
    let mut scene = Scene::new(200, 200);
    scene.push_shape_filled(triangle_path(), [1.0, 0.0, 0.0, 1.0]);

    let hit = hit_test_topmost(&scene, [95.0, 95.0]);
    assert!(
        hit.is_none(),
        "AABB-corner point that's outside the triangle should miss (got {hit:?})"
    );
}

#[test]
fn r2_triangle_with_transform_still_path_precise() {
    // Translate the triangle by (100, 100). Centroid moves to
    // ~(150, 133); the AABB-corner-but-outside point moves to
    // (195, 195).
    let mut scene = Scene::new(400, 400);
    let xf = scene.push_transform(Transform::translate_2d(100.0, 100.0));
    scene.ops.push(netrender::scene::SceneOp::Shape(
        netrender::scene::SceneShape {
            path: triangle_path(),
            fill_color: Some([1.0, 0.0, 0.0, 1.0]),
            stroke: None,
            transform_id: xf,
            clip_rect: netrender::NO_CLIP,
            clip_corner_radii: SHARP_CLIP,
        },
    ));

    // Local centroid (50, 33) translates to world (150, 133); inside.
    assert!(
        hit_test_topmost(&scene, [150.0, 133.0]).is_some(),
        "centroid (post-translate) hits"
    );
    // Local (95, 95) translates to world (195, 195); inside the
    // world AABB but outside the triangle.
    assert!(
        hit_test_topmost(&scene, [195.0, 195.0]).is_none(),
        "AABB corner outside triangle (post-translate) still misses"
    );
}

// ── R3: Layer-clip path-precise ──────────────────────────────────────

#[test]
fn r3_rounded_rect_clip_corner_misses() {
    // 100×100 layer with rounded clip at radius 30. The point
    // (5, 5) is inside the layer's AABB but in a clipped-out
    // corner region.
    let mut scene = Scene::new(200, 200);
    scene.push_layer_clip(SceneClip::Rect {
        rect: [0.0, 0.0, 100.0, 100.0],
        radii: [30.0, 30.0, 30.0, 30.0],
    });
    // Rect that fills the entire layer interior.
    scene.push_rect(0.0, 0.0, 100.0, 100.0, [1.0, 0.0, 0.0, 1.0]);
    scene.ops.push(netrender::scene::SceneOp::PopLayer);

    // Center of the layer — inside both AABB and rounded rect. Hit.
    assert!(
        hit_test_topmost(&scene, [50.0, 50.0]).is_some(),
        "center hits inside rounded clip"
    );
    // Top-left AABB corner — inside the AABB but outside the
    // rounded-rect interior (radius is 30; (5,5) is well within
    // the corner cutout). Miss.
    assert!(
        hit_test_topmost(&scene, [5.0, 5.0]).is_none(),
        "AABB corner misses with rounded clip (got {:?})",
        hit_test_topmost(&scene, [5.0, 5.0])
    );
}

#[test]
fn r3_rounded_rect_clip_just_inside_radius_hits() {
    // A point that's outside the corner-cutout disc but still
    // close to the AABB corner — should hit. The corner disc has
    // center at (radius, radius) and radius = radii[0], so any
    // point at distance ≤ radius from that center is inside the
    // rounded shape.
    let mut scene = Scene::new(200, 200);
    scene.push_layer_clip(SceneClip::Rect {
        rect: [0.0, 0.0, 100.0, 100.0],
        radii: [30.0, 30.0, 30.0, 30.0],
    });
    scene.push_rect(0.0, 0.0, 100.0, 100.0, [1.0, 0.0, 0.0, 1.0]);
    scene.ops.push(netrender::scene::SceneOp::PopLayer);

    // (30, 30) is exactly at the corner-disc center — interior.
    assert!(hit_test_topmost(&scene, [30.0, 30.0]).is_some());
    // (40, 40) is well inside.
    assert!(hit_test_topmost(&scene, [40.0, 40.0]).is_some());
}

#[test]
fn r3_path_clip_triangle_corner_misses() {
    // A path clip shaped as a triangle. AABB [0..100, 0..100];
    // (5, 5) is inside the AABB but outside the triangle.
    let mut scene = Scene::new(200, 200);
    scene.push_layer_clip(SceneClip::Path(triangle_path()));
    scene.push_rect(0.0, 0.0, 100.0, 100.0, [1.0, 0.0, 0.0, 1.0]);
    scene.ops.push(netrender::scene::SceneOp::PopLayer);

    assert!(
        hit_test_topmost(&scene, [50.0, 33.0]).is_some(),
        "triangle centroid hits through path clip"
    );
    assert!(
        hit_test_topmost(&scene, [95.0, 95.0]).is_none(),
        "AABB corner outside triangle clip should miss (got {:?})",
        hit_test_topmost(&scene, [95.0, 95.0])
    );
}

#[test]
fn r3_sharp_rect_clip_unchanged_by_r3_refactor() {
    // The R3 refactor must not regress the existing sharp-rect
    // clip case: a point inside the rect should hit, outside
    // should miss. This guards against an over-eager refactor
    // breaking the simple path.
    let mut scene = Scene::new(200, 200);
    scene.push_layer_clip(SceneClip::Rect {
        rect: [10.0, 10.0, 90.0, 90.0],
        radii: SHARP_CLIP,
    });
    scene.push_rect(0.0, 0.0, 200.0, 200.0, [1.0, 0.0, 0.0, 1.0]);
    scene.ops.push(netrender::scene::SceneOp::PopLayer);

    assert!(
        hit_test_topmost(&scene, [50.0, 50.0]).is_some(),
        "center hits"
    );
    assert!(
        hit_test_topmost(&scene, [5.0, 5.0]).is_none(),
        "outside clip rect misses"
    );
}

#[test]
fn r2_r3_combined_shape_inside_path_clipped_layer() {
    // A triangle shape inside a triangle-clipped layer — both
    // path-precise checks must agree. Hit at the centroid of the
    // shape (which is also inside the clip triangle); miss at a
    // point inside both AABBs but outside the inner triangle.
    let mut scene = Scene::new(400, 400);
    scene.push_layer_clip(SceneClip::Path(triangle_path()));
    scene.push_shape_filled(triangle_path(), [0.0, 1.0, 0.0, 1.0]);
    scene.ops.push(netrender::scene::SceneOp::PopLayer);

    let centroid_hits = hit_test(&scene, [50.0, 33.0]);
    assert!(!centroid_hits.is_empty(), "centroid hits");
    let corner_hits = hit_test(&scene, [95.0, 95.0]);
    assert!(
        corner_hits.is_empty(),
        "AABB corner misses both shape and clip"
    );
}
