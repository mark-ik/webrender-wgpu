/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! `GpuShaders` impl for `WgpuDevice` (P4i).
//!
//! Loads committed SPIR-V from `webrender/res/spirv/{stem}.{stage}.spv`,
//! creates wgpu `ShaderModule`s, builds a `RenderPipeline` via the
//! P3 vertex schema adapter and `layout: None` (lets wgpu's internal
//! naga auto-derive the bind-group layout from SPIR-V reflection —
//! works for the 97/125 reflectable stages after the P2-spike fixes).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::super::traits::GpuShaders;
use super::super::types::{ShaderError, TextureSlot, VertexDescriptor};
use super::types::{PipelineVariantKey, WgpuProgram, WgpuUniformLocation};
use super::vertex_layout::WgpuVertexLayouts;
use super::WgpuDevice;

/// Maps a `BlendMode` to the wgpu `BlendState` (separate color + alpha
/// components). Currently covers the common modes; extends additively
/// as more shaders surface that need them.
pub(super) fn blend_state_for(mode: super::super::traits::BlendMode) -> wgpu::BlendState {
    use super::super::traits::BlendMode as BM;
    use wgpu::{BlendComponent, BlendFactor, BlendOperation, BlendState};
    match mode {
        BM::Alpha => BlendState {
            color: BlendComponent {
                src_factor: BlendFactor::SrcAlpha,
                dst_factor: BlendFactor::OneMinusSrcAlpha,
                operation: BlendOperation::Add,
            },
            alpha: BlendComponent {
                src_factor: BlendFactor::One,
                dst_factor: BlendFactor::OneMinusSrcAlpha,
                operation: BlendOperation::Add,
            },
        },
        BM::PremultipliedAlpha => BlendState {
            color: BlendComponent {
                src_factor: BlendFactor::One,
                dst_factor: BlendFactor::OneMinusSrcAlpha,
                operation: BlendOperation::Add,
            },
            alpha: BlendComponent {
                src_factor: BlendFactor::One,
                dst_factor: BlendFactor::OneMinusSrcAlpha,
                operation: BlendOperation::Add,
            },
        },
        BM::Screen => BlendState {
            color: BlendComponent {
                src_factor: BlendFactor::One,
                dst_factor: BlendFactor::OneMinusSrc,
                operation: BlendOperation::Add,
            },
            alpha: BlendComponent::OVER,
        },
        BM::Multiply => BlendState {
            color: BlendComponent {
                src_factor: BlendFactor::Dst,
                dst_factor: BlendFactor::Zero,
                operation: BlendOperation::Add,
            },
            alpha: BlendComponent::OVER,
        },
        // Other modes (subpixel, dual-source, advanced, etc.) require
        // wgpu features (DUAL_SOURCE_BLENDING) and/or have no direct
        // wgpu equivalent. Fall back to PremultipliedAlpha — wrong
        // visually but keeps the pipeline buildable until per-mode work
        // lands. Logged at warn level for visibility.
        _ => {
            log::warn!("blend_state_for: mode {:?} not yet mapped, using PremultipliedAlpha", mode);
            BlendState::PREMULTIPLIED_ALPHA_BLENDING
        }
    }
}

/// Translates a `PipelineVariantKey::color_write_mask` (4-bit RGBA) to
/// `wgpu::ColorWrites` flags.
pub(super) fn color_writes_from_mask(mask: u8) -> wgpu::ColorWrites {
    let mut w = wgpu::ColorWrites::empty();
    if mask & 0x1 != 0 { w |= wgpu::ColorWrites::RED; }
    if mask & 0x2 != 0 { w |= wgpu::ColorWrites::GREEN; }
    if mask & 0x4 != 0 { w |= wgpu::ColorWrites::BLUE; }
    if mask & 0x8 != 0 { w |= wgpu::ColorWrites::ALPHA; }
    w
}

