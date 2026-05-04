/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! P5 cluster #2 smoke: readback through GpuPass.
//!
//! Renders ps_clear into a texture, then exercises three readback paths
//! to verify they return the expected pixels:
//!   - `get_tex_image_into(&texture, format, &mut output)`
//!   - `attach_read_texture(&texture)` + `read_pixels(&img_desc)` (Vec)
//!   - `attach_read_texture(&texture)` + `read_pixels_into(rect, format, &mut)`
//!
//! The shared readback helper handles the 256-byte aligned bytes_per_row
//! requirement by allocating a staging buffer with the aligned stride
//! and compacting rows back into the (tightly packed) caller output.

#![cfg(feature = "wgpu_backend")]

use api::{ImageBufferKind, ImageDescriptor, ImageDescriptorFlags, ImageFormat};
use api::units::{DeviceIntPoint, DeviceIntSize, FramebufferIntPoint, FramebufferIntRect,
    FramebufferIntSize};
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
        label: Some("wgpu_readback_smoke device"),
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

/// Sets up + drives a render of a full-target rect into `target_tex`,
/// fragment-colored with `color` (RGBA, premultiplied not relevant here
/// since blend is off). Returns nothing; afterwards `target_tex` holds
/// `color` at every pixel.
fn render_solid(
    device: &mut WgpuDevice,
    target_tex: &WgpuTexture,
    color: [f32; 4],
) {
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

    let vao = device.create_vao(&DESC, 1);
    #[repr(C)]
    #[derive(Copy, Clone)]
    struct Vert { pos: [u8; 2], _pad: [u8; 2] }
    let verts: [Vert; 4] = [
        Vert { pos: [0, 0],     _pad: [0, 0] },
        Vert { pos: [255, 0],   _pad: [0, 0] },
        Vert { pos: [255, 255], _pad: [0, 0] },
        Vert { pos: [0, 255],   _pad: [0, 0] },
    ];
    device.update_vao_main_vertices(&vao, &verts, VertexUsageHint::Static);
    let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];
    device.update_vao_indices(&vao, &indices, VertexUsageHint::Static);
    #[repr(C)]
    #[derive(Copy, Clone)]
    struct Inst { rect: [f32; 4], color: [f32; 4] }
    let instances: [Inst; 1] = [Inst {
        rect: [-1.0, -1.0, 2.0, 2.0],
        color,
    }];
    device.update_vao_instances(&vao, &instances, VertexUsageHint::Static, None);

    use euclid::default::Transform3D;
    let identity: Transform3D<f32> = Transform3D::identity();
    device.set_uniforms(&program, &identity);

    device.begin_frame();
    device.bind_draw_target(WgpuDrawTarget::Texture {
        view: Arc::new(target_tex.texture.create_view(&wgpu::TextureViewDescriptor::default())),
        dimensions: target_tex.size,
        with_depth: false,
    });
    device.clear_target(Some([0.0, 0.0, 0.0, 1.0]), None, None);
    let bound = device.bind_program(&program);
    assert!(bound, "bind_program returned false");
    device.bind_vao(&vao);
    device.draw_indexed_triangles_instanced_u16(6, 1);
    device.end_frame();

    device.delete_program(program);
    device.delete_vao(vao);
}

/// `get_tex_image_into` end-to-end: render solid red, copy entire texture
/// into a tightly-packed Vec, verify the center pixel is red.
#[test]
fn get_tex_image_into_returns_rendered_pixels() {
    let Some(mut device) = try_create_device() else {
        eprintln!("skip: no wgpu adapter available");
        return;
    };

    let target_tex: WgpuTexture = device.create_texture(
        ImageBufferKind::Texture2D,
        ImageFormat::BGRA8,
        16, 16,
        TextureFilter::Nearest,
        Some(RenderTargetInfo { has_depth: false }),
    );
    render_solid(&mut device, &target_tex, [1.0, 0.0, 0.0, 1.0]);

    // Tightly packed: 16 * 16 * 4 = 1024 bytes.
    let mut output = vec![0u8; 16 * 16 * 4];
    device.get_tex_image_into(&target_tex, ImageFormat::BGRA8, &mut output);

    // Center pixel (8, 8) — BGRA byte order. Expect (B=0, G=0, R=255, A=255).
    let row_bpr = 16 * 4;
    let center = 8 * row_bpr + 8 * 4;
    assert_eq!(output[center], 0,     "B: {:?}", &output[center..center+4]);
    assert_eq!(output[center + 1], 0, "G: {:?}", &output[center..center+4]);
    assert_eq!(output[center + 2], 255, "R: {:?}", &output[center..center+4]);
    assert_eq!(output[center + 3], 255, "A: {:?}", &output[center..center+4]);

    device.delete_texture(target_tex);
}

