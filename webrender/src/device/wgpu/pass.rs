/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! `GpuPass` impl for `WgpuDevice` (P5).
//!
//! Render-pass / draw machinery. Most state-recording methods (bind_*,
//! enable_*, set_blend_*) update fields on `WgpuDevice`; draw methods
//! consume that state to open a render pass + issue a draw + close it.
//!
//! Current approach is "one render pass per draw" — wasteful but
//! correctness-first. Batching multiple draws into a single pass between
//! `bind_draw_target` boundaries is a future optimization.

use api::{ImageDescriptor, ImageFormat};
use api::units::{DeviceIntRect, DeviceSize, FramebufferIntRect};
use euclid::default::Transform3D;

use crate::internal_types::Swizzle;

use super::super::traits::{BlendMode, GpuPass, GpuResources, GpuShaders};
use super::super::types::{DepthFunction, TextureFilter, TextureSlot, VertexDescriptor};
use super::shaders::{blend_state_for, color_writes_from_mask};
use super::types::PipelineVariantKey;
use super::vertex_layout::WgpuVertexLayouts;
use super::{BoundProgram, WgpuDevice};

/// Builds a pipeline variant given a `BoundProgram` + state key. Mirrors
/// `shaders::build_pipeline_variant` but operates on the bound snapshot
/// (which has the modules + descriptor + uniform_buffer needed) rather
/// than `WgpuProgram` directly. Kept in pass.rs to avoid a circular
/// pass.rs <-> shaders.rs reference that the build function would need
/// to take a generic "pipeline-build sources" trait.
fn build_variant_inline(
    device: &wgpu::Device,
    bp: &BoundProgram,
    key: PipelineVariantKey,
) -> wgpu::RenderPipeline {
    let layouts = WgpuVertexLayouts::from_descriptor(&bp.descriptor);
    let buffers = layouts.buffers();
    let nonempty: Vec<wgpu::VertexBufferLayout<'_>> = buffers
        .iter()
        .filter(|b| !b.attributes.is_empty())
        .cloned()
        .collect();

    let blend = key.blend.map(blend_state_for);
    let write_mask = color_writes_from_mask(key.color_write_mask);

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(&format!(
            "WgpuProgram[{}] variant blend={:?} mask={:#x}",
            bp.stem, key.blend, key.color_write_mask,
        )),
        layout: None,
        vertex: wgpu::VertexState {
            module: &bp.vert_module,
            entry_point: Some("main"),
            buffers: &nonempty,
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &bp.frag_module,
            entry_point: Some("main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Bgra8Unorm,
                blend,
                write_mask,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

impl GpuPass for WgpuDevice {
    fn bind_read_target(&mut self, _target: <Self as GpuResources>::ReadTarget) {
        // Readback path — separate from the draw path. P5+ wires this.
    }
    fn reset_read_target(&mut self) {
        // No-op for now.
    }

    fn bind_draw_target(&mut self, target: <Self as GpuResources>::DrawTarget) {
        // Records the new target. Render passes are opened lazily at draw
        // time using this target's view. If a previous target's draws are
        // pending in the encoder, they're already finalized (each draw is
        // its own pass in the current minimum-viable approach).
        self.current_target = Some(target);
    }

    fn reset_draw_target(&mut self) {
        self.current_target = None;
    }

    fn bind_external_draw_target(&mut self, _fbo_id: <Self as GpuResources>::RenderTargetHandle) {
        // External draw target binding currently not exercised through
        // wgpu. The renderer's call sites for this are GL-specific.
    }

    fn bind_program(&mut self, program: &<Self as GpuShaders>::Program) -> bool {
        // GL-style returns false if program isn't linked; for wgpu, that
        // means no DEFAULT pipeline in the cache (link_program seeds it).
        let has_default = program
            .pipelines
            .borrow()
            .contains_key(&PipelineVariantKey::DEFAULT);
        if !has_default {
            return false;
        }
        // Snapshot what issue_draw needs to (re)build variants. wgpu
        // handles + Rc are all cheap clones.
        self.bound_program = Some(BoundProgram {
            vert_module: program.vert_module.clone(),
            frag_module: program.frag_module.clone(),
            uniform_buffer: program.uniform_buffer.clone(),
            stem: program.stem.clone(),
            descriptor: VertexDescriptor {
                vertex_attributes: program.descriptor.vertex_attributes,
                instance_attributes: program.descriptor.instance_attributes,
            },
            pipelines: program.pipelines.clone(),
        });
        self.bound_uniform_buffer = Some(program.uniform_buffer.clone());
        // bound_pipeline left for resolve_pipeline_variant to fill in
        // based on current state; clear stale value from a prior bind.
        self.bound_pipeline = None;
        true
    }

    fn set_uniforms(
        &self,
        program: &<Self as GpuShaders>::Program,
        transform: &Transform3D<f32>,
    ) {
        // WrLocals UBO is `mat4 uTransform;` at offset 0. Write the
        // 64-byte matrix directly via queue.write_buffer (deferred to
        // next submission).
        let bytes = transform.to_array();
        // Transform3D::to_array() is [f32; 16] = 64 bytes.
        let byte_slice: &[u8] = unsafe {
            std::slice::from_raw_parts(
                bytes.as_ptr() as *const u8,
                std::mem::size_of_val(&bytes),
            )
        };
        self.queue.write_buffer(&program.uniform_buffer, 0, byte_slice);
    }

    fn set_shader_texture_size(
        &self,
        _program: &<Self as GpuShaders>::Program,
        _texture_size: DeviceSize,
    ) {
        // GL: writes a uTextureSize uniform (used by some shaders for
        // texelFetch coordinate scaling). For wgpu, this would write to
        // a uniform buffer; current corpus doesn't reflect a uTextureSize
        // location, so this is a no-op until an actual user surfaces.
    }

    fn bind_vao(&mut self, vao: &<Self as GpuResources>::Vao) {
        // wgpu::Buffer is cheap-clone.
        self.bound_vertex_buffer = vao.vertex_buffer.borrow().clone();
        self.bound_instance_buffer = vao.instance_buffer.borrow().clone();
        self.bound_index_buffer = vao.index_buffer.borrow().clone();
    }

    fn bind_custom_vao(&mut self, _vao: &<Self as GpuResources>::CustomVao) {
        // Custom VAOs use multi-stream layout; not yet exercised through
        // wgpu (renderer call sites are GL-specific).
    }

    fn bind_texture<S>(
        &mut self,
        slot: S,
        texture: &<Self as GpuResources>::Texture,
        _swizzle: Swizzle,
    )
    where
        S: Into<TextureSlot>,
    {
        // Records (slot → view) for `issue_draw` to consume when building
        // the frag-stage bind group. wgpu::TextureView is cheap-clone
        // (Arc internally). Swizzle is ignored — wgpu has no per-texture
        // swizzle (matches `swizzle_settings()` returning None).
        let slot_idx = slot.into().0;
        self.bound_textures.insert(slot_idx, texture.view.clone());
        // Clear any sampler override left by a prior bind_external_texture
        // on this slot — bind_texture uses the device's default sampler.
        self.bound_sampler_overrides.remove(&slot_idx);
    }

    fn bind_external_texture<S>(
        &mut self,
        slot: S,
        external_texture: &<Self as GpuResources>::ExternalTexture,
    )
    where
        S: Into<TextureSlot>,
    {
        // Same shape as bind_texture, but the view comes from the
        // embedder's host-shared wgpu texture and the sampler may be
        // overridden per-binding (None = use default sampler).
        let slot_idx = slot.into().0;
        // Cheap clone — Arc-wrapped TextureView underneath.
        self.bound_textures
            .insert(slot_idx, (*external_texture.view).clone());
        match external_texture.sampler.as_ref() {
            Some(sampler) => {
                self.bound_sampler_overrides.insert(slot_idx, sampler.clone());
            }
            None => {
                self.bound_sampler_overrides.remove(&slot_idx);
            }
        }
    }

    fn clear_target(
        &self,
        color: Option<[f32; 4]>,
        _depth: Option<f32>,
        _rect: Option<FramebufferIntRect>,
    ) {
        // Records pending clear for the next render pass open. Trait
        // takes &self (matches GL); pending_clear is a Cell for safe
        // interior mutability.
        //
        // Trade-off: this means a renderer that calls clear_target
        // without following draws won't see anything happen. WebRender's
        // pattern is always clear-then-draw, so this is fine. If a
        // standalone clear ever surfaces, we'd open + close a pass
        // immediately (requires RefCell on frame_encoder).
        let Some(rgba) = color else { return };
        self.pending_clear.set(Some(wgpu::Color {
            r: rgba[0] as f64,
            g: rgba[1] as f64,
            b: rgba[2] as f64,
            a: rgba[3] as f64,
        }));
    }

    // Depth state recording: stored on the device but not yet honored by
    // pipeline construction (depth_stencil = None always until depth-
    // target rendering lands). State methods are real no-ops in effect.
    fn enable_depth(&self, _depth_func: DepthFunction) {}
    fn disable_depth(&self) {}
    fn enable_depth_write(&self) {}
    fn disable_depth_write(&self) {}
    fn disable_stencil(&self) {}

    fn set_scissor_rect(&self, rect: FramebufferIntRect) {
        self.scissor_rect.set(Some(rect));
    }
    fn enable_scissor(&self) {
        self.scissor_enabled.set(true);
    }
    fn disable_scissor(&self) {
        self.scissor_enabled.set(false);
    }

    fn enable_color_write(&self) {
        self.color_write_mask.set(0xF);
    }
    fn disable_color_write(&self) {
        self.color_write_mask.set(0x0);
    }

    fn set_blend(&mut self, enable: bool) {
        self.blend_enabled.set(enable);
    }
    fn set_blend_mode(&mut self, mode: BlendMode) {
        self.blend_mode.set(Some(mode));
    }

    fn draw_triangles_u16(&mut self, _first_vertex: i32, index_count: i32) {
        // GL signature passes first_vertex as a byte offset into the
        // index buffer; for u16 indices that's first_vertex/2 in element
        // terms. wgpu's draw_indexed handles this via the index range
        // argument. For typical WebRender draws first_vertex is 0;
        // non-zero offsets are P5+ work.
        self.issue_draw_indexed(index_count as u32, 1, wgpu::IndexFormat::Uint16);
    }

    fn draw_triangles_u32(&mut self, _first_vertex: i32, index_count: i32) {
        self.issue_draw_indexed(index_count as u32, 1, wgpu::IndexFormat::Uint32);
    }

    fn draw_indexed_triangles(&mut self, index_count: i32) {
        self.issue_draw_indexed(index_count as u32, 1, wgpu::IndexFormat::Uint16);
    }

    fn draw_indexed_triangles_instanced_u16(
        &mut self,
        index_count: i32,
        instance_count: i32,
    ) {
        self.issue_draw_indexed(
            index_count as u32,
            instance_count.max(1) as u32,
            wgpu::IndexFormat::Uint16,
        );
    }

    fn draw_nonindexed_points(&mut self, _first_vertex: i32, _vertex_count: i32) {
        // Non-indexed draws need a different code path (no index buffer
        // bind). Used for debug primitives only; defer until those land.
    }
    fn draw_nonindexed_lines(&mut self, _first_vertex: i32, _vertex_count: i32) {
        // Same as draw_nonindexed_points; debug-only path.
    }

    fn blit_render_target(
        &mut self,
        _src_target: <Self as GpuResources>::ReadTarget,
        _src_rect: FramebufferIntRect,
        _dest_target: <Self as GpuResources>::DrawTarget,
        _dest_rect: FramebufferIntRect,
        _filter: TextureFilter,
    ) {
        // Blit via copy_texture_to_texture is the wgpu equivalent;
        // implementation deferred.
    }

    fn blit_render_target_invert_y(
        &mut self,
        _src_target: <Self as GpuResources>::ReadTarget,
        _src_rect: FramebufferIntRect,
        _dest_target: <Self as GpuResources>::DrawTarget,
        _dest_rect: FramebufferIntRect,
    ) {}

    fn read_pixels(&mut self, img_desc: &ImageDescriptor) -> Vec<u8> {
        // Reads from the texture currently attached via
        // `attach_read_texture`, starting at (0, 0) of size
        // `img_desc.size`. Mirrors GL's `read_pixels` which calls
        // `glReadPixels(0, 0, w, h, ...)`. ImageDescriptor.offset is a
        // byte offset in a backing buffer (tiling metadata), not a
        // source-rect coordinate, so it isn't used here.
        let Some(texture) = self.current_read_texture.borrow().clone() else {
            log::warn!("read_pixels: no read texture attached, returning empty");
            return Vec::new();
        };
        let extent = wgpu::Extent3d {
            width: img_desc.size.width.max(0) as u32,
            height: img_desc.size.height.max(0) as u32,
            depth_or_array_layers: 1,
        };
        readback_texture_to_vec(
            &self.device, &self.queue, &texture, wgpu::Origin3d::ZERO, extent, img_desc.format,
        )
    }

    fn read_pixels_into(
        &mut self,
        rect: FramebufferIntRect,
        format: ImageFormat,
        output: &mut [u8],
    ) {
        let Some(texture) = self.current_read_texture.borrow().clone() else {
            log::warn!("read_pixels_into: no read texture attached, leaving output untouched");
            return;
        };
        let origin = wgpu::Origin3d {
            x: rect.min.x.max(0) as u32,
            y: rect.min.y.max(0) as u32,
            z: 0,
        };
        let extent = wgpu::Extent3d {
            width: rect.width().max(0) as u32,
            height: rect.height().max(0) as u32,
            depth_or_array_layers: 1,
        };
        readback_texture_into_slice(
            &self.device, &self.queue, &texture, origin, extent, format, output,
        );
    }

    fn read_pixels_into_pbo(
        &mut self,
        _read_target: <Self as GpuResources>::ReadTarget,
        _rect: DeviceIntRect,
        _format: ImageFormat,
        _pbo: &<Self as GpuResources>::Pbo,
    ) {
        // WgpuReadTarget is currently a unit marker — no source identity
        // to copy from. Implementing this path requires enriching
        // WgpuReadTarget with a Texture/View handle (similar to how
        // WgpuDrawTarget Option II carried view via the enum). Deferred
        // until the renderer's wgpu path has a call site that constructs
        // a real read target.
    }

    fn get_tex_image_into(
        &mut self,
        texture: &<Self as GpuResources>::Texture,
        format: ImageFormat,
        output: &mut [u8],
    ) {
        // Copies the entire texture into `output`. Caller-provided slice
        // must be at least width*height*bytes_per_pixel; rows are written
        // tightly packed (the 256-byte aligned staging buffer is compacted).
        let extent = wgpu::Extent3d {
            width: texture.size.width.max(0) as u32,
            height: texture.size.height.max(0) as u32,
            depth_or_array_layers: 1,
        };
        readback_texture_into_slice(
            &self.device,
            &self.queue,
            &texture.texture,
            wgpu::Origin3d::ZERO,
            extent,
            format,
            output,
        );
    }
}

impl WgpuDevice {
    /// Resolves the right pipeline variant for current render state and
    /// stores it in `self.bound_pipeline`. Builds + caches the variant
    /// on cache miss. Returns false if no program is bound.
    fn resolve_pipeline_variant(&mut self) -> bool {
        let Some(bp) = self.bound_program.as_ref() else {
            return false;
        };
        let key = PipelineVariantKey {
            blend: if self.blend_enabled.get() {
                self.blend_mode.get()
            } else {
                None
            },
            color_write_mask: self.color_write_mask.get(),
        };
        // Borrow the cache; if hit, clone-out the pipeline; else build,
        // insert, and clone-out.
        let pipeline = {
            let cache = bp.pipelines.borrow();
            cache.get(&key).cloned()
        };
        let pipeline = match pipeline {
            Some(p) => p,
            None => {
                // Construct via shaders::build_pipeline_variant. Need
                // a synthetic WgpuProgram-like struct to pass in (it
                // only reads vert_module/frag_module/stem/descriptor),
                // OR we inline the build here. Inline is simpler.
                let new_pipeline = build_variant_inline(&self.device, bp, key);
                bp.pipelines.borrow_mut().insert(key, new_pipeline.clone());
                new_pipeline
            }
        };
        self.bound_pipeline = Some(pipeline);
        true
    }

    /// Opens a render pass against `current_target`, replays the bound
    /// pipeline + buffers + bind group(s), issues an instanced indexed
    /// draw, closes the pass. One pass per draw — correctness over
    /// performance for the minimum viable P5.
    fn issue_draw_indexed(
        &mut self,
        index_count: u32,
        instance_count: u32,
        index_format: wgpu::IndexFormat,
    ) {
        // Resolve the pipeline variant for current state first (needs
        // &mut self). Updates self.bound_pipeline; cached on WgpuProgram.
        if !self.resolve_pipeline_variant() {
            return;
        }
        // Snapshot the target (Arc-cloned views internally — cheap).
        // Owning a copy lets us release the immutable self.current_target
        // borrow before we need to borrow self.frame_encoder mutably.
        let target = match self.current_target.as_ref() {
            Some(t) => t.clone(),
            None => return,
        };
        let Some(pipeline) = self.bound_pipeline.as_ref() else {
            return;
        };
        let Some(vertex_buffer) = self.bound_vertex_buffer.as_ref() else {
            return;
        };
        let Some(instance_buffer) = self.bound_instance_buffer.as_ref() else {
            return;
        };
        let Some(index_buffer) = self.bound_index_buffer.as_ref() else {
            return;
        };
        let Some(uniform_buffer) = self.bound_uniform_buffer.as_ref() else {
            return;
        };
        let Some(encoder) = self.frame_encoder.as_mut() else {
            // begin_frame wasn't called; nothing to record into.
            return;
        };

        // Build bind group for the WrLocals UBO. Auto-derived layout
        // exposes one BindGroupLayout per stage (vert→set 0, frag→set 1
        // per gen_spirv's per-stage descriptor set assignment). For
        // ps_clear, only set 0 (vert) has bindings — the WrLocals UBO
        // at binding 0.
        let bgl0 = pipeline.get_bind_group_layout(0);
        let bind_group_0 = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("WgpuDevice issue_draw set 0"),
            layout: &bgl0,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        // Consume any pending clear color (set by clear_target). If the
        // renderer cleared before drawing, this pass starts with a
        // LoadOp::Clear; otherwise it preserves prior content via Load.
        let load_op = match self.pending_clear.take() {
            Some(color) => wgpu::LoadOp::Clear(color),
            None => wgpu::LoadOp::Load,
        };

        // Build set 1 (frag stage textures + samplers) when textures
        // are bound. Empty Vec when none — wgpu accepts no-set-1 only
        // if the pipeline doesn't declare set 1, which matches ps_clear
        // (no textures). Textured shaders must have textures bound.
        let set1_layout;
        let bind_group_1;
        let needs_set1 = !self.bound_textures.is_empty();
        if needs_set1 {
            // Image at binding 2*i, sampler at binding 2*i+1, per
            // gen_spirv's split-then-renumber convention. We bind
            // every (image, sampler) pair — naga's auto-derived layout
            // exposes both. Per-slot sampler overrides (set by
            // bind_external_texture when the embedder supplied a
            // sampler) take precedence over the default sampler.
            let default_sampler =
                self.default_sampler.as_ref().expect("default sampler");
            let mut entries: Vec<wgpu::BindGroupEntry<'_>> = Vec::new();
            // Sort slots so the binding order is deterministic.
            let mut slots: Vec<&usize> = self.bound_textures.keys().collect();
            slots.sort();
            for &slot in &slots {
                let view = self.bound_textures.get(slot).unwrap();
                let image_binding = (*slot as u32) * 2;
                let sampler_binding = image_binding + 1;
                let sampler: &wgpu::Sampler = self
                    .bound_sampler_overrides
                    .get(slot)
                    .map(|s| s.as_ref())
                    .unwrap_or(default_sampler);
                entries.push(wgpu::BindGroupEntry {
                    binding: image_binding,
                    resource: wgpu::BindingResource::TextureView(view),
                });
                entries.push(wgpu::BindGroupEntry {
                    binding: sampler_binding,
                    resource: wgpu::BindingResource::Sampler(sampler),
                });
            }
            set1_layout = pipeline.get_bind_group_layout(1);
            bind_group_1 = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("WgpuDevice issue_draw set 1"),
                layout: &set1_layout,
                entries: &entries,
            }));
        } else {
            bind_group_1 = None;
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("WgpuDevice draw"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target.view(),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: load_op,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            pass.set_pipeline(pipeline);
            // Scissor is per-pass state (not pipeline). Apply if enabled
            // and a rect was set.
            if self.scissor_enabled.get() {
                if let Some(rect) = self.scissor_rect.get() {
                    let w = rect.width().max(0) as u32;
                    let h = rect.height().max(0) as u32;
                    let x = rect.min.x.max(0) as u32;
                    let y = rect.min.y.max(0) as u32;
                    pass.set_scissor_rect(x, y, w, h);
                }
            }
            pass.set_bind_group(0, &bind_group_0, &[]);
            if let Some(bg1) = bind_group_1.as_ref() {
                pass.set_bind_group(1, bg1, &[]);
            }
            pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            pass.set_vertex_buffer(1, instance_buffer.slice(..));
            pass.set_index_buffer(index_buffer.slice(..), index_format);
            pass.draw_indexed(0..index_count, 0, 0..instance_count);
        }
    }
}

// ---- Readback helpers (cluster #2) -------------------------------------
//
// Shared machinery used by `read_pixels`, `read_pixels_into`, and
// `get_tex_image_into`. The pattern is:
//   1. allocate a staging wgpu::Buffer with 256-byte aligned bytes_per_row
//      (wgpu's COPY_BYTES_PER_ROW_ALIGNMENT)
//   2. encoder.copy_texture_to_buffer + queue.submit
//   3. buffer.slice(..).map_async + device.poll(Wait) — synchronous wait
//   4. read mapped range, compact rows (drop trailing alignment padding)
//      into either an owned Vec<u8> or a caller-provided &mut [u8]
//
// Synchronous semantics match GL's glReadPixels. wgpu's async map needs
// an explicit poll; we use PollType::Wait so callers don't have to
// pump the device themselves.

fn readback_aligned_bpr(width: u32, bytes_per_pixel: u32) -> u32 {
    let unaligned = width * bytes_per_pixel;
    (unaligned + 255) & !255
}

fn readback_to_aligned_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    origin: wgpu::Origin3d,
    extent: wgpu::Extent3d,
    bytes_per_pixel: u32,
) -> (wgpu::Buffer, u32) {
    let aligned_bpr = readback_aligned_bpr(extent.width, bytes_per_pixel);
    let buffer_size = (aligned_bpr * extent.height) as u64;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("WgpuDevice readback staging"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("WgpuDevice readback encoder"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(aligned_bpr),
                rows_per_image: Some(extent.height),
            },
        },
        extent,
    );
    queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
        .expect("wgpu device poll for readback");
    (buffer, aligned_bpr)
}