/// Builds a `wgpu::RenderPipeline` for a given program + variant key.
/// Used by both `link_program` (eager DEFAULT build) and the draw-time
/// cache-miss path (`pipeline_for` in mod.rs).
pub(super) fn build_pipeline_variant(
    device: &wgpu::Device,
    program: &WgpuProgram,
    descriptor: &VertexDescriptor,
    key: PipelineVariantKey,
) -> wgpu::RenderPipeline {
    let layouts = WgpuVertexLayouts::from_descriptor(descriptor);
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
            program.stem, key.blend, key.color_write_mask,
        )),
        layout: None,
        vertex: wgpu::VertexState {
            module: &program.vert_module,
            entry_point: Some("main"),
            buffers: &nonempty,
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &program.frag_module,
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

/// Builds the SPIR-V artifact stem from a base shader name + features.
/// Mirrors `gen_spirv.rs`'s naming exactly: features sorted, then
/// underscore-joined onto the base. Empty feature list = bare base name.
pub(super) fn shader_stem(base: &str, features: &[&str]) -> String {
    if features.is_empty() {
        base.to_string()
    } else {
        let mut sorted: Vec<&str> = features.to_vec();
        sorted.sort_unstable();
        format!("{}_{}", base, sorted.join("_"))
    }
}

// build.rs walks res/spirv/*.spv and emits this lookup module with each
// blob baked in via include_bytes!. No runtime fs needed for the
// committed corpus.
mod spirv_blobs {
    include!(concat!(env!("OUT_DIR"), "/spirv_blobs.rs"));
}

/// Returns the SPIR-V bytes for `(stem, stage)`. First tries the build.rs
/// generated lookup table (committed corpus, baked in via include_bytes!);
/// falls back to fs::read for shaders not in the corpus (e.g. test
/// fixtures generated separately). Returns `Err(NotFound)` if neither
/// source has the artifact.
pub(super) fn load_committed_spv(stem: &str, stage: &str) -> Result<Vec<u8>, std::io::Error> {
    if let Some(bytes) = spirv_blobs::lookup(stem, stage) {
        return Ok(bytes.to_vec());
    }
    let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("res");
    path.push("spirv");
    path.push(format!("{}.{}.spv", stem, stage));
    std::fs::read(path)
}

impl GpuShaders for WgpuDevice {
    type Program = WgpuProgram;
    type UniformLocation = WgpuUniformLocation;

    fn create_program(
        &mut self,
        base_filename: &'static str,
        features: &[&'static str],
    ) -> Result<Self::Program, ShaderError> {
        let stem = shader_stem(base_filename, features);
        let vert_bytes = load_committed_spv(&stem, "vert").map_err(|e| {
            ShaderError::Compilation(stem.clone(), format!("load vert: {}", e))
        })?;
        let frag_bytes = load_committed_spv(&stem, "frag").map_err(|e| {
            ShaderError::Compilation(stem.clone(), format!("load frag: {}", e))
        })?;

        let vert_module =
            self.create_shader_module_from_spv(Some(&format!("{}.vert", stem)), &vert_bytes);
        let frag_module =
            self.create_shader_module_from_spv(Some(&format!("{}.frag", stem)), &frag_bytes);

        // WrLocals UBO is mat4 (64 bytes). Sized for that even when the
        // particular shader doesn't sample uTransform — wasted 64 bytes
        // per program is negligible.
        let uniform_buffer = self.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("WgpuProgram[{}] uTransform UBO", stem)),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(WgpuProgram {
            vert_module,
            frag_module,
            pipelines: Rc::new(RefCell::new(HashMap::new())),
            uniform_buffer,
            stem,
            // Placeholder descriptor — link_program overwrites with the
            // real one when called. create_program-then-use-without-link
            // is unsupported in WebRender's flow (renderer always links
            // before draw).
            descriptor: VertexDescriptor {
                vertex_attributes: &[],
                instance_attributes: &[],
            },
        })
    }

    fn create_program_linked(
        &mut self,
        base_filename: &'static str,
        features: &[&'static str],
        descriptor: &VertexDescriptor,
    ) -> Result<Self::Program, ShaderError> {
        let mut program = self.create_program(base_filename, features)?;
        self.link_program(&mut program, descriptor)?;
        Ok(program)
    }

    fn link_program(
        &mut self,
        program: &mut Self::Program,
        descriptor: &VertexDescriptor,
    ) -> Result<(), ShaderError> {
        // Stash the descriptor on the program so future variant builds
        // can use it.
        program.descriptor = VertexDescriptor {
            vertex_attributes: descriptor.vertex_attributes,
            instance_attributes: descriptor.instance_attributes,
        };
        // Eagerly build the DEFAULT-state pipeline (no blend, full color
        // write). Variants are built lazily at draw time.
        let pipeline = build_pipeline_variant(
            self.device(),
            program,
            descriptor,
            PipelineVariantKey::DEFAULT,
        );
        program
            .pipelines
            .borrow_mut()
            .insert(PipelineVariantKey::DEFAULT, pipeline);
        Ok(())
    }

    fn delete_program(&mut self, _program: Self::Program) {
        // Drop releases the modules + pipeline + uniform buffer.
    }

    fn get_uniform_location(
        &self,
        _program: &Self::Program,
        _name: &str,
    ) -> Self::UniformLocation {
        // wgpu has no name-based uniform location lookup; uniforms are
        // bound by group/binding index. Return a placeholder; callers
        // that store the result and pass it back to set_uniforms get
        // the WrLocals UBO write regardless of the name.
        WgpuUniformLocation
    }

    fn bind_shader_samplers<S>(
        &mut self,
        _program: &Self::Program,
        _bindings: &[(&'static str, S)],
    )
    where
        S: Into<TextureSlot> + Copy,
    {
        // GL: sets sampler uniform values at named locations to texture
        // unit indices. In wgpu, sampler-to-binding mapping is baked
        // into the pipeline layout (auto-derived from SPIR-V). The
        // renderer's BindGroup construction picks up the correct
        // bindings by index at draw time. No-op here.
    }
}
