/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Backend-agnostic device traits for WebRender's renderer.
//!
//! Four traits split by concern; backends (currently `GlDevice`, future
//! `WgpuDevice`) implement all four. Renderer code that wants backend
//! independence takes trait bounds; renderer code that needs backend-specific
//! behaviour binds the concrete type.
//!
//! Trait hierarchy:
//!
//! ```text
//!   GpuFrame                  <- frame lifecycle, capabilities, parameters
//!     |
//!     +-- GpuResources        <- texture/buffer/sampler/FBO/PBO/VAO ownership
//!     |
//!     +-- GpuShaders          <- program/pipeline/uniform-location lifecycle
//!     |
//!     +-- GpuPass: GpuShaders + GpuResources
//!                             <- per-pass binding, state, draw, blit, readback
//! ```
//!
//! See `notes/2026-04-30_p0_method_assignment.md` for the full method-to-trait
//! assignment. This file is the authoritative trait declaration.
//!
//! ## Logical pass state vs. immediate state
//!
//! Several `GpuPass` methods (`set_blend_mode`, `set_blend`, `enable_depth`,
//! `disable_depth`, `enable_depth_write`, `disable_depth_write`,
//! `disable_stencil`, `enable_color_write`, `disable_color_write`) describe
//! **logical** pass state, not immediate per-call GPU state. Backends with
//! pipeline-baked state (wgpu, future wgpu-hal) translate state changes
//! into a pipeline-cache lookup keyed on `device::types::PipelineVariantKey`:
//! a draw with a new state combination builds + caches a fresh pipeline;
//! subsequent draws with the same combination reuse it. The GL backend
//! treats the same methods as immediate state (`glBlendFunc` etc.) since
//! GL state is mutable per-draw. Both fulfil the same contract from the
//! renderer's perspective.
//!
//! ## Resource usage hints
//!
//! Trait methods that create textures and buffers (`create_texture`,
//! `create_vbo`, `create_pbo`, plus the implicit usage of
//! `update_vao_main_vertices`/`update_vao_indices`/`update_vao_instances`)
//! deliberately do **not** carry explicit `wgpu::TextureUsages` /
//! `wgpu::BufferUsages` parameters. The existing parameters already
//! discriminate the relevant cases:
//!
//! - `create_texture(..., render_target: Option<RenderTargetInfo>)`
//!   distinguishes render-attachment vs sampled textures.
//! - `create_vbo<T>` / `update_vao_main_vertices` / `update_vao_instances`
//!   are vertex usage by definition (VBO = vertex buffer).
//! - `update_vao_indices` is index usage by definition.
//! - `create_pbo` is staging usage (CPU-mapped).
//!
//! Backends that need explicit usage flags (wgpu-hal in particular)
//! derive them from these existing parameters at impl time, optionally
//! widening to all-bits-set when uncertain. The cost is at most some
//! lost driver optimization; correctness is unaffected. Re-evaluate this
//! decision if the wgpu-hal impl surfaces a case where derivation is
//! genuinely impossible.

use api::{ImageBufferKind, ImageDescriptor, ImageFormat, Parameter};
use api::units::{DeviceIntRect, DeviceIntSize, DeviceSize, FramebufferIntRect};
use euclid::default::Transform3D;
use crate::internal_types::{RenderTargetInfo, Swizzle, SwizzleSettings};
use malloc_size_of::MallocSizeOfOps;
use crate::render_api::MemoryReport;
use std::num::NonZeroUsize;
use std::os::raw::c_void;

// Backend-neutral types referenced by the trait signatures. These all
// live in `device::types` (the shared module), so traits.rs has no
// dependency on any specific backend module — the trait surface is
// fully backend-neutral.
use super::types::{
    Capabilities, DepthFunction, GpuFrameId, ShaderError, StrideAlignment, Texel,
    TextureFilter, TextureFormatPair, TextureSlot, UploadMethod, VertexDescriptor,
    VertexUsageHint,
};
// Re-exported so `super::traits::BlendMode` continues to resolve at the
// existing call sites; the canonical definition lives in `super::types`.
pub use super::types::BlendMode;

/// Frame lifecycle, capabilities, parameters, and global queries.
///
/// Implemented by every backend. Supertrait of `GpuResources`, `GpuShaders`,
/// `GpuPass` so consumers binding any of those automatically get the frame
/// surface in scope.
pub trait GpuFrame {
    // --- Frame lifecycle ---

