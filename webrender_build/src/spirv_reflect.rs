/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! SPIR-V reflection — shared between `reflect_spirv` binary and tests.
//!
//! See `notes/2026-04-30_wgpu_device_plan.md` A1 (the verification-oracle
//! role) and `bin/reflect_spirv.rs` for the binary entry point.
//!
//! ## Schema
//!
//! The emitted manifest carries enough information to construct a
//! `wgpu_hal::BindGroupLayoutEntry` (and equivalently a
//! `wgpu::BindGroupLayoutEntry`) for every reflected binding **without
//! consulting any other source**. wgpu can auto-derive layouts at runtime
//! via naga validation; wgpu-hal cannot, so the manifest must carry the
//! full layout information for the wgpu-hal backend to consume at startup.
//!
//! Visibility is **not** carried per-binding. Instead, the manifest
//! discriminates `vert` and `frag` stages explicitly and the same
//! `(group, binding)` pair may appear in both. Consumers compute
//! visibility as the union of the stages in which the binding appears.
//!
//! `multisampled` defaults to `false` and `read_only` defaults to `false`;
//! those defaults match WebRender's actual SPIR-V corpus and let older
//! manifests (or hand-written tests) omit the fields without surprise.

use naga::front::spv;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Top-level manifest: shader name -> per-stage reflection result.
/// Sorted (BTreeMap) for deterministic byte-identical output.
#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BindingsManifest {
    /// Map from artifact stem (e.g. "ps_clear", "brush_solid_ALPHA_PASS")
    /// to vert/frag stage info.
    pub shaders: BTreeMap<String, ShaderEntry>,
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShaderEntry {
    pub vert: Option<StageReflection>,
    pub frag: Option<StageReflection>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StageReflection {
    /// Bind-group resources sorted by (group, binding).
    pub bindings: Vec<BindingEntry>,
    /// Vertex input attributes (vertex stage only; empty for fragment).
    pub vertex_inputs: Vec<VertexInputEntry>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BindingEntry {
    pub name: String,
    pub group: u32,
    pub binding: u32,
    /// Resource kind plus any kind-specific layout fields. Flattened so
    /// the top-level JSON object has fields like `"kind": "sampled_texture",
    /// "sample_type": "float", "view_dimension": "d2", ...`.
    #[serde(flatten)]
    pub kind: BindingKind,
}

/// Resource kind discriminant carrying every layout field needed to
/// construct a `wgpu_hal::BindGroupLayoutEntry`. New variants must remain
/// additive — consumers should treat unknown variants as
/// `BindingKind::Other` rather than failing.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BindingKind {
    UniformBuffer,
    StorageBuffer {
        #[serde(default)]
        read_only: bool,
    },
    SampledTexture {
        sample_type: TextureSampleType,
        view_dimension: TextureViewDimension,
        #[serde(default)]
        multisampled: bool,
    },
    DepthTexture {
        view_dimension: TextureViewDimension,
        #[serde(default)]
        multisampled: bool,
    },
    StorageTexture {
        access: StorageTextureAccess,
        format: String,
        view_dimension: TextureViewDimension,
    },
    Sampler {
        binding_type: SamplerBindingType,
    },
    ExternalTexture,
    /// Anything naga reflected but we don't currently classify. Lossless
    /// fallback so the schema can grow without breaking old manifests.
    Other {
        description: String,
    },
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Copy, Clone)]
#[serde(rename_all = "snake_case")]
pub enum TextureSampleType {
    /// Floating-point texel; whether the sampler may use linear filtering
    /// is determined at sampler bind time, not in the shader IR.
    Float,
    Sint,
    Uint,
    Depth,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Copy, Clone)]
#[serde(rename_all = "snake_case")]
pub enum TextureViewDimension {
    D1,
    D2,
    D2Array,
    Cube,
    CubeArray,
    D3,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Copy, Clone)]
#[serde(rename_all = "snake_case")]
pub enum StorageTextureAccess {
    ReadOnly,
    WriteOnly,
    ReadWrite,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Copy, Clone)]