/// `read_pixels` (Vec returning) via `attach_read_texture`. Verifies the
/// attach-then-read flow and that the returned Vec has the right size.
#[test]
fn read_pixels_after_attach_read_texture_returns_correct_vec() {
    let Some(mut device) = try_create_device() else {
        eprintln!("skip: no wgpu adapter available");
        return;
    };

    let target_tex: WgpuTexture = device.create_texture(
        ImageBufferKind::Texture2D,
        ImageFormat::BGRA8,
        16, 16,
        TextureFilter::Nearest,
        Some(RenderTargetInfo { has_depth: false }),
    );
    // Solid green this time so we can distinguish from the get_tex_image
    // test's red.
    render_solid(&mut device, &target_tex, [0.0, 1.0, 0.0, 1.0]);

    device.attach_read_texture(&target_tex);
    let img_desc = ImageDescriptor {
        format: ImageFormat::BGRA8,
        size: DeviceIntSize::new(16, 16),
        stride: None,
        offset: 0,
        flags: ImageDescriptorFlags::empty(),
    };
    let pixels = device.read_pixels(&img_desc);
    assert_eq!(pixels.len(), 16 * 16 * 4, "Vec should be tightly packed");
    let row_bpr = 16 * 4;
    let center = 8 * row_bpr + 8 * 4;
    // Green: B=0, G=255, R=0, A=255 in BGRA byte order.
    assert_eq!(pixels[center], 0,       "B: {:?}", &pixels[center..center+4]);
    assert_eq!(pixels[center + 1], 255, "G: {:?}", &pixels[center..center+4]);
    assert_eq!(pixels[center + 2], 0,   "R: {:?}", &pixels[center..center+4]);
    assert_eq!(pixels[center + 3], 255, "A: {:?}", &pixels[center..center+4]);

    device.delete_texture(target_tex);
}

/// `read_pixels_into` with a sub-rect. Reads only the bottom-right 8x8
/// quadrant; verifies the rect-cropping path through readback works
/// correctly.
#[test]
fn read_pixels_into_with_subrect_returns_correct_slice() {
    let Some(mut device) = try_create_device() else {
        eprintln!("skip: no wgpu adapter available");
        return;
    };

    let target_tex: WgpuTexture = device.create_texture(
        ImageBufferKind::Texture2D,
        ImageFormat::BGRA8,
        16, 16,
        TextureFilter::Nearest,
        Some(RenderTargetInfo { has_depth: false }),
    );
    // Solid blue: B=255, G=0, R=0, A=255 in BGRA byte order.
    render_solid(&mut device, &target_tex, [0.0, 0.0, 1.0, 1.0]);

    device.attach_read_texture(&target_tex);
    // Bottom-right 8x8 quadrant: x=[8..16), y=[8..16).
    let rect = FramebufferIntRect::from_origin_and_size(
        FramebufferIntPoint::new(8, 8),
        FramebufferIntSize::new(8, 8),
    );
    let mut output = vec![0u8; 8 * 8 * 4];
    device.read_pixels_into(rect, ImageFormat::BGRA8, &mut output);

    // Every pixel in the cropped output should be blue.
    for chunk in output.chunks_exact(4) {
        assert_eq!(chunk, &[255, 0, 0, 255], "expected BGRA blue, got {:?}", chunk);
    }

    device.delete_texture(target_tex);
}

/// Edge case: `get_tex_image_into` on an output slice that's too small
/// must not panic — it should log + leave the slice untouched. (The
/// helper warns; output stays at the initial fill value.)
#[test]
fn get_tex_image_into_too_small_output_is_safe() {
    let Some(mut device) = try_create_device() else {
        eprintln!("skip: no wgpu adapter available");
        return;
    };
    let target_tex: WgpuTexture = device.create_texture(
        ImageBufferKind::Texture2D,
        ImageFormat::BGRA8,
        16, 16,
        TextureFilter::Nearest,
        Some(RenderTargetInfo { has_depth: false }),
    );
    render_solid(&mut device, &target_tex, [1.0, 0.0, 0.0, 1.0]);

    // Way too small: only 16 bytes for a 16x16 BGRA8 (needs 1024).
    let mut output = vec![0xAAu8; 16];
    device.get_tex_image_into(&target_tex, ImageFormat::BGRA8, &mut output);
    // Slice should be unchanged because the helper bails out early.
    assert!(output.iter().all(|&b| b == 0xAA), "output should be untouched");

    device.delete_texture(target_tex);
}

// Unused helper kept for parity with other smoke tests; silences the
// import warning by referencing the type even when not constructed.
#[allow(dead_code)]
fn _unused_silencer() -> Option<DeviceIntPoint> { None }
