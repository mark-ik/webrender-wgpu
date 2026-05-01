/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Compositor trait shapes — the seam axiom 14 of the design plan
//! reserves at Phase 0.5.
//!
//! These are *empty* traits today. Bodies are fleshed out at Phase 12,
//! when a consumer (servo-wgpu / graphshell / etc.) wires up a
//! platform compositor adapter. The reason they exist now, with no
//! methods, is that Phases 5–7 (image cache, render-task graph,
//! picture cache) need a place to defer texture-creation-time
//! external-memory flag decisions. Without the seam in place at
//! Phase 0.5, those phases would either hardcode wgpu defaults
//! (closing the door on platform-handle handoff) or invent ad-hoc
//! plumbing that would need to be ripped out at Phase 12.
//!
//! The third workspace crate — `netrender_compositor` — lands when a
//! consumer needs platform compositor adapters. Until then, the
//! traits live here.

/// Software-compositor handoff. The embedder gives the renderer a
/// `wgpu::TextureView` per tile; the renderer renders into it.
///
/// Phase 12 fleshes this out with the per-tile contract: visibility,
/// lifecycle, dirty-rect signalling. Today: empty seam (axiom 14).
pub trait Compositor {}

/// Native-compositor handoff. The embedder gives the renderer a
/// platform handle (CALayer / IOSurface / DXGI shared handle); the
/// renderer hands rendered tile metadata back; the embedder samples /
/// presents.
///
/// Phase 12 fleshes this out per platform (macOS first when
/// servo-wgpu needs it, then Windows DirectComposition). Tile-cache
/// textures (Phase 7), transient-pool textures (Phase 6), and any
/// texture that may be presented through `NativeCompositor` must be
/// allocated through code paths that *can* request the relevant wgpu
/// external-memory features. Today: empty seam (axiom 14).
pub trait NativeCompositor {}
