/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Vertex / index / uniform / storage buffer arenas; dynamic-offset
//! suballocator helpers.

/// Allocate a uniform-buffer arena big enough for `slots` per-draw
/// entries of `entry_size` bytes each, padded to the device's
/// `min_uniform_buffer_offset_alignment`. Returns the buffer plus the
/// stride (the sub-allocation step) for use with
/// `set_bind_group(offset)`.
#[allow(dead_code)] // first consumer (per-draw uniforms) lands later in renderer-body work
pub(crate) fn create_uniform_arena(
    device: &wgpu::Device,
    entry_size: u64,
    slots: u64,
) -> (wgpu::Buffer, u64) {
    let alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
    let stride = entry_size.next_multiple_of(alignment);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("uniform arena"),
        size: stride * slots,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    (buffer, stride)
}

/// Create a storage buffer initialized with `contents`. Storage-buffer
/// access replaces the GL-era data-texture pattern.
#[allow(dead_code)] // exercised by tests; renderer-body consumer lands at Phase 2
pub(crate) fn create_storage_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    contents: &[u8],
) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: contents.len() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, contents);
    buffer
}

/// Create a vertex buffer initialized with `contents`. Per-instance
/// attribute streams (e.g. `aData ivec4`) live here.
#[allow(dead_code)] // exercised by tests; renderer-body consumer lands at Phase 2
pub(crate) fn create_vertex_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    contents: &[u8],
) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: contents.len() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, contents);
    buffer
}

/// Create a single-shot uniform buffer initialized with `contents`. Use
/// this for per-frame / per-pass static uniforms. Per-draw uniform
/// sub-allocation goes through [`create_uniform_arena`].
#[allow(dead_code)] // exercised by tests; renderer-body consumer lands at Phase 2
pub(crate) fn create_uniform_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    contents: &[u8],
) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: contents.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, contents);
    buffer
}
