/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phases 2' / 5' — netrender Scene → vello::Scene translator.
//!
//! Phase 2': rects with per-primitive transform and axis-aligned
//! clip. Phase 5': image ingestion with per-image transform, clip,
//! UV sub-region, and alpha tint. Gradients land in 8'. Output is
//! suitable for `Renderer::render_to_texture`; receipts are at
//! `tests/p2prime_vello_rects.rs` and `tests/p5prime_vello_image.rs`.
//!
//! ## Image-tint scope (Phase 5a)
//!
//! `SceneImage.color` is a premultiplied RGBA tint. Phase 5a handles
//! achromatic tints (`r == g == b == a` per the §3.2 plan: "(a, a,
//! a, a) is an alpha multiplier") via `ImageBrush::with_alpha(a)`.
//! Chromatic tints (the §3.2 plan's "Mix::Multiply layer" path —
//! used by 9A's mask-as-tinted-image case) require an extra layer
//! and land in a later sub-phase. Non-achromatic input panics with a
//! TODO so callers don't get silently wrong colors.
//!
//! ## Boundary conventions (verified Phase 1' p1prime_02 / p1prime_03)
//!
//! - `SceneRect.color` is **premultiplied** RGBA. `peniko::Color`
//!   expects **straight-alpha**. We unpremultiply at the boundary:
//!   `(r/a, g/a, b/a, a)` for `a > 0`, `(0, 0, 0, 0)` for `a == 0`.
//! - Vello stores straight-alpha sRGB-encoded values in its output
//!   target. The compositor (downstream sample stage) is responsible
//!   for premultiplying after the hardware sRGB→linear decode; that
//!   contract is unchanged from §6.1.
//! - `interpolation_cs` is not threaded through gradients (no-op on
//!   the GPU compute path; see §3.3 / p1prime_03).
//!
//! ## Coordinate conventions
//!
//! `Transform.m` is a column-major 4×4 with the 2D affine in
//! `(m[0], m[1], m[4], m[5], m[12], m[13])` = `(a, b, c, d, e, f)`,
//! matching `kurbo::Affine::new([a, b, c, d, e, f])`.

use std::collections::HashMap;
use std::sync::Arc;

use vello::kurbo::{Affine, Rect};
use vello::peniko::{
    self, Blob, Color, Fill, ImageAlphaType, ImageBrush, ImageData, ImageFormat,
};

use crate::scene::{ImageKey, NO_CLIP, Scene, SceneImage, SceneRect, Transform};

/// Translate a netrender [`Scene`] into a [`vello::Scene`] suitable
/// for [`vello::Renderer::render_to_texture`].
///
/// Phase 2' / 5' scope: rects + images, with per-primitive transform
/// and clip. Gradients in `scene` are silently ignored (Phase 8').
/// Painter order matches the parent scene: rects first, then images
/// painted over them — the same ordering the existing netrender
/// pipeline uses.
pub fn scene_to_vello(scene: &Scene) -> vello::Scene {
    let mut vscene = vello::Scene::new();

    let images = build_image_cache(scene);

    for rect in &scene.rects {
        emit_rect(&mut vscene, rect, &scene.transforms);
    }
    for image in &scene.images {
        emit_image(&mut vscene, image, &scene.transforms, &images);
    }

    vscene
}

fn build_image_cache(scene: &Scene) -> HashMap<ImageKey, ImageData> {
    let mut cache = HashMap::with_capacity(scene.image_sources.len());
    for (key, data) in &scene.image_sources {
        let blob = Blob::new(Arc::new(data.bytes.clone()));
        cache.insert(
            *key,
            ImageData {
                data: blob,
                format: ImageFormat::Rgba8,
                alpha_type: ImageAlphaType::Alpha,
                width: data.width,
                height: data.height,
            },
        );
    }
    cache
}

fn emit_rect(vscene: &mut vello::Scene, rect: &SceneRect, transforms: &[Transform]) {
    let affine = transform_to_affine(&transforms[rect.transform_id as usize]);
    let shape = Rect::new(
        rect.x0 as f64,
        rect.y0 as f64,
        rect.x1 as f64,
        rect.y1 as f64,
    );
    let color = unpremultiply_color(rect.color);

    let needs_clip = rect.clip_rect != NO_CLIP;
    if needs_clip {
        let clip = Rect::new(
            rect.clip_rect[0] as f64,
            rect.clip_rect[1] as f64,
            rect.clip_rect[2] as f64,
            rect.clip_rect[3] as f64,
        );
        vscene.push_layer(
            Fill::NonZero,
            peniko::Mix::Normal,
            1.0,
            Affine::IDENTITY,
            &clip,
        );
    }
    vscene.fill(Fill::NonZero, affine, color, None, &shape);
    if needs_clip {
        vscene.pop_layer();
    }
}

