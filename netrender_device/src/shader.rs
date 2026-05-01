/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! WGSL module loading + cache (`include_str!`-based); WGSL `override`
//! specialization is handled at pipeline-factory time.

/// Solid-colour brush shader (Phase 1 smoke test, GL-era ABI).
pub(crate) const BRUSH_SOLID_WGSL: &str = include_str!("shaders/brush_solid.wgsl");

/// Phase 2 solid-rect batch shader. Fresh layout: per-instance storage
/// buffer indexed by `@builtin(instance_index)`, color inlined per
/// instance, ortho-projection-only per-frame uniform.
pub(crate) const BRUSH_RECT_SOLID_WGSL: &str = include_str!("shaders/brush_rect_solid.wgsl");

/// Phase 5 textured-rect batch shader. Instance data in storage buffer;
/// texture + sampler bound at slots 3–4. Nearest-clamp sampler only
/// (filterable: false). sRGB handling deferred to Phase 7.
pub(crate) const BRUSH_IMAGE_WGSL: &str = include_str!("shaders/brush_image.wgsl");

/// Phase 6 separable Gaussian blur. Fullscreen-quad VS (no vertex buffer);
/// 5-tap kernel along `params.step`. Call H then V for a full 2-D blur.
pub(crate) const BRUSH_BLUR_WGSL: &str = include_str!("shaders/brush_blur.wgsl");

/// Phase 8A 2-stop analytic linear gradient. Same bind-group shape as
/// `brush_rect_solid`; instance struct adds two endpoints and a second
/// color for a 96-byte stride.
pub(crate) const BRUSH_LINEAR_GRADIENT_WGSL: &str =
    include_str!("shaders/brush_linear_gradient.wgsl");

/// Phase 8B 2-stop analytic radial gradient. Identical bind-group shape
/// and 96-byte instance stride as `brush_linear_gradient`; the
/// 16-byte `params` slot encodes (center.xy, radii.xy) instead of
/// linear's (start.xy, end.xy), and the fragment shader computes
/// `t = length((pixel - center) / radii)` per fragment.
pub(crate) const BRUSH_RADIAL_GRADIENT_WGSL: &str =
    include_str!("shaders/brush_radial_gradient.wgsl");

/// Phase 8C 2-stop analytic conic gradient. Same shape as the linear
/// and radial families; `params` encodes (center.xy, start_angle, _pad)
/// and the fragment shader computes `t = fract((atan2(dy, dx) -
/// start_angle) / 2π)`.
pub(crate) const BRUSH_CONIC_GRADIENT_WGSL: &str =
    include_str!("shaders/brush_conic_gradient.wgsl");
