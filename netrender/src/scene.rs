/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 3 scene representation — adds per-primitive transforms and
//! axis-aligned clip rectangles to the Phase 2 solid-rect baseline.
//! Phase 5 adds image primitives (textured rects).
//!
//! Design plan §5 Phase 3: "Lift `space.rs`, `spatial_tree.rs`,
//! `transform.rs` math from old webrender." Phase 3 uses 4×4
//! column-major matrices for generality; the 2D affine subset is the
//! initial surface (translate / rotate / scale helpers). Full spatial
//! tree hierarchy (parent → child reference chains) is deferred to the
//! later phase that ingests `BuiltDisplayList` spatial nodes.
//!
//! Backward compat: `Scene::push_rect` still works unchanged. The
//! transforms array always has index 0 = identity, so existing callers
//! that do not pass a `transform_id` render exactly as in Phase 2.

use std::collections::HashMap;

/// A 4×4 column-major transform matrix.
///
/// Column `i` occupies `m[i*4..i*4+4]`. Identity: columns are
/// `(1,0,0,0)`, `(0,1,0,0)`, `(0,0,1,0)`, `(0,0,0,1)`.
///
/// In WGSL this maps directly to `mat4x4<f32>` in a storage buffer
/// (same column-major layout, 64 bytes per element, align 16).
#[derive(Debug, Clone, Copy)]
pub struct Transform {
    /// Column-major: `m[col*4 + row]`.
    pub m: [f32; 16],
}

impl Transform {
    pub const IDENTITY: Self = Self {
        m: [
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ],
    };

    /// 2D translation — moves by `(tx, ty)` in the XY plane.
    pub fn translate_2d(tx: f32, ty: f32) -> Self {
        Self {
            m: [
                1.0, 0.0, 0.0, 0.0,
                0.0, 1.0, 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                tx,  ty,  0.0, 1.0,
            ],
        }
    }

    /// 2D counter-clockwise rotation by `angle_radians` around the origin.
    pub fn rotate_2d(angle_radians: f32) -> Self {
        let (s, c) = angle_radians.sin_cos();
        Self {
            m: [
                 c,   s,  0.0, 0.0,
                -s,   c,  0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                0.0, 0.0, 0.0, 1.0,
            ],
        }
    }

    /// 2D uniform scale by `(sx, sy)` around the origin.
    pub fn scale_2d(sx: f32, sy: f32) -> Self {
        Self {
            m: [
                sx,  0.0, 0.0, 0.0,
                0.0, sy,  0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                0.0, 0.0, 0.0, 1.0,
            ],
        }
    }

    /// Returns the transform that applies `self` first, then `other`.
    /// Equivalent to the matrix product `other × self`.
    ///
    /// Example: `scale.then(rotate).then(translate)` applies scale,
    /// then rotation around origin, then translation.
    pub fn then(&self, other: &Transform) -> Transform {
        // C = other × self.
        // C[col*4+row] = Σ_k  other.m[k*4+row] × self.m[col*4+k]
        let a = &other.m;
        let b = &self.m;
        let mut c = [0.0f32; 16];
        for col in 0..4usize {
            for row in 0..4usize {
                let mut s = 0.0f32;
                for k in 0..4usize {
                    s += a[k * 4 + row] * b[col * 4 + k];
                }
                c[col * 4 + row] = s;
            }
        }
        Transform { m: c }
    }
}

/// One solid-colored rectangle with a per-primitive transform and an
/// optional axis-aligned device-space clip rectangle.
///
/// `x0/y0/x1/y1` are in **local space** — the transform at
/// `transform_id` maps them to device-pixel space. When
/// `transform_id == 0` (identity) the coordinates are device-pixel
/// coordinates directly (backward-compatible with Phase 2).
#[derive(Debug, Clone)]
pub struct SceneRect {
    /// Local-space left / top / right / bottom.
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    /// Premultiplied RGBA.
    pub color: [f32; 4],
    /// Index into `Scene::transforms`. `0` is always the identity.
    pub transform_id: u32,
    /// Axis-aligned clip rectangle in device pixels `[x0, y0, x1, y1]`.
    /// `[NEG_INFINITY, NEG_INFINITY, INFINITY, INFINITY]` disables clipping.
    pub clip_rect: [f32; 4],
}