fn emit_image(
    vscene: &mut vello::Scene,
    image: &SceneImage,
    transforms: &[Transform],
    cache: &HashMap<ImageKey, ImageData>,
) {
    let img = cache
        .get(&image.key)
        .expect("scene_to_vello: SceneImage references unknown ImageKey");

    let alpha = achromatic_tint_alpha(image.color);
    let brush = ImageBrush::new(img.clone()).with_alpha(alpha);

    let target = Rect::new(
        image.x0 as f64,
        image.y0 as f64,
        image.x1 as f64,
        image.y1 as f64,
    );
    let world = transform_to_affine(&transforms[image.transform_id as usize]);
    let brush_xform = uv_to_target_affine(image.uv, target, img.width, img.height);

    let needs_clip = image.clip_rect != NO_CLIP;
    if needs_clip {
        let clip = Rect::new(
            image.clip_rect[0] as f64,
            image.clip_rect[1] as f64,
            image.clip_rect[2] as f64,
            image.clip_rect[3] as f64,
        );
        vscene.push_layer(
            Fill::NonZero,
            peniko::Mix::Normal,
            1.0,
            Affine::IDENTITY,
            &clip,
        );
    }
    vscene.fill(Fill::NonZero, world, &brush, Some(brush_xform), &target);
    if needs_clip {
        vscene.pop_layer();
    }
}

/// Map UV `[u0, v0, u1, v1]` (normalized to `[0, 1]`) of a `(W, H)`
/// image onto a target `Rect`. The returned affine is the brush
/// transform passed to `vello::Scene::fill`: it maps brush-local
/// coordinates (= image pixel coordinates) onto target-rect
/// coordinates so that the UV sub-region lands on the rect's bounds.
fn uv_to_target_affine(uv: [f32; 4], target: Rect, image_w: u32, image_h: u32) -> Affine {
    let (u0, v0, u1, v1) = (uv[0] as f64, uv[1] as f64, uv[2] as f64, uv[3] as f64);
    let w = image_w as f64;
    let h = image_h as f64;
    // Source pixel range covered by the UV slice.
    let src_x0 = u0 * w;
    let src_y0 = v0 * h;
    let src_w = (u1 - u0) * w;
    let src_h = (v1 - v0) * h;
    let tgt_w = target.width();
    let tgt_h = target.height();
    let sx = if src_w.abs() > 0.0 { tgt_w / src_w } else { 1.0 };
    let sy = if src_h.abs() > 0.0 { tgt_h / src_h } else { 1.0 };
    // brush_xform * src_pixel = target_pixel, i.e. translate then scale.
    Affine::translate((target.x0 - src_x0 * sx, target.y0 - src_y0 * sy))
        * Affine::scale_non_uniform(sx, sy)
}

/// Extract the achromatic alpha multiplier from a premultiplied tint.
/// Panics if the tint has chromatic content (R != G or G != B); those
/// require a Mix::Multiply layer not yet implemented (§3.2 footnote).
fn achromatic_tint_alpha(color: [f32; 4]) -> f32 {
    let [r, g, b, a] = color;
    let chromatic = (r - g).abs() > 1e-3 || (g - b).abs() > 1e-3 || (r - a).abs() > 1e-3;
    assert!(
        !chromatic,
        "vello_rasterizer: chromatic image tints not yet supported (color = {:?}). \
         §3.2 calls for a Mix::Multiply layer; land that before using non-achromatic tints.",
        color
    );
    a.clamp(0.0, 1.0)
}

fn transform_to_affine(t: &Transform) -> Affine {
    Affine::new([
        t.m[0] as f64,
        t.m[1] as f64,
        t.m[4] as f64,
        t.m[5] as f64,
        t.m[12] as f64,
        t.m[13] as f64,
    ])
}

fn unpremultiply_color(c: [f32; 4]) -> Color {
    let a = c[3];
    if a > 0.0 {
        Color::from_rgba8(
            (c[0] / a * 255.0).round().clamp(0.0, 255.0) as u8,
            (c[1] / a * 255.0).round().clamp(0.0, 255.0) as u8,
            (c[2] / a * 255.0).round().clamp(0.0, 255.0) as u8,
            (a * 255.0).round().clamp(0.0, 255.0) as u8,
        )
    } else {
        Color::from_rgba8(0, 0, 0, 0)
    }
}
