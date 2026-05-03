/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! `Renderer` shell — vello-backed.
//!
//! Public entry point: [`Renderer::render_vello`]. The renderer
//! owns a [`crate::vello_tile_rasterizer::VelloTileRasterizer`]
//! (constructed at init when `NetrenderOptions::enable_vello` is
//! true) and a [`TileCache`] (constructed when
//! `NetrenderOptions::tile_cache_size` is `Some(_)`). Both must be
//! present for `render_vello` to succeed.
//!
//! The Renderer used to host a parallel batched-WGSL rasterizer
//! (`prepare()` / `render()` returning `PreparedFrame`); that path
//! was retired in favor of a single vello pipeline. The brush
//! pipeline factories on `WgpuDevice` (brush_blur, clip_rectangle)
//! are still used by render-graph tasks, but the rasterizer-side
//! brush_solid / brush_rect_solid / brush_image / brush_gradient
//! factories are now unreachable from netrender; they're slated for
//! removal from `netrender_device` in a follow-up.
//!
//! Phase mapping after the cleanup:
//!
//! - **Phase 6** (render-graph for filters / blur / clip-mask
//!   tasks) lives on, intersecting with the rasterizer via
//!   [`Renderer::insert_image_vello`] — render-graph outputs
//!   become image sources for vello scenes.
//! - **Phase 7** picture caching is now the [`TileCache`]
//!   algorithm + the per-tile `vello::Scene` cache inside
//!   `VelloTileRasterizer`.
//! - **Phase 8** gradients (linear / radial / conic / N-stop) are
//!   `peniko::Gradient` mapped from `SceneGradient`; see
//!   `vello_rasterizer::scene_to_vello`.
//! - **Phase 9** clips are vello `push_layer` shapes (axis-aligned
//!   today; arbitrary path on Phase 9' completion).

pub(crate) mod init;

use std::sync::{Arc, Mutex};

use netrender_device::WgpuDevice;

use crate::scene::{ImageKey, Scene};
use crate::tile_cache::TileCache;

pub struct Renderer {
    pub wgpu_device: WgpuDevice,
    /// Phase 7: tile-cache invalidation algorithm. Configured via
    /// `NetrenderOptions::tile_cache_size`. Required for
    /// `render_vello` (the vello rasterizer holds its per-tile
    /// `vello::Scene` cache against this tile cache's coords).
    pub(crate) tile_cache: Option<Mutex<TileCache>>,
    /// Phase 7' — vello-backed tile rasterizer. Constructed at init
    /// when `NetrenderOptions::enable_vello` is true.
    pub(crate) vello_rasterizer: Option<Mutex<crate::vello_tile_rasterizer::VelloTileRasterizer>>,
}

/// Per-frame load policy on the color attachment for `render_vello`.
/// `Clear(c)` maps to vello's `RenderParams::base_color`. `Load` is
/// not supported on the vello path (vello always overwrites the
/// entire target); it's accepted for API compatibility and treated
/// as `Clear(transparent)`.
pub enum ColorLoad {
    Clear(wgpu::Color),
    Load,
}

impl Default for ColorLoad {
    fn default() -> Self {
        Self::Clear(wgpu::Color::TRANSPARENT)
    }
}

impl Renderer {
    /// Borrow the tile cache mutex (used by tests for invalidation
    /// inspection). Returns `None` if `tile_cache_size` was `None`.
    pub fn tile_cache(&self) -> Option<&Mutex<TileCache>> {
        self.tile_cache.as_ref()
    }

    /// Register a GPU-resident wgpu texture as an image source for
    /// subsequent `render_vello` calls under the given `ImageKey`.
    /// Render-graph outputs (blur results, mask coverage textures,
    /// etc.) become addressable from within a vello scene's
    /// `SceneImage` primitives via this entry point.
    ///
    /// The texture is cloned (cheap — `wgpu::Texture` is internally
    /// Arc-shared) and handed to `vello::Renderer::register_texture`
    /// (Path B from rasterizer plan §3.5). Entries persist across
    /// `render_vello` calls until `unregister_image_vello` is
    /// called or the renderer is dropped. Overrides win over
    /// `scene.image_sources` entries with the same `ImageKey`.
    ///
    /// # Panics
    ///
    /// If `enable_vello` was false at construction.
    pub fn insert_image_vello(&self, key: ImageKey, texture: Arc<wgpu::Texture>) {
        let rast_mutex = self
            .vello_rasterizer
            .as_ref()
            .expect("Renderer::insert_image_vello requires enable_vello = true");
        let mut rast = rast_mutex.lock().expect("vello_rasterizer lock");
        rast.register_texture(key, (*texture).clone());
    }

