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

use api::{ImageFormat, Parameter};
use crate::internal_types::SwizzleSettings;
use malloc_size_of::MallocSizeOfOps;
use crate::render_api::MemoryReport;
use std::os::raw::c_void;

// Types currently defined in the GL device module that the trait signatures
// reference. As the wgpu backend is added, types that are truly
// backend-neutral (like `GpuFrameId`, `Capabilities`, `UploadMethod`,
// `StrideAlignment`, `TextureFormatPair`) should migrate out of `gl.rs` into
// a shared location. For P0 they stay where they are; the trait references
// them through this import.
use super::gl::{
    Capabilities,
    GpuFrameId,
    StrideAlignment,
    TextureFormatPair,
    UploadMethod,
};

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
/// Method signatures and associated types are added in subsequent P0 commits as
/// methods are moved out of `impl Device` in `gl.rs`. See the assignment table
/// for the full list.
pub trait GpuResources: GpuFrame {
    // Associated types (filled in as methods are moved):
    //   type Texture;
    //   type ExternalTexture;
    //   type Vao;
    //   type CustomVao;
    //   type Pbo;
    //   type BoundPbo<'a>;          (GAT, lifetime tied to &mut self)
    //   type TextureUploader<'a>;   (GAT)
    //   type Vbo<T>;                (GAT, generic over element type)
    //
    // Methods to follow: create_texture, delete_texture, copy_*_texture*,
    //   invalidate_*_target, reuse_render_target, create_fbo*, delete_fbo,
    //   create_pbo*, delete_pbo, create_vao*, delete_vao, create_custom_vao,
    //   delete_custom_vao, create_vbo, delete_vbo, allocate_vbo, fill_vbo,
    //   update_vao_*, upload_texture*, map_pbo_for_readback,
    //   attach_read_texture*, required_upload_size_and_stride.
}

/// Program / pipeline / uniform-location lifecycle.
///
/// In wgpu terms, "program" maps to a `RenderPipeline` keyed by
/// (SPIRV module, vertex layout, baked state). Methods that *use* a bound
/// program (`bind_program`, `set_uniforms`, `set_shader_texture_size`) live on
/// `GpuPass` because they're per-pass operations, not lifecycle.
pub trait GpuShaders: GpuFrame {
    // Associated types (filled in as methods are moved):
    //   type Program;
    //   type UniformLocation;
    //
    // Methods to follow: create_program, create_program_linked, link_program,
    //   delete_program, get_uniform_location, bind_shader_samplers.
}

/// Per-pass binding, state, draw commands, blits, readback.
///
/// Supertrait of `GpuShaders + GpuResources` so the bind methods can name
/// `Self::Program`, `Self::Texture`, `Self::Vao` etc. without re-declaring
/// the associated types.
pub trait GpuPass: GpuShaders + GpuResources {
    // Methods to follow:
    //   bind_read_target, reset_read_target, bind_draw_target, reset_draw_target,
    //   bind_external_draw_target,
    //   bind_program, set_uniforms, set_shader_texture_size,
    //   bind_vao, bind_custom_vao, bind_texture, bind_external_texture,
    //   clear_target,
    //   enable_depth, disable_depth, enable_depth_write, disable_depth_write,
    //   disable_stencil, set_scissor_rect, enable_scissor, disable_scissor,
    //   enable_color_write, disable_color_write,
    //   set_blend, set_blend_mode (collapsed enum-keyed; see plan A2 + assignment doc),
    //   draw_triangles_u16, draw_triangles_u32, draw_indexed_triangles,
    //   draw_indexed_triangles_instanced_u16, draw_nonindexed_points,
    //   draw_nonindexed_lines,
    //   blit_render_target, blit_render_target_invert_y,
    //   read_pixels, read_pixels_into, read_pixels_into_pbo, get_tex_image_into.
}
