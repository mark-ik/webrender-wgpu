/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Cross-module integration smoke tests. See plan §6 S2 receipt.

use super::*;

/// S2 receipt: record a single rectangle DrawIntent, flush via
/// `pass.rs`, read back the target, assert the pixels match the palette
/// colour. End-to-end exercise of the §4.6–4.9 architectural patterns:
/// storage-buffer palette read, dynamic-offset per-draw uniform,
/// push-constant palette index, WGSL override-specialized constant,
/// `DrawIntent` recording into `pass::flush_pass` (no inline draw).
#[test]
fn render_rect_smoke() {
    let dev = core::boot().expect("wgpu boot");
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let dim = 8_u32;

    let target = dev.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("S2 smoke target"),
        size: wgpu::Extent3d {
            width: dim,
            height: dim,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let pipe = pipeline::build_brush_solid(&dev.device, format);

    // Per-draw uniform: full-clip-space rect at slot 0.
    let entry_size: u64 = 16; // vec4<f32>
    let (uniform_buffer, _stride) =
        buffer::create_uniform_arena(&dev.device, entry_size, 1);
    let rect: [f32; 4] = [-1.0, -1.0, 2.0, 2.0];
    let rect_bytes: Vec<u8> = rect.iter().flat_map(|f| f.to_ne_bytes()).collect();
    dev.queue.write_buffer(&uniform_buffer, 0, &rect_bytes);

    // Storage palette: index 0 is opaque red.
    let mut palette = vec![[0.0_f32; 4]; 16];
    palette[0] = [1.0, 0.0, 0.0, 1.0];
    let palette_bytes: Vec<u8> = palette
        .iter()
        .flat_map(|c| c.iter().flat_map(|f| f.to_ne_bytes()))
        .collect();
    let palette_buffer =
        buffer::create_storage_buffer(&dev.device, &dev.queue, "S2 palette", &palette_bytes);

    let bind_group = binding::brush_solid_bind_group(
        &dev.device,
        &pipe.layout,
        &uniform_buffer,
        entry_size,
        &palette_buffer,
    );

    // Record one DrawIntent — palette_index = 0 → red.
    let palette_index: u32 = 0;
    let draws = vec![pass::DrawIntent {
        vertex_range: 0..4,
        instance_range: 0..1,
        uniform_offset: 0,
        push_constants: palette_index.to_ne_bytes().to_vec(),
    }];

    let mut encoder = dev
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("S2 smoke encoder"),
        });
    pass::flush_pass(
        &mut encoder,
        &target_view,
        &pipe.pipeline,
        &bind_group,
        wgpu::Color::TRANSPARENT,
        "S2 smoke pass",
        &draws,
    );

    // Readback.
    let padded_bytes_per_row = (dim * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = dev.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("S2 smoke readback"),
        size: padded_bytes_per_row as u64 * dim as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d { x: 0, y: 0, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(dim),
            },
        },
        wgpu::Extent3d {
            width: dim,
            height: dim,
            depth_or_array_layers: 1,
        },
    );
    dev.queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    dev.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    rx.recv().expect("map sender").expect("map");

    let mapped = slice.get_mapped_range();
    // The full-NDC quad covers the whole target. Sample the centre row's
    // first pixel to confirm the palette colour reached the framebuffer.
    let mid_row = (dim / 2) as usize;
    let row_start = mid_row * padded_bytes_per_row as usize;
    assert_eq!(&mapped[row_start..row_start + 4], &[255, 0, 0, 255]);
}