pub const NO_CLIP: [f32; 4] =
    [f32::NEG_INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::INFINITY];

/// Opaque identifier for a cached GPU texture. Caller-assigned; any
/// unique `u64` works (hash of path, monotonic counter, etc.).
pub type ImageKey = u64;

/// CPU-side pixel data for one image. Format: RGBA8Unorm, row-major,
/// tightly packed (`width * 4` bytes per row). sRGB handling is deferred
/// to Phase 7; for now the bytes are treated as linear values.
#[derive(Debug, Clone)]
pub struct ImageData {
    pub width: u32,
    pub height: u32,
    /// Raw RGBA8 bytes; `len()` must equal `width * height * 4`.
    pub bytes: Vec<u8>,
}

/// One 2-stop linear gradient rectangle (Phase 8A).
///
/// Gradient direction and length are defined by `start_point` /
/// `end_point` in **local space** (same coordinate system as `x0..y1`).
/// Color at any pixel is `mix(color0, color1, t)` where
/// `t = clamp(dot(pixel - start, dir) / |dir|^2, 0, 1)`.
///
/// Both colors are **premultiplied**. A gradient is opaque iff both
/// stops have `alpha >= 1.0`; otherwise it goes through the
/// alpha-blend pipeline.
#[derive(Debug, Clone)]
pub struct SceneLinearGradient {
    /// Local-space rect bounds.
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    /// Gradient line start (local space). Color0 is the value here.
    pub start_point: [f32; 2],
    /// Gradient line end (local space). Color1 is the value here.
    pub end_point: [f32; 2],
    /// Premultiplied RGBA at the start of the gradient line.
    pub color0: [f32; 4],
    /// Premultiplied RGBA at the end of the gradient line.
    pub color1: [f32; 4],
    /// Index into `Scene::transforms`; `0` = identity.
    pub transform_id: u32,
    /// Device-space axis-aligned clip; `NO_CLIP` disables clipping.
    pub clip_rect: [f32; 4],
}

/// One 2-stop radial gradient rectangle (Phase 8B).
///
/// Radial parameters: `center` (local space) is where `color0` lives;
/// `radii = (rx, ry)` define an ellipse — set `rx == ry` for a circular
/// gradient. Color at any pixel is `mix(color0, color1, t)` where
/// `t = clamp(length((pixel - center) / radii), 0, 1)`. So `color1`
/// is the value at the elliptical boundary `t = 1` and remains the
/// value for any pixel outside it.
///
/// Both colors are **premultiplied**. Opaque iff both stops have
/// `alpha >= 1.0`.
#[derive(Debug, Clone)]
pub struct SceneRadialGradient {
    /// Local-space rect bounds.
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    /// Center of the radial gradient (local space). Color0 lives here.
    pub center: [f32; 2],
    /// Radii of the gradient ellipse (local space). `[r, r]` for circular.
    pub radii: [f32; 2],
    /// Premultiplied RGBA at the center.
    pub color0: [f32; 4],
    /// Premultiplied RGBA at and beyond the elliptical boundary.
    pub color1: [f32; 4],
    /// Index into `Scene::transforms`; `0` = identity.
    pub transform_id: u32,
    /// Device-space axis-aligned clip; `NO_CLIP` disables clipping.
    pub clip_rect: [f32; 4],
}

