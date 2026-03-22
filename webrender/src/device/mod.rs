/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

use std::ops::Add;
use api::{ImageFormat, ImageBufferKind};
use api::units::FramebufferIntRect;

// ── Shared types ─────────────────────────────────────────────────────────────
// These types are used by both backends and live here so that the GpuDevice
// trait (below) can reference them without depending on gleam.

/// Sequence number for rendered frames, as tracked by the device layer.
#[derive(Debug, Copy, Clone, PartialEq, Ord, Eq, PartialOrd)]
#[cfg_attr(feature = "capture", derive(Serialize))]
#[cfg_attr(feature = "replay", derive(Deserialize))]
pub struct GpuFrameId(usize);

impl GpuFrameId {
    pub fn new(value: usize) -> Self {
        GpuFrameId(value)
    }
}

impl Add<usize> for GpuFrameId {
    type Output = GpuFrameId;
    fn add(self, other: usize) -> GpuFrameId {
        GpuFrameId(self.0 + other)
    }
}

/// Texture sampling filter.
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "capture", derive(Serialize))]
#[cfg_attr(feature = "replay", derive(Deserialize))]
pub enum TextureFilter {
    Nearest,
    Linear,
    Trilinear,
}

/// Pair of internal (GPU-native) and external (user-provided) texture formats.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "capture", derive(Serialize))]
#[cfg_attr(feature = "replay", derive(Deserialize))]
pub struct TextureFormatPair<T> {
    /// Format the GPU natively stores texels in.
    pub internal: T,
    /// Format we expect the users to provide the texels in.
    pub external: T,
}

impl<T: Copy> From<T> for TextureFormatPair<T> {
    fn from(value: T) -> Self {
        TextureFormatPair {
            internal: value,
            external: value,
        }
    }
}

/// Metadata used when allocating a texture as a render target.
#[derive(Copy, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "capture", derive(Serialize))]
#[cfg_attr(feature = "replay", derive(Deserialize))]
pub struct RenderTargetInfo {
    pub has_depth: bool,
}

// ── GpuDevice trait ───────────────────────────────────────────────────────────
// The minimal surface that the renderer needs from a GPU backend.  Each
// backend defines its own opaque `Texture` and `Program` types via associated
// types, keeping the trait backend-agnostic.
//
// This is a *declaration-only* sketch for Phase 1.  The GL backend
// impl lives in `gl.rs`; the wgpu impl will live in `wgpu_device.rs`.
// Call-sites in `renderer/mod.rs` will be migrated to use the trait in
// Phase 2 once both impls are proven correct.

pub trait GpuDevice {
    /// The backend's opaque texture handle.
    type Texture;
    /// The backend's opaque shader-program handle.
    type Program;

    // --- Frame lifecycle ---

    fn begin_frame(&mut self) -> GpuFrameId;
    fn end_frame(&mut self);

    // --- Texture management ---

    fn create_texture(
        &mut self,
        target: ImageBufferKind,
        format: ImageFormat,
        width: i32,
        height: i32,
        filter: TextureFilter,
        render_target: Option<RenderTargetInfo>,
    ) -> Self::Texture;

    fn upload_texture_immediate(&mut self, texture: &Self::Texture, pixels: &[u8]);

    fn delete_texture(&mut self, texture: Self::Texture);

    // --- Draw calls ---

    fn draw_triangles_u16(&mut self, first_vertex: i32, index_count: i32);
    fn draw_triangles_u32(&mut self, first_vertex: i32, index_count: i32);

    // --- Readback (used by snapshot tests and capture) ---

    fn read_pixels_into(
        &mut self,
        rect: FramebufferIntRect,
        format: ImageFormat,
        output: &mut [u8],
    );
}

// ── Backend modules ───────────────────────────────────────────────────────────

#[cfg(feature = "gl_backend")]
mod gl;
#[cfg(feature = "gl_backend")]
pub mod query_gl;

#[cfg(feature = "gl_backend")]
pub use self::gl::*;
#[cfg(feature = "gl_backend")]
pub use self::query_gl as query;

#[cfg(feature = "wgpu_backend")]
mod wgpu_device;
#[cfg(feature = "wgpu_backend")]
pub use self::wgpu_device::WgpuDevice;