    /// Drop a previously-registered `insert_image_vello` entry.
    /// No-op if `key` was never registered or `enable_vello` is
    /// false.
    pub fn unregister_image_vello(&self, key: ImageKey) {
        let Some(rast_mutex) = self.vello_rasterizer.as_ref() else { return };
        let mut rast = rast_mutex.lock().expect("vello_rasterizer lock");
        rast.unregister_texture(key);
    }

    /// Number of tiles whose `vello::Scene`s were rebuilt during
    /// the most recent `render_vello` call. `0` after a no-op
    /// frame (unchanged scene). Returns `None` if `enable_vello`
    /// was false.
    pub fn vello_last_dirty_count(&self) -> Option<usize> {
        let rast_mutex = self.vello_rasterizer.as_ref()?;
        let rast = rast_mutex.lock().expect("vello_rasterizer lock");
        Some(rast.last_dirty_count())
    }

    /// Number of tile-Scenes currently held in the vello
    /// rasterizer's cache. Returns `None` if `enable_vello` was
    /// false.
    pub fn vello_cached_tile_count(&self) -> Option<usize> {
        let rast_mutex = self.vello_rasterizer.as_ref()?;
        let rast = rast_mutex.lock().expect("vello_rasterizer lock");
        Some(rast.cached_tile_count())
    }

    /// Render `scene` into `target_view` via the vello-backed tile
    /// rasterizer.
    ///
    /// Steps (all internal):
    /// 1. `tile_cache.invalidate(scene)` → list of dirty tile coords.
    /// 2. For each dirty tile, build a filtered `vello::Scene`
    ///    containing only the primitives whose AABB intersects the
    ///    tile's world rect.
    /// 3. Compose all cached tile-Scenes into a master Scene with
    ///    per-tile clip layers.
    /// 4. One `vello::Renderer::render_to_texture` call.
    ///
    /// `clear` controls the base color. `Clear(c)` is the typical
    /// case; `Load` is not supported by vello's compute pipeline
    /// (which always overwrites the entire target) and is treated
    /// as `Clear(transparent)` for API compatibility.
    ///
    /// # Panics
    ///
    /// - If `enable_vello` was false at construction.
    /// - If `tile_cache_size` was `None` at construction.
    /// - If a vello render error occurs (mirrors the existing
    ///   `render()` shape, which doesn't return a Result).
    pub fn render_vello(
        &self,
        scene: &Scene,
        target_view: &wgpu::TextureView,
        clear: ColorLoad,
    ) {
        let rast_mutex = self
            .vello_rasterizer
            .as_ref()
            .expect("Renderer::render_vello requires NetrenderOptions::enable_vello = true");
        let tc_mutex = self
            .tile_cache
            .as_ref()
            .expect("Renderer::render_vello requires NetrenderOptions::tile_cache_size = Some(_)");

        let base = match clear {
            ColorLoad::Clear(c) => vello::peniko::Color::new([
                c.r as f32, c.g as f32, c.b as f32, c.a as f32,
            ]),
            ColorLoad::Load => vello::peniko::Color::new([0.0, 0.0, 0.0, 0.0]),
        };

        let mut rast = rast_mutex.lock().expect("vello_rasterizer lock");
        let mut tc = tc_mutex.lock().expect("tile_cache lock");
        rast.render(scene, &mut tc, target_view, base)
            .unwrap_or_else(|e| panic!("vello render_to_texture failed: {:?}", e));
    }
}

#[derive(Debug)]
pub enum RendererError {
    WgpuFeaturesMissing(wgpu::Features),
    /// `NetrenderOptions::enable_vello = true` requires
    /// `tile_cache_size = Some(_)`. The vello rasterizer holds the
    /// per-tile `vello::Scene` cache against the tile cache's
    /// coords; without a tile cache there's nothing for it to cache
    /// against.
    VelloRequiresTileCache,
    /// `vello::Renderer` construction failed during
    /// `create_netrender_instance`. The wrapped string is vello's
    /// error formatted via `{:?}` (vello::Error doesn't implement
    /// `std::error::Error` in 0.8 — the string is informational
    /// only).
    VelloInit(String),
}
