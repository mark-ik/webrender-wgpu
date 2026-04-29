/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! BindGroupLayout + BindGroup factories. See pipeline-first migration
//! plan §6 P1, parent plan §4.6 (storage buffers, not data textures).

/// brush_solid bind group layout.
///
/// - Slot 0: PrimitiveHeader storage buffer (read-only). Mirrors GL
///   `sPrimitiveHeadersF` + `sPrimitiveHeadersI`, collapsed into a
///   single std430 struct (parent §4.6).
/// - Slot 1: GpuBuffer storage buffer (read-only). Holds brush-specific
///   `vec4<f32>` slots indexed by `header.specific_prim_address` (per
///   GL `fetch_from_gpu_buffer_1f`).
///
/// Per-frame and per-pass uniforms (viewport, device_pixel_scale, blend
/// mode hints) land in P1.2+ as the transform pipeline wires up.
pub fn brush_solid_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("brush_solid bind group layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    })
}

/// Build a brush_solid bind group from PrimitiveHeader + GpuBuffer
/// storage buffers. Both are bound as full-buffer ranges; per-draw
/// indexing happens inside the shader via `instance_index` and
/// `header.specific_prim_address`.
pub fn brush_solid_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    prim_headers: &wgpu::Buffer,
    gpu_buffer_f: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("brush_solid bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: prim_headers.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: gpu_buffer_f.as_entire_binding(),
            },
        ],
    })
}