    fn begin_frame(&mut self) -> GpuFrameId;
    fn end_frame(&mut self);
    fn reset_state(&mut self);

    // --- Parameters ---

    fn set_parameter(&mut self, param: &Parameter);

    // --- Capability queries ---

    fn get_capabilities(&self) -> &Capabilities;
    fn max_texture_size(&self) -> i32;
    fn clamp_max_texture_size(&mut self, size: i32);
    fn surface_origin_is_top_left(&self) -> bool;
    fn preferred_color_formats(&self) -> TextureFormatPair<ImageFormat>;
    fn swizzle_settings(&self) -> Option<SwizzleSettings>;
    fn supports_extension(&self, extension: &str) -> bool;

    // --- Depth / ortho ---

    fn depth_bits(&self) -> i32;
    fn max_depth_ids(&self) -> i32;
    fn ortho_near_plane(&self) -> f32;
    fn ortho_far_plane(&self) -> f32;

    // --- Upload configuration ---

    fn required_pbo_stride(&self) -> StrideAlignment;
    fn upload_method(&self) -> &UploadMethod;
    fn use_batched_texture_uploads(&self) -> bool;
    fn use_draw_calls_for_texture_copy(&self) -> bool;
    fn batched_upload_threshold(&self) -> i32;

    // --- Diagnostics ---

    fn echo_driver_messages(&self);
    fn report_memory(&self, size_op_funs: &MallocSizeOfOps, swgl: *mut c_void) -> MemoryReport;
    fn depth_targets_memory(&self) -> usize;
}

/// Resource ownership and upload: textures, buffers, samplers, FBOs, PBOs, VAOs.
///
/// `attach_read_texture_external` (takes a raw `gl::GLuint`) and
/// `delete_external_texture` (cfg-gated to `feature = "replay"`) stay as
/// inherent methods on the concrete `Device` for now — both leak GL-typed
/// values that don't generalize cleanly.
pub trait GpuResources: GpuFrame {
    type Texture;
    type Vao;
    type CustomVao;
    type Pbo;
    /// Custom-VAO stream descriptor (vbo + per-attribute layout).
    /// GAT lifetime parameter is the borrow scope of the attribute slice
    /// inside each `Stream`.
    type Stream<'a>;
    /// Generic-element vertex buffer (GAT).
    type Vbo<T>;
    /// Opaque handle to a render target (GL: FBO id; wgpu: a textureview
    /// or surface frame, depending on backend usage).
    type RenderTargetHandle: Copy;
    /// Backend-specific descriptor for a read source. Each backend defines
    /// its own enum (GL: variants reference FBOId; wgpu: would reference
    /// TextureView etc.). Renderer code names the concrete type via the
    /// type alias path.
    type ReadTarget;
    /// Backend-specific descriptor for a draw destination. Variants in
    /// the GL impl carry FBOId + raw texture id/target; the wgpu impl
    /// would carry TextureView equivalents. Refactoring to a common
    /// parameterized form was considered and rejected — the variants
    /// want different handle shapes.
    type DrawTarget;
    /// Externally-owned texture handle (host-provided). GL: wraps GLuint
    /// + target; wgpu: would wrap TextureView + Sampler. Constructors
    /// stay on the concrete type since the renderer call sites are
    /// already backend-specific.
    type ExternalTexture;
    /// Pool of upload PBOs (or backend-equivalent staging buffers).
    /// Renderer holds one as a field; constructor stays concrete.
    type UploadPboPool;
    /// RAII handle for a CPU-mapped PBO; lifetime ties it to the bound state.
    type BoundPbo<'a>
    where
        Self: 'a;
    /// Per-frame texture upload session; lifetime tied to the borrowed PBO pool.
    type TextureUploader<'a>;

    // --- Texture lifecycle ---

    fn create_texture(
        &mut self,
        target: ImageBufferKind,
        format: ImageFormat,
        width: i32,
        height: i32,
        filter: TextureFilter,
        render_target: Option<RenderTargetInfo>,
    ) -> Self::Texture;

    fn delete_texture(&mut self, texture: Self::Texture);

    fn copy_entire_texture(&mut self, dst: &mut Self::Texture, src: &Self::Texture);

    fn copy_texture_sub_region(
        &mut self,
        src_texture: &Self::Texture,
        src_x: usize,
        src_y: usize,
        dest_texture: &Self::Texture,
        dest_x: usize,
        dest_y: usize,
        width: usize,
        height: usize,
    );