fn readback_texture_to_vec(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    origin: wgpu::Origin3d,
    extent: wgpu::Extent3d,
    format: ImageFormat,
) -> Vec<u8> {
    if extent.width == 0 || extent.height == 0 {
        return Vec::new();
    }
    let bytes_per_pixel = format.bytes_per_pixel() as u32;
    let (buffer, aligned_bpr) =
        readback_to_aligned_buffer(device, queue, texture, origin, extent, bytes_per_pixel);
    let row_bytes = (extent.width * bytes_per_pixel) as usize;
    let mut out = Vec::with_capacity(row_bytes * extent.height as usize);
    let slice = buffer.slice(..);
    let data = slice.get_mapped_range();
    for row in 0..extent.height as usize {
        let src_off = row * aligned_bpr as usize;
        out.extend_from_slice(&data[src_off..src_off + row_bytes]);
    }
    drop(data);
    buffer.unmap();
    out
}

fn readback_texture_into_slice(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    origin: wgpu::Origin3d,
    extent: wgpu::Extent3d,
    format: ImageFormat,
    output: &mut [u8],
) {
    if extent.width == 0 || extent.height == 0 {
        return;
    }
    let bytes_per_pixel = format.bytes_per_pixel() as u32;
    let row_bytes = (extent.width * bytes_per_pixel) as usize;
    let needed = row_bytes * extent.height as usize;
    if output.len() < needed {
        log::warn!(
            "readback_texture_into_slice: output too small ({} < {} for {}x{})",
            output.len(), needed, extent.width, extent.height,
        );
        return;
    }
    let (buffer, aligned_bpr) =
        readback_to_aligned_buffer(device, queue, texture, origin, extent, bytes_per_pixel);
    let slice = buffer.slice(..);
    let data = slice.get_mapped_range();
    for row in 0..extent.height as usize {
        let src_off = row * aligned_bpr as usize;
        let dst_off = row * row_bytes;
        output[dst_off..dst_off + row_bytes]
            .copy_from_slice(&data[src_off..src_off + row_bytes]);
    }
    drop(data);
    buffer.unmap();
}
