/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! P5 smoke: full end-to-end render through WgpuDevice. Creates a target
//! texture, sets up ps_clear's pipeline + vertex/instance/index buffers,
//! issues one draw, reads back the pixels, verifies the clear color is
//! present at the expected location.
//!
//! This is the first test that exercises the entire chain:
//!   GpuFrame::begin_frame
//!     → GpuPass::bind_draw_target (Option II: WgpuDrawTarget carries
//!         Arc<TextureView>)
//!     → GpuPass::clear_target
//!     → GpuPass::bind_program (records pipeline + uniform_buffer)
//!     → GpuShaders::set_uniforms (writes WrLocals UBO)
//!     → GpuPass::bind_vao (records VBO/IBO/instance buffer handles)
//!     → GpuPass::draw_indexed_triangles_instanced_u16 (opens render
//!         pass with LoadOp::Clear, builds bind group, issues draw,
//!         closes pass)
//!   GpuFrame::end_frame (submits encoder)
//!   readback (copy_texture_to_buffer + map + verify)

#![cfg(feature = "wgpu_backend")]

use api::{ImageBufferKind, ImageFormat};
use api::units::DeviceIntSize;
use std::sync::Arc;
use webrender::{
    GpuFrame, GpuPass, GpuResources, GpuShaders, RenderTargetInfo, TextureFilter,
    VertexAttribute, VertexDescriptor, VertexUsageHint, WgpuDevice, WgpuDrawTarget,
    WgpuTexture,
};

fn try_create_device() -> Option<WgpuDevice> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::default(),
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("wgpu_end_to_end_smoke device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
        experimental_features: wgpu::ExperimentalFeatures::default(),
    }))
    .ok()?;
    Some(WgpuDevice::from_parts(
        Arc::new(instance),
        Arc::new(adapter),
        Arc::new(device),
        Arc::new(queue),
        None,
        None,
    ))
}

#[test]
fn ps_clear_renders_clear_color_into_texture() {
    let Some(mut device) = try_create_device() else {
        eprintln!("skip: no wgpu adapter available");
        return;
    };

    // Target texture: 16x16 BGRA8 render target.
    let target_tex: WgpuTexture = device.create_texture(
        ImageBufferKind::Texture2D,
        ImageFormat::BGRA8,
        16,
        16,
        TextureFilter::Nearest,
        Some(RenderTargetInfo { has_depth: false }),
    );

    // Build the ps_clear program (already proven by P4i smoke).
    static VERT: &[VertexAttribute] = &[VertexAttribute::quad_instance_vertex()];
    static INST: &[VertexAttribute] = &[
        VertexAttribute::f32x4("aRect"),
        VertexAttribute::f32x4("aColor"),
    ];
    static DESC: VertexDescriptor = VertexDescriptor {
        vertex_attributes: VERT,
        instance_attributes: INST,
    };
    let program = device
        .create_program_linked("ps_clear", &[], &DESC)
        .expect("ps_clear program builds");

    // VAO with a quad: 4 verts (u8x2 positions), 6 indices (two triangles),
    // 1 instance (full-screen rect, red color).
    let vao = device.create_vao(&DESC, 1);
    // Vertex positions are unorm8x2; quad covering [0,255] mapped to [0,1].
    let positions: [[u8; 2]; 4] = [[0, 0], [255, 0], [255, 255], [0, 255]];
    // Pad each to 4 bytes (vertex stride = 4 with VERTEX_ALIGNMENT). Use
    // a 4-byte struct.
    #[repr(C)]
    #[derive(Copy, Clone)]
    struct Vert {
        pos: [u8; 2],
        _pad: [u8; 2],
    }
    let verts: [Vert; 4] = [
        Vert { pos: positions[0], _pad: [0, 0] },
        Vert { pos: positions[1], _pad: [0, 0] },
        Vert { pos: positions[2], _pad: [0, 0] },
        Vert { pos: positions[3], _pad: [0, 0] },
    ];
    device.update_vao_main_vertices(&vao, &verts, VertexUsageHint::Static);
    let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];
    device.update_vao_indices(&vao, &indices, VertexUsageHint::Static);
    // Instance: aRect=full unit-NDC square + aColor=red. ps_clear's vertex
    // shader emits gl_Position = vec4(aRect.xy + aPosition*aRect.zw, 0, 1)
    // (or similar). For a full-target draw, aRect = (-1, -1, 2, 2) maps
    // aPosition [0,1]^2 to NDC [-1,1]^2.
    #[repr(C)]
    #[derive(Copy, Clone)]
    struct Inst {
        rect: [f32; 4],
        color: [f32; 4],
    }
    let instances: [Inst; 1] = [Inst {
        rect: [-1.0, -1.0, 2.0, 2.0],
        color: [1.0, 0.0, 0.0, 1.0], // red
    }];
    device.update_vao_instances(&vao, &instances, VertexUsageHint::Static, None);

    // Set uTransform to identity. Transform3D::identity() returns column-
    // major mat4 expected by GLSL/SPIR-V.
    use euclid::default::Transform3D;
    let identity: Transform3D<f32> = Transform3D::identity();
    device.set_uniforms(&program, &identity);

    // Drive a frame.
    device.begin_frame();
    device.bind_draw_target(WgpuDrawTarget::Texture {
        view: Arc::new(target_tex.texture.create_view(&wgpu::TextureViewDescriptor::default())),
        dimensions: DeviceIntSize::new(16, 16),
        with_depth: false,
    });
    // Clear to opaque green so we can distinguish "ps_clear ran" (would
    // overwrite) from "nothing drew" (green remains).
    device.clear_target(Some([0.0, 1.0, 0.0, 1.0]), None, None);
    let bound = device.bind_program(&program);
    assert!(bound, "bind_program returned false");
    device.bind_vao(&vao);
    device.draw_indexed_triangles_instanced_u16(6, 1);
    device.end_frame();

    // Readback: copy target_tex to a mappable buffer, map, sample one pixel
    // at the center.
    let wgpu_dev = device.device().clone();
    let queue = device.queue().clone();
    let aligned_bpr = ((16 * 4 + 255) / 256) * 256;
    let buffer_size = (aligned_bpr * 16) as u64;
    let readback = wgpu_dev.create_buffer(&wgpu::BufferDescriptor {
        label: Some("end-to-end readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = wgpu_dev.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("readback encoder"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target_tex.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(aligned_bpr as u32),
                rows_per_image: Some(16),
            },
        },
        wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    wgpu_dev
        .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
        .expect("poll");
    let data = slice.get_mapped_range();

    // Sample center pixel (8, 8). BGRA8 byte order: B, G, R, A.
    let row_offset = 8 * aligned_bpr;
    let pixel_offset = row_offset + 8 * 4;
    let center: [u8; 4] = [
        data[pixel_offset],
        data[pixel_offset + 1],
        data[pixel_offset + 2],
        data[pixel_offset + 3],
    ];

    drop(data);
    readback.unmap();
    device.delete_texture(target_tex);
    device.delete_program(program);
    device.delete_vao(vao);

    // The fragment shader for ps_clear simply writes oFragColor = vColor;
    // we passed instance.color = red. Center pixel should be R=255 G=0 B=0.
    // BGRA byte order: B=0, G=0, R=255, A=255.
    eprintln!("center BGRA = {:?}", center);
    assert_eq!(center[0], 0, "B channel: {:?}", center);
    assert_eq!(center[1], 0, "G channel: {:?}", center);
    assert_eq!(center[2], 255, "R channel (red expected): {:?}", center);
    assert_eq!(center[3], 255, "A channel: {:?}", center);
}
