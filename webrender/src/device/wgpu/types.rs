/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Wrapper / marker structs used as `WgpuDevice`'s associated types.
//!
//! Each type satisfies one of the trait associated types declared in
//! `device::traits::{GpuShaders, GpuResources}`. Distinct types per assoc
//! type preserve the type-system contract; the field shapes match what
//! each method body needs.

use api::{ImageBufferKind, ImageFormat};
use api::units::{DeviceIntSize, FramebufferIntRect, FramebufferIntSize};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;

use super::super::types::{TextureFilter, VertexDescriptor};

// PipelineVariantKey lives in `device::types` (the backend-neutral module)
// so wgpu and the future wgpu-hal backend share the same key shape.
// Re-exported here so existing `super::types::PipelineVariantKey` imports
// in this directory keep working.
pub use super::super::types::PipelineVariantKey;

/// A wgpu-backed shader program.
///
/// Bundles vert + frag SPIR-V `ShaderModule`s plus the uniform buffer for
/// the `WrLocals { uTransform }` UBO. Pipelines are cached lazily per
/// `PipelineVariantKey` (state combination): a draw with new state
/// builds + caches a fresh pipeline; subsequent draws with the same
/// state reuse it.
///
/// The base/default pipeline (`PipelineVariantKey::DEFAULT`) is built
/// eagerly by `link_program`.
pub struct WgpuProgram {
    pub vert_module: wgpu::ShaderModule,
    pub frag_module: wgpu::ShaderModule,
    /// Pipeline variants keyed by render-state combination. Includes the
    /// `DEFAULT` key (no blend, full color write) seeded by `link_program`.
    /// `Rc<RefCell<...>>` so `bind_program` can stash an Rc-clone on the
    /// device — variants built at draw time after `bind_program` returns
    /// land in the same map and stay cached for subsequent draws.
    pub pipelines: Rc<RefCell<HashMap<PipelineVariantKey, wgpu::RenderPipeline>>>,
    /// Uniform buffer for WrLocals { mat4 uTransform; }. 64 bytes.
    pub uniform_buffer: wgpu::Buffer,
    /// Stem name (e.g. "ps_clear", "brush_solid_ALPHA_PASS") for diagnostics.
    pub stem: String,
    /// Vertex descriptor used at link time. Stored so variant pipelines
    /// can reuse the same vertex layout. The `&'static [VertexAttribute]`
    /// slices inside are cheap to keep.
    pub descriptor: VertexDescriptor,
}

/// wgpu doesn't have per-uniform locations — bindings are at the bind-group
/// level, by index not by name. Placeholder that satisfies the trait
/// associated type without carrying meaningful state. Renderer code that
/// writes a uniform value goes through `set_uniforms` (writes to the
/// program's uniform buffer at offset 0), not via this location handle.
pub struct WgpuUniformLocation;

/// A wgpu-backed texture. Holds the GPU resource + a default view +
/// metadata mirroring what GL's `Texture` carries. The view is created
/// alongside the texture so `bind_texture` doesn't have to lazily
/// construct one per draw.
pub struct WgpuTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub format: ImageFormat,
    pub size: api::units::DeviceIntSize,
    pub filter: TextureFilter,
    pub target: ImageBufferKind,
    pub is_render_target: bool,
}

/// Vertex array object equivalent. wgpu has no VAO concept — at draw
/// time, the renderer binds vertex/instance/index buffers directly to
/// the RenderPass. `WgpuVao` bundles them so the renderer can keep the
/// "VAO = a complete vertex setup" abstraction.
///
/// All three buffer slots start as `None`; the corresponding `update_vao_*`
/// methods allocate (or reallocate when growing) on first call. The
/// trait takes `&Self::Vao` (not `&mut`) for those updates — matching
/// GL's `&VAO` signature where mutation flowed via global GL state — so
/// we use `RefCell`/`Cell` for interior mutability.
pub struct WgpuVao {
    pub vertex_buffer: RefCell<Option<wgpu::Buffer>>,
    pub vertex_count: Cell<usize>,
    pub instance_buffer: RefCell<Option<wgpu::Buffer>>,
    pub instance_count: Cell<usize>,
    pub index_buffer: RefCell<Option<wgpu::Buffer>>,
    pub index_count: Cell<usize>,
    /// The descriptor used to create this VAO; `&'static`-borrowed slices
    /// inside, so owning the struct is cheap.
    pub descriptor: VertexDescriptor,
    pub instance_divisor: u32,
}

/// Custom VAO — multi-stream vertex setup. One buffer per Stream in
/// the original `&[Stream<'_>]`.
pub struct WgpuCustomVao {
    pub buffers: Vec<wgpu::Buffer>,
}

/// PBO equivalent. Generic `wgpu::Buffer` used for staged uploads and
/// readback. `buffer` is `None` for default-constructed PBOs;
/// `create_pbo_with_size` populates it.
pub struct WgpuPbo {
    pub buffer: Option<wgpu::Buffer>,
    pub size: usize,
}