    fn invalidate_render_target(&mut self, texture: &Self::Texture);
    fn invalidate_depth_target(&mut self);

    fn reuse_render_target<T: Texel>(
        &mut self,
        texture: &mut Self::Texture,
        rt_info: RenderTargetInfo,
    );

    // --- FBO lifecycle ---

    fn create_fbo(&mut self) -> Self::RenderTargetHandle;
    fn create_fbo_for_external_texture(&mut self, texture_id: u32) -> Self::RenderTargetHandle;
    fn delete_fbo(&mut self, fbo: Self::RenderTargetHandle);

    // --- PBO lifecycle ---

    fn create_pbo(&mut self) -> Self::Pbo;
    fn create_pbo_with_size(&mut self, size: usize) -> Self::Pbo;
    fn delete_pbo(&mut self, pbo: Self::Pbo);

    // --- VAO lifecycle ---

    fn create_vao(&mut self, descriptor: &VertexDescriptor, instance_divisor: u32) -> Self::Vao;
    fn create_vao_with_new_instances(
        &mut self,
        descriptor: &VertexDescriptor,
        base_vao: &Self::Vao,
    ) -> Self::Vao;
    fn delete_vao(&mut self, vao: Self::Vao);

    fn create_custom_vao<'a>(&mut self, streams: &[Self::Stream<'a>]) -> Self::CustomVao;
    fn delete_custom_vao(&mut self, vao: Self::CustomVao);

    // --- VBO lifecycle (generic over element type) ---

    fn create_vbo<T>(&mut self) -> Self::Vbo<T>;
    fn delete_vbo<T>(&mut self, vbo: Self::Vbo<T>);
    fn allocate_vbo<V>(
        &mut self,
        vbo: &mut Self::Vbo<V>,
        count: usize,
        usage_hint: VertexUsageHint,
    );
    fn fill_vbo<V>(&mut self, vbo: &Self::Vbo<V>, data: &[V], offset: usize);

    // --- VAO buffer updates ---

    fn update_vao_main_vertices<V>(
        &mut self,
        vao: &Self::Vao,
        vertices: &[V],
        usage_hint: VertexUsageHint,
    );
    fn update_vao_instances<V: Clone>(
        &mut self,
        vao: &Self::Vao,
        instances: &[V],
        usage_hint: VertexUsageHint,
        repeat: Option<NonZeroUsize>,
    );
    fn update_vao_indices<I>(
        &mut self,
        vao: &Self::Vao,
        indices: &[I],
        usage_hint: VertexUsageHint,
    );

    // --- Upload paths ---

    fn upload_texture<'a>(&mut self, pbo_pool: &'a mut Self::UploadPboPool) -> Self::TextureUploader<'a>;

    fn upload_texture_immediate<T: Texel>(&mut self, texture: &Self::Texture, pixels: &[T]);

    fn map_pbo_for_readback<'a>(&'a mut self, pbo: &'a Self::Pbo) -> Option<Self::BoundPbo<'a>>;

    // --- Read-target attachment ---

    fn attach_read_texture(&mut self, texture: &Self::Texture);

    // --- Upload sizing query ---

    fn required_upload_size_and_stride(
        &self,
        size: DeviceIntSize,
        format: ImageFormat,
    ) -> (usize, usize);
}

/// Program / pipeline / uniform-location lifecycle.
///
/// In wgpu terms, "program" maps to a `RenderPipeline` keyed by
/// (SPIRV module, vertex layout, baked state). Methods that *use* a bound
/// program (`bind_program`, `set_uniforms`, `set_shader_texture_size`) live on
/// `GpuPass` because they're per-pass operations, not lifecycle.
pub trait GpuShaders: GpuFrame {
    /// Concrete program/pipeline handle owned by this backend.
    type Program;
    /// Concrete uniform-location handle owned by this backend.
    type UniformLocation;

