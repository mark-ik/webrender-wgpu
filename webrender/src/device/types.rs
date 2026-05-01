/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Backend-neutral device types.
//!
//! Types that don't depend on a specific graphics API and need to be
//! visible regardless of which backend feature(s) are enabled. Compiled
//! unconditionally — `traits.rs` and any cross-backend renderer code can
//! import these without cfg-gates.
//!
//! P1 begins by lifting the simplest pure types here. More follow as the
//! wgpu impl wires up. See assignment-doc R2 for the full lift/associated-type
//! categorization.

use std::ops::Add;

/// Sequence number for frames, as tracked by the device layer.
#[derive(Debug, Copy, Clone, PartialEq, Ord, Eq, PartialOrd)]
#[cfg_attr(feature = "capture", derive(Serialize))]
#[cfg_attr(feature = "replay", derive(Deserialize))]
pub struct GpuFrameId(pub usize);

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

/// Sampler unit index used by `bind_texture` etc.
pub struct TextureSlot(pub usize);

/// Texture filtering mode for sampling.
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "capture", derive(Serialize))]
#[cfg_attr(feature = "replay", derive(Deserialize))]
pub enum TextureFilter {
    Nearest,
    Linear,
    Trilinear,
}

/// Hint to the GPU about how a vertex buffer's contents will be used.
/// Backends translate this into their own usage flags
/// (GL: `STATIC_DRAW`/`DYNAMIC_DRAW`/`STREAM_DRAW`; wgpu: `BufferUsages` flags).
#[derive(Copy, Clone, Debug)]
pub enum VertexUsageHint {
    Static,
    Dynamic,
    Stream,
}