/// Custom-VAO stream descriptor (placeholder — has no constructor).
/// `create_custom_vao` is currently unreachable through cross-backend
/// renderer code paths; real impl lands when a renderer call site wires
/// through.
pub struct WgpuStream<'a>(PhantomData<&'a ()>);

/// Vertex/index buffer (VBO equivalent). Generic over element type T
/// (PhantomData enforcement only). `buffer` is `None` until
/// `allocate_vbo` runs.
pub struct WgpuVbo<T> {
    pub buffer: Option<wgpu::Buffer>,
    pub count: usize,
    _marker: PhantomData<T>,
}

impl<T> WgpuVbo<T> {
    pub(super) fn new() -> Self {
        WgpuVbo { buffer: None, count: 0, _marker: PhantomData }
    }
}

/// Render target identity marker. wgpu has no FBO concept — render passes
/// attach a `TextureView` directly when started. Today this handle is
/// opaque + identity-only; P5+ may evolve it (or, more likely, the
/// `WgpuDrawTarget` enum carries the view directly per Option II of the
/// FBO design discussion).
#[derive(Copy, Clone)]
pub struct WgpuRenderTargetHandle;

pub struct WgpuReadTarget;

/// Draw destination for a render pass (Option II of the FBO design — the
/// view flows through the enum directly rather than via
/// `WgpuRenderTargetHandle` indirection).
///
/// Each variant carries `Arc<wgpu::TextureView>` so that constructing /
/// passing a `WgpuDrawTarget` is cheap (just an Arc bump). The renderer
/// is expected to construct these from a `WgpuTexture`'s view at the
/// point where it'd otherwise call `bind_draw_target`.
#[derive(Clone)]
pub enum WgpuDrawTarget {
    /// Target the device's default surface frame (when one exists).
    Default {
        rect: FramebufferIntRect,
        total_size: FramebufferIntSize,
    },
    /// Target a renderable texture.
    Texture {
        view: Arc<wgpu::TextureView>,
        dimensions: DeviceIntSize,
        with_depth: bool,
    },
    /// Target an externally-supplied texture view (e.g. host-shared).
    External {
        view: Arc<wgpu::TextureView>,
        size: FramebufferIntSize,
    },
}

impl WgpuDrawTarget {
    pub fn dimensions(&self) -> DeviceIntSize {
        match self {
            WgpuDrawTarget::Default { total_size, .. } => total_size.cast_unit(),
            WgpuDrawTarget::Texture { dimensions, .. } => *dimensions,
            WgpuDrawTarget::External { size, .. } => size.cast_unit(),
        }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        match self {
            WgpuDrawTarget::Texture { view, .. }
            | WgpuDrawTarget::External { view, .. } => view,
            WgpuDrawTarget::Default { .. } => {
                panic!("WgpuDrawTarget::Default has no inline view; surface acquisition is P5+ work")
            }
        }
    }
}

/// Embedder-supplied external texture (host-shared `wgpu::Texture`).
/// Wraps an `Arc<wgpu::TextureView>` for the bind group entry plus an
/// optional `Arc<wgpu::Sampler>` (when `None`, the device's default
/// sampler is used at bind time).
///
/// Constructors are exposed via `WgpuExternalTexture::new` so embedders
/// that own a `wgpu::Texture` (e.g. host-shared compositor surfaces)
/// can wrap it once and hand it to the renderer's external_images map.
pub struct WgpuExternalTexture {
    pub view: Arc<wgpu::TextureView>,
    pub sampler: Option<Arc<wgpu::Sampler>>,
}

impl WgpuExternalTexture {
    pub fn new(view: Arc<wgpu::TextureView>, sampler: Option<Arc<wgpu::Sampler>>) -> Self {
        WgpuExternalTexture { view, sampler }
    }
}

pub struct WgpuUploadPboPool;

/// Lifetime-bound RAII handle for a CPU-mapped PBO; tied to `&mut self`
/// scope. Real impl lands when `map_pbo_for_readback` is wired up.
pub struct WgpuBoundPbo<'a>(PhantomData<&'a ()>);

/// Per-frame texture-upload session bound to the borrowed PBO pool.
/// Real impl lands when `upload_texture` is wired up.
pub struct WgpuTextureUploader<'a>(PhantomData<&'a ()>);

/// Maps WebRender's `ImageFormat` to wgpu's `TextureFormat`. Used by
/// `create_texture`. BGRA8 picks the linear (`Bgra8Unorm`) variant; sRGB
/// conversions happen at pipeline level, not via the texture format.
pub(crate) fn image_format_to_wgpu(fmt: ImageFormat) -> wgpu::TextureFormat {
    use wgpu::TextureFormat as TF;
    match fmt {
        ImageFormat::R8 => TF::R8Unorm,
        ImageFormat::R16 => TF::R16Unorm,
        ImageFormat::BGRA8 => TF::Bgra8Unorm,
        ImageFormat::RGBAF32 => TF::Rgba32Float,
        ImageFormat::RG8 => TF::Rg8Unorm,
        ImageFormat::RG16 => TF::Rg16Unorm,
        ImageFormat::RGBAI32 => TF::Rgba32Sint,
        ImageFormat::RGBA8 => TF::Rgba8Unorm,
    }
}
