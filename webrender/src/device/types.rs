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

use api::ImageFormat;
use std::num::NonZeroUsize;
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

/// Depth-test comparison function.
///
/// Backend-neutral named variants — backends translate to their own enum
/// (GL: `gl::ALWAYS`/`gl::LESS`/`gl::LEQUAL`; wgpu: `wgpu::CompareFunction`).
/// Variants match webrender's current usage; expand additively if a backend
/// impl needs more (e.g. `Equal`, `Greater`, `Never`).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum DepthFunction {
    Always,
    Less,
    LessEqual,
}

/// Vertex attribute scalar type.
#[derive(Debug, Copy, Clone)]
pub enum VertexAttributeKind {
    F32,
    U8Norm,
    U16Norm,
    I32,
    U16,
}

impl VertexAttributeKind {
    pub fn size_in_bytes(&self) -> u32 {
        match *self {
            VertexAttributeKind::F32 => 4,
            VertexAttributeKind::U8Norm => 1,
            VertexAttributeKind::U16Norm => 2,
            VertexAttributeKind::I32 => 4,
            VertexAttributeKind::U16 => 2,
        }
    }
}

/// Single vertex attribute slot in a vertex schema.
#[derive(Debug)]
pub struct VertexAttribute {
    pub name: &'static str,
    pub count: u32,
    pub kind: VertexAttributeKind,
}

impl VertexAttribute {
    pub const fn quad_instance_vertex() -> Self {
        VertexAttribute {
            name: "aPosition",
            count: 2,
            kind: VertexAttributeKind::U8Norm,
        }
    }

    pub const fn gpu_buffer_address(name: &'static str) -> Self {
        VertexAttribute {
            name,
            count: 1,
            kind: VertexAttributeKind::I32,
        }
    }

    pub const fn f32x4(name: &'static str) -> Self {
        VertexAttribute { name, count: 4, kind: VertexAttributeKind::F32 }
    }

    pub const fn f32x3(name: &'static str) -> Self {
        VertexAttribute { name, count: 3, kind: VertexAttributeKind::F32 }
    }

    pub const fn f32x2(name: &'static str) -> Self {
        VertexAttribute { name, count: 2, kind: VertexAttributeKind::F32 }
    }

    pub const fn f32(name: &'static str) -> Self {
        VertexAttribute { name, count: 1, kind: VertexAttributeKind::F32 }
    }

    pub const fn i32x4(name: &'static str) -> Self {
        VertexAttribute { name, count: 4, kind: VertexAttributeKind::I32 }
    }

    pub const fn i32x2(name: &'static str) -> Self {
        VertexAttribute { name, count: 2, kind: VertexAttributeKind::I32 }
    }

    pub const fn i32(name: &'static str) -> Self {
        VertexAttribute { name, count: 1, kind: VertexAttributeKind::I32 }
    }

    pub const fn u16(name: &'static str) -> Self {
        VertexAttribute { name, count: 1, kind: VertexAttributeKind::U16 }
    }

    pub const fn u16x2(name: &'static str) -> Self {
        VertexAttribute { name, count: 2, kind: VertexAttributeKind::U16 }
    }

    pub fn size_in_bytes(&self) -> u32 {
        self.count * self.kind.size_in_bytes()
    }
}

/// Vertex schema: per-vertex + per-instance attribute lists.
#[derive(Debug)]
pub struct VertexDescriptor {
    pub vertex_attributes: &'static [VertexAttribute],
    pub instance_attributes: &'static [VertexAttribute],
}

/// Method of uploading texel data from CPU to GPU.
#[derive(Debug, Clone)]
pub enum UploadMethod {
    /// Just call the device's direct sub-image upload (GL: `glTexSubImage`).
    Immediate,
    /// Accumulate the changes in a PBO first before transferring to a texture.
    PixelBuffer(VertexUsageHint),
}

/// Plain old data that can be used to initialize a texture.
pub unsafe trait Texel: Copy + Default {
    fn image_format() -> ImageFormat;
}

unsafe impl Texel for u8 {
    fn image_format() -> ImageFormat { ImageFormat::R8 }
}

/// Native/external texture format pair (some backends require both an
/// internal storage format and an external transfer format for uploads).
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
        TextureFormatPair { internal: value, external: value }
    }
}

/// Stride alignment requirement for PBO uploads.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum StrideAlignment {
    Bytes(NonZeroUsize),
    Pixels(NonZeroUsize),
}

impl StrideAlignment {
    pub fn num_bytes(&self, format: ImageFormat) -> NonZeroUsize {
        match *self {
            Self::Bytes(bytes) => bytes,
            Self::Pixels(pixels) => {
                let bpp = format.bytes_per_pixel() as usize;
                NonZeroUsize::new(pixels.get() * bpp).expect("non-zero stride")
            }
        }
    }
}

/// Errors produced by shader compilation/linking.
#[derive(Debug, Clone)]
pub enum ShaderError {
    Compilation(String, String), // name, error message
    Link(String, String),        // name, error message
}