#[serde(rename_all = "snake_case")]
pub enum SamplerBindingType {
    Filtering,
    NonFiltering,
    Comparison,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VertexInputEntry {
    pub name: String,
    pub location: u32,
    /// e.g. "vec4<f32>", "vec2<u32>" — naga's scalar+vector type form.
    pub ty: String,
}

/// Reflection result for a directory of `.spv` artifacts.
pub struct ReflectResult {
    pub manifest: BindingsManifest,
    /// Files that failed to reflect, with the naga error string.
    pub errors: Vec<(String, String)>,
}

/// Walks `spirv_dir` for `*.spv` files and reflects each one. Returns the
/// assembled manifest plus any per-file errors. Naming convention:
/// `{shader_name}.{vert|frag}.spv`.
pub fn reflect_dir(spirv_dir: &Path) -> ReflectResult {
    assert!(spirv_dir.exists(), "spirv dir not found: {}", spirv_dir.display());

    let mut manifest = BindingsManifest::default();
    let mut errors: Vec<(String, String)> = Vec::new();

    let mut entries: Vec<_> = fs::read_dir(spirv_dir)
        .expect("read spirv dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("spv"))
        .collect();
    entries.sort();

    for path in &entries {
        let file_stem = path.file_name().and_then(|s| s.to_str()).expect("utf8 path");
        let without_spv = file_stem.trim_end_matches(".spv");
        let (shader_name, stage) = match without_spv.rsplit_once('.') {
            Some(parts) => parts,
            None => {
                errors.push((file_stem.to_string(), "no stage suffix".to_string()));
                continue;
            }
        };

        let bytes = fs::read(path).expect("read spv");
        match reflect_module(&bytes, stage == "vert") {
            Ok(reflection) => {
                let entry = manifest.shaders.entry(shader_name.to_string()).or_default();
                match stage {
                    "vert" => entry.vert = Some(reflection),
                    "frag" => entry.frag = Some(reflection),
                    other => errors.push((file_stem.to_string(), format!("unknown stage {}", other))),
                }
            }
            Err(e) => errors.push((file_stem.to_string(), e)),
        }
    }

    ReflectResult { manifest, errors }
}

fn reflect_module(spirv: &[u8], is_vertex_stage: bool) -> Result<StageReflection, String> {
    if spirv.len() % 4 != 0 {
        return Err(format!("spirv length not /4: {}", spirv.len()));
    }
    let module = spv::parse_u8_slice(spirv, &spv::Options::default())
        .map_err(|e| format!("naga parse: {:?}", e))?;

    let mut bindings: Vec<BindingEntry> = module
        .global_variables
        .iter()
        .filter_map(|(_handle, gv)| {
            let binding = gv.binding.as_ref()?;
            Some(BindingEntry {
                name: gv.name.clone().unwrap_or_default(),
                group: binding.group,
                binding: binding.binding,
                kind: classify_global(&module, gv),
            })
        })
        .collect();
    bindings.sort_by_key(|b| (b.group, b.binding));

    let vertex_inputs: Vec<VertexInputEntry> = if is_vertex_stage {
        module
            .entry_points
            .iter()
            .find(|ep| matches!(ep.stage, naga::ShaderStage::Vertex))
            .map(|ep| {
                let mut inputs: Vec<VertexInputEntry> = ep
                    .function
                    .arguments
                    .iter()
                    .filter_map(|arg| {
                        let binding = arg.binding.as_ref()?;
                        let location = match binding {
                            naga::Binding::Location { location, .. } => *location,
                            _ => return None,
                        };
                        Some(VertexInputEntry {
                            name: arg.name.clone().unwrap_or_default(),
                            location,
                            ty: format_type(&module, arg.ty),
                        })
                    })
                    .collect();
                inputs.sort_by_key(|v| v.location);
                inputs
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    Ok(StageReflection { bindings, vertex_inputs })
}

fn classify_global(module: &naga::Module, gv: &naga::GlobalVariable) -> BindingKind {
    let inner = &module.types[gv.ty].inner;
    match inner {
        naga::TypeInner::Image { dim, arrayed, class } => {
            let view_dimension = image_dim_to_view_dimension(*dim, *arrayed);
            match class {
                naga::ImageClass::Sampled { kind, multi } => BindingKind::SampledTexture {
                    sample_type: scalar_kind_to_sample_type(*kind),
                    view_dimension,
                    multisampled: *multi,
                },
                naga::ImageClass::Depth { multi } => BindingKind::DepthTexture {
                    view_dimension,
                    multisampled: *multi,
                },
                naga::ImageClass::Storage { format, access } => BindingKind::StorageTexture {
                    access: storage_access_to_kind(*access),
                    format: format!("{:?}", format),
                    view_dimension,
                },
                naga::ImageClass::External => BindingKind::ExternalTexture,
            }
        }
        naga::TypeInner::Sampler { comparison } => BindingKind::Sampler {
            binding_type: if *comparison {
                SamplerBindingType::Comparison
            } else {
                SamplerBindingType::Filtering
            },
        },
        naga::TypeInner::Struct { .. } => match gv.space {
            naga::AddressSpace::Uniform => BindingKind::UniformBuffer,
            naga::AddressSpace::Storage { access } => BindingKind::StorageBuffer {
                read_only: !access.contains(naga::StorageAccess::STORE),
            },
            other => BindingKind::Other {
                description: format!("struct in {:?}", other),
            },
        },
        other => BindingKind::Other {
            description: format!("{:?}", other),
        },
    }
}

fn image_dim_to_view_dimension(dim: naga::ImageDimension, arrayed: bool) -> TextureViewDimension {
    use naga::ImageDimension as D;
    match (dim, arrayed) {
        (D::D1, false) => TextureViewDimension::D1,
        (D::D1, true) => TextureViewDimension::D2Array, // GL/SPIR-V D1 arrays land as D2Array in wgpu
        (D::D2, false) => TextureViewDimension::D2,
        (D::D2, true) => TextureViewDimension::D2Array,
        (D::D3, _) => TextureViewDimension::D3,
        (D::Cube, false) => TextureViewDimension::Cube,
        (D::Cube, true) => TextureViewDimension::CubeArray,
    }
}

fn scalar_kind_to_sample_type(kind: naga::ScalarKind) -> TextureSampleType {
    match kind {
        naga::ScalarKind::Float => TextureSampleType::Float,
        naga::ScalarKind::Sint => TextureSampleType::Sint,
        naga::ScalarKind::Uint => TextureSampleType::Uint,
        // Bool / abstract scalars don't appear as image-sample types in
        // valid SPIR-V; fall back to Float for resilience.
        _ => TextureSampleType::Float,
    }
}

fn storage_access_to_kind(access: naga::StorageAccess) -> StorageTextureAccess {
    let load = access.contains(naga::StorageAccess::LOAD);
    let store = access.contains(naga::StorageAccess::STORE);
    match (load, store) {
        (true, false) => StorageTextureAccess::ReadOnly,
        (false, true) => StorageTextureAccess::WriteOnly,
        _ => StorageTextureAccess::ReadWrite,
    }
}

fn format_type(module: &naga::Module, ty: naga::Handle<naga::Type>) -> String {
    let inner = &module.types[ty].inner;
    match inner {
        naga::TypeInner::Scalar(s) => format_scalar(*s),
        naga::TypeInner::Vector { size, scalar } => {
            format!("vec{}<{}>", *size as u32, format_scalar(*scalar))
        }
        naga::TypeInner::Matrix { columns, rows, scalar } => format!(
            "mat{}x{}<{}>",
            *columns as u32,
            *rows as u32,
            format_scalar(*scalar)
        ),
        other => format!("{:?}", other),
    }
}

fn format_scalar(s: naga::Scalar) -> String {
    let kind = match s.kind {
        naga::ScalarKind::Sint => "i",
        naga::ScalarKind::Uint => "u",
        naga::ScalarKind::Float => "f",
        naga::ScalarKind::Bool => "bool",
        naga::ScalarKind::AbstractInt => "abstract_int",
        naga::ScalarKind::AbstractFloat => "abstract_float",
    };
    if s.kind == naga::ScalarKind::Bool {
        kind.to_string()
    } else {
        format!("{}{}", kind, s.width * 8)
    }
}