/// One 2-stop conic gradient rectangle (Phase 8C).
///
/// `t` sweeps around `center`. With y+ pointing downward (screen
/// convention), `atan2(dy, dx)` increases clockwise: 0 = east,
/// pi/2 = south, pi = west, -pi/2 = north. The gradient seam (where
/// `t` wraps from 1 back to 0) sits at `start_angle` radians; setting
/// `start_angle = -π/2` matches CSS `conic-gradient(from 0deg)`'s
/// 12-o'clock start.
///
/// 2-stop semantics introduce a hard discontinuity at the seam where
/// `color1` jumps back to `color0`. Setting `color0 == color1`
/// produces a uniform fill.
#[derive(Debug, Clone)]
pub struct SceneConicGradient {
    /// Local-space rect bounds.
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    /// Center of the conic sweep (local space).
    pub center: [f32; 2],
    /// Seam angle in radians (counterclockwise math convention; with
    /// y-down screen coords the angle increases visually clockwise).
    pub start_angle: f32,
    /// Premultiplied RGBA at `t = 0` (just after the seam).
    pub color0: [f32; 4],
    /// Premultiplied RGBA at `t = 1` (just before the seam).
    pub color1: [f32; 4],
    /// Index into `Scene::transforms`; `0` = identity.
    pub transform_id: u32,
    /// Device-space axis-aligned clip; `NO_CLIP` disables clipping.
    pub clip_rect: [f32; 4],
}

/// One textured rectangle. UV corners map the image onto the rect;
/// the tint color is multiplied element-wise with the sampled value
/// (premultiplied; `[1,1,1,1]` = no tint).
#[derive(Debug, Clone)]
pub struct SceneImage {
    /// Local-space corners (same coordinate system as `SceneRect`).
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    /// UV corners `[u0, v0, u1, v1]` in normalised `[0, 1]` space.
    /// `[0, 0, 1, 1]` maps the full image to the rect.
    pub uv: [f32; 4],
    /// Premultiplied RGBA tint. `[1, 1, 1, 1]` is a no-op.
    pub color: [f32; 4],
    /// Cache key for the GPU texture (see `Scene::set_image_source`).
    pub key: ImageKey,
    /// Index into `Scene::transforms`; `0` = identity.
    pub transform_id: u32,
    /// Device-space axis-aligned clip; `NO_CLIP` disables clipping.
    pub clip_rect: [f32; 4],
}

/// A flat list of primitives to be rendered into one frame.
///
/// Phase 3 adds `transforms` (a palette of 4×4 matrices) and per-rect
/// `transform_id` / `clip_rect`. Phase 4 sorts for correct depth order.
/// Phase 5 adds `images` (textured rects) and `image_sources` (pixel data).
///
/// Draw order: rects are at painter indices 0..N_rects; images follow at
/// indices N_rects..N_total. Images therefore paint "in front of" all rects
/// in depth — correct for overlays.
#[derive(Debug, Clone)]
pub struct Scene {
    /// Viewport size in device pixels.
    pub viewport_width: u32,
    pub viewport_height: u32,
    /// Solid-color primitives in painter order (back-to-front).
    pub rects: Vec<SceneRect>,
    /// Textured-rect primitives in painter order (back-to-front).
    /// These paint on top of all rects.
    pub images: Vec<SceneImage>,
    /// 2-stop linear gradients in painter order (back-to-front).
    /// These paint on top of all rects and images (Phase 8A).
    pub linear_gradients: Vec<SceneLinearGradient>,
    /// 2-stop radial gradients in painter order (back-to-front).
    /// These paint on top of all rects, images, and linear gradients
    /// (Phase 8B). Within-frame interleaving of linear and radial
    /// (linear A → radial B → linear C) is not preserved by Phase 8;
    /// linear gradients always paint behind radial gradients.
    pub radial_gradients: Vec<SceneRadialGradient>,
    /// 2-stop conic gradients in painter order (back-to-front).
    /// These paint on top of every other family in Phase 8C
    /// (rects → images → linear → radial → conic). Same Phase 8
    /// limitation as above: family boundaries dominate user push
    /// interleaving until 8D's unified gradient list lands.
    pub conic_gradients: Vec<SceneConicGradient>,
    /// Transform palette. Index 0 is always identity.
    pub transforms: Vec<Transform>,
    /// CPU-side pixel data keyed by `ImageKey`. On first `prepare()`,
    /// each entry is uploaded to the GPU and cached there. Subsequent
    /// frames may omit data for already-cached keys.
    pub image_sources: HashMap<ImageKey, ImageData>,
}