    fn create_program(
        &mut self,
        base_filename: &'static str,
        features: &[&'static str],
    ) -> Result<Self::Program, ShaderError>;

    fn create_program_linked(
        &mut self,
        base_filename: &'static str,
        features: &[&'static str],
        descriptor: &VertexDescriptor,
    ) -> Result<Self::Program, ShaderError>;

    fn link_program(
        &mut self,
        program: &mut Self::Program,
        descriptor: &VertexDescriptor,
    ) -> Result<(), ShaderError>;

    fn delete_program(&mut self, program: Self::Program);

    fn get_uniform_location(&self, program: &Self::Program, name: &str) -> Self::UniformLocation;

    fn bind_shader_samplers<S>(&mut self, program: &Self::Program, bindings: &[(&'static str, S)])
    where
        S: Into<TextureSlot> + Copy;
}

/// Per-pass binding, state, draw commands, blits, readback.
///
/// Supertrait of `GpuShaders + GpuResources` so the bind methods can name
/// `Self::Program`, `Self::Texture`, `Self::Vao` etc. without re-declaring
/// the associated types.
pub trait GpuPass: GpuShaders + GpuResources {
    // --- Render target binding ---

    fn bind_read_target(&mut self, target: Self::ReadTarget);
    fn reset_read_target(&mut self);
    fn bind_draw_target(&mut self, target: Self::DrawTarget);
    fn reset_draw_target(&mut self);
    fn bind_external_draw_target(&mut self, fbo_id: Self::RenderTargetHandle);

    // --- Program / uniform binding (per-pass operations on a bound program) ---

    fn bind_program(&mut self, program: &Self::Program) -> bool;
    fn set_uniforms(&self, program: &Self::Program, transform: &Transform3D<f32>);
    fn set_shader_texture_size(&self, program: &Self::Program, texture_size: DeviceSize);

    // --- Vertex array / texture binding ---

    fn bind_vao(&mut self, vao: &Self::Vao);
    fn bind_custom_vao(&mut self, vao: &Self::CustomVao);

    fn bind_texture<S>(&mut self, slot: S, texture: &Self::Texture, swizzle: Swizzle)
    where
        S: Into<TextureSlot>;

    fn bind_external_texture<S>(&mut self, slot: S, external_texture: &Self::ExternalTexture)
    where
        S: Into<TextureSlot>;

    // --- Clears ---

    fn clear_target(
        &self,
        color: Option<[f32; 4]>,
        depth: Option<f32>,
        rect: Option<FramebufferIntRect>,
    );

    // --- Depth / stencil state ---

    fn enable_depth(&self, depth_func: DepthFunction);
    fn disable_depth(&self);
    fn enable_depth_write(&self);
    fn disable_depth_write(&self);
    fn disable_stencil(&self);

    // --- Scissor / color write ---

    fn set_scissor_rect(&self, rect: FramebufferIntRect);
    fn enable_scissor(&self);
    fn disable_scissor(&self);
    fn enable_color_write(&self);
    fn disable_color_write(&self);

    // --- Blend state ---

    fn set_blend(&mut self, enable: bool);

    /// Selects the blend equation/factors for subsequent draws. The 16
    /// per-mode `set_blend_mode_*` methods on `Device` remain as internal
    /// dispatchers but are no longer part of the trait surface.
    fn set_blend_mode(&mut self, mode: BlendMode);

    // --- Draw commands ---

    fn draw_triangles_u16(&mut self, first_vertex: i32, index_count: i32);
    fn draw_triangles_u32(&mut self, first_vertex: i32, index_count: i32);
    fn draw_indexed_triangles(&mut self, index_count: i32);
    fn draw_indexed_triangles_instanced_u16(&mut self, index_count: i32, instance_count: i32);
    fn draw_nonindexed_points(&mut self, first_vertex: i32, vertex_count: i32);
    fn draw_nonindexed_lines(&mut self, first_vertex: i32, vertex_count: i32);

    // --- Blits ---

    fn blit_render_target(
        &mut self,
        src_target: Self::ReadTarget,
        src_rect: FramebufferIntRect,
        dest_target: Self::DrawTarget,
        dest_rect: FramebufferIntRect,
        filter: TextureFilter,
    );
    fn blit_render_target_invert_y(
        &mut self,
        src_target: Self::ReadTarget,
        src_rect: FramebufferIntRect,
        dest_target: Self::DrawTarget,
        dest_rect: FramebufferIntRect,
    );

    // --- Readback ---

    fn read_pixels(&mut self, img_desc: &ImageDescriptor) -> Vec<u8>;
    fn read_pixels_into(&mut self, rect: FramebufferIntRect, format: ImageFormat, output: &mut [u8]);
    fn read_pixels_into_pbo(
        &mut self,
        read_target: Self::ReadTarget,
        rect: DeviceIntRect,
        format: ImageFormat,
        pbo: &Self::Pbo,
    );
    fn get_tex_image_into(&mut self, texture: &Self::Texture, format: ImageFormat, output: &mut [u8]);
}