impl Scene {
    pub fn new(viewport_width: u32, viewport_height: u32) -> Self {
        Self {
            viewport_width,
            viewport_height,
            rects: Vec::new(),
            images: Vec::new(),
            linear_gradients: Vec::new(),
            radial_gradients: Vec::new(),
            conic_gradients: Vec::new(),
            transforms: vec![Transform::IDENTITY], // index 0 = identity
            image_sources: HashMap::new(),
        }
    }

    /// Register a transform and return its index into the palette.
    pub fn push_transform(&mut self, t: Transform) -> u32 {
        let id = self.transforms.len() as u32;
        self.transforms.push(t);
        id
    }

    /// Append a rect at device-pixel coordinates with no transform and
    /// no clip (backward-compatible Phase 2 API).
    pub fn push_rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: [f32; 4]) {
        self.rects.push(SceneRect {
            x0, y0, x1, y1,
            color,
            transform_id: 0,
            clip_rect: NO_CLIP,
        });
    }

    /// Append a rect with an explicit transform id.
    pub fn push_rect_transformed(
        &mut self,
        x0: f32, y0: f32, x1: f32, y1: f32,
        color: [f32; 4],
        transform_id: u32,
    ) {
        self.rects.push(SceneRect {
            x0, y0, x1, y1,
            color,
            transform_id,
            clip_rect: NO_CLIP,
        });
    }

    /// Append a rect with an explicit transform and a device-space clip.
    pub fn push_rect_clipped(
        &mut self,
        x0: f32, y0: f32, x1: f32, y1: f32,
        color: [f32; 4],
        transform_id: u32,
        clip_rect: [f32; 4],
    ) {
        self.rects.push(SceneRect {
            x0, y0, x1, y1,
            color,
            transform_id,
            clip_rect,
        });
    }

    /// Register pixel data for `key` without adding a draw primitive.
    /// Call this before `push_image_ref` if you want to separate data
    /// registration from draw-list building.
    pub fn set_image_source(&mut self, key: ImageKey, data: ImageData) {
        self.image_sources.entry(key).or_insert(data);
    }

    /// Append an image rect at device-pixel coordinates.
    ///
    /// `data` is uploaded once on first `prepare()` and cached by `key`.
    /// Subsequent calls with the same `key` ignore `data`.
    /// UV defaults to `[0, 0, 1, 1]` (full image); tint to white `[1,1,1,1]`.
    pub fn push_image(
        &mut self,
        x0: f32, y0: f32, x1: f32, y1: f32,
        key: ImageKey,
        data: ImageData,
    ) {
        self.image_sources.entry(key).or_insert(data);
        self.images.push(SceneImage {
            x0, y0, x1, y1,
            uv: [0.0, 0.0, 1.0, 1.0],
            color: [1.0, 1.0, 1.0, 1.0],
            key,
            transform_id: 0,
            clip_rect: NO_CLIP,
        });
    }

    /// Append a 2-stop linear gradient at device-pixel coords. UV-style
    /// `start` / `end` are in the same local-space coordinate system as
    /// `x0..y1`. No transform, no clip.
    pub fn push_linear_gradient(
        &mut self,
        x0: f32, y0: f32, x1: f32, y1: f32,
        start: [f32; 2],
        end: [f32; 2],
        color0: [f32; 4],
        color1: [f32; 4],
    ) {
        self.linear_gradients.push(SceneLinearGradient {
            x0, y0, x1, y1,
            start_point: start,
            end_point: end,
            color0,
            color1,
            transform_id: 0,
            clip_rect: NO_CLIP,
        });
    }

    /// Append a 2-stop linear gradient with full control over transform
    /// and device-space clip.
    pub fn push_linear_gradient_full(
        &mut self,
        x0: f32, y0: f32, x1: f32, y1: f32,
        start: [f32; 2],
        end: [f32; 2],
        color0: [f32; 4],
        color1: [f32; 4],
        transform_id: u32,
        clip_rect: [f32; 4],
    ) {
        self.linear_gradients.push(SceneLinearGradient {
            x0, y0, x1, y1,
            start_point: start,
            end_point: end,
            color0,
            color1,
            transform_id,
            clip_rect,
        });
    }

    /// Append a 2-stop radial gradient (Phase 8B). `center` and
    /// `radii` are in local space; for a circular gradient, set
    /// `radii = [r, r]`. Color0 at the center, color1 at the
    /// elliptical boundary; pixels beyond the boundary are clamped to
    /// color1.
    pub fn push_radial_gradient(
        &mut self,
        x0: f32, y0: f32, x1: f32, y1: f32,
        center: [f32; 2],
        radii: [f32; 2],
        color0: [f32; 4],
        color1: [f32; 4],
    ) {
        self.radial_gradients.push(SceneRadialGradient {
            x0, y0, x1, y1,
            center,
            radii,
            color0,
            color1,
            transform_id: 0,
            clip_rect: NO_CLIP,
        });
    }

    /// Append a 2-stop conic gradient (Phase 8C). `t = 0` lives at
    /// `start_angle`, sweeping clockwise (in screen coords with y down)
    /// to `t = 1` just before wrapping back to `start_angle`.
    pub fn push_conic_gradient(
        &mut self,
        x0: f32, y0: f32, x1: f32, y1: f32,
        center: [f32; 2],
        start_angle: f32,
        color0: [f32; 4],
        color1: [f32; 4],
    ) {
        self.conic_gradients.push(SceneConicGradient {
            x0, y0, x1, y1,
            center,
            start_angle,
            color0,
            color1,
            transform_id: 0,
            clip_rect: NO_CLIP,
        });
    }

    /// Append a 2-stop conic gradient with full control over transform
    /// and device-space clip.
    pub fn push_conic_gradient_full(
        &mut self,
        x0: f32, y0: f32, x1: f32, y1: f32,
        center: [f32; 2],
        start_angle: f32,
        color0: [f32; 4],
        color1: [f32; 4],
        transform_id: u32,
        clip_rect: [f32; 4],
    ) {
        self.conic_gradients.push(SceneConicGradient {
            x0, y0, x1, y1,
            center,
            start_angle,
            color0,
            color1,
            transform_id,
            clip_rect,
        });
    }

    /// Append a 2-stop radial gradient with full control over transform
    /// and device-space clip.
    pub fn push_radial_gradient_full(
        &mut self,
        x0: f32, y0: f32, x1: f32, y1: f32,
        center: [f32; 2],
        radii: [f32; 2],
        color0: [f32; 4],
        color1: [f32; 4],
        transform_id: u32,
        clip_rect: [f32; 4],
    ) {
        self.radial_gradients.push(SceneRadialGradient {
            x0, y0, x1, y1,
            center,
            radii,
            color0,
            color1,
            transform_id,
            clip_rect,
        });
    }

    /// Append an image rect with full control over UV, tint, transform,
    /// and clip.
    pub fn push_image_full(
        &mut self,
        x0: f32, y0: f32, x1: f32, y1: f32,
        uv: [f32; 4],
        color: [f32; 4],
        key: ImageKey,
        transform_id: u32,
        clip_rect: [f32; 4],
    ) {
        self.images.push(SceneImage {
            x0, y0, x1, y1,
            uv,
            color,
            key,
            transform_id,
            clip_rect,
        });
    }
}

impl Default for Scene {
    fn default() -> Self {
        Self::new(0, 0)
    }
}
