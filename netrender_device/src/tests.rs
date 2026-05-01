/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Cross-module device-side smoke tests. Scene-level golden tests
//! (oracle PNG/YAML pairs) live in `netrender/tests/`; they're
//! renderer-side per design plan §5 Phase 0.5.

use crate::{adapter, binding, buffer, core, pass, pipeline, texture};

/// Render one solid quad through the production-shape brush_solid
/// pipeline. Inputs flow through two storage buffers that mirror
/// `prim_shared.glsl::PrimitiveHeader` and the gpu_buffer's
/// brush-specific `vec4` slot. The primitive ABI here is the GL-era
/// contract preserved as a smoke (axiom 12); Phase 2 re-decides it
/// once the batch builder lands.
#[test]
fn render_rect_smoke() {
    let dev_adapter = adapter::WgpuDevice::boot().expect("wgpu boot");
    let dev = &dev_adapter.core;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let dim = 8_u32;

    let target = dev.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("P1.1 smoke target"),
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

    let pipe = pipeline::build_brush_solid_specialized(&dev.device, format, /* alpha_pass */ false);

    // PrimitiveHeader storage: one entry. `local_rect` covers full clip
    // space (-1..1); `specific_prim_address` points at gpu_buffer slot 0
    // where the colour lives. The other fields are placeholders.
    //
    // Layout (std430, 64 bytes; matches the WGSL struct in
    // `shaders/brush_solid.wgsl`):
    //   local_rect            vec4<f32>
    //   local_clip_rect       vec4<f32>
    //   z, specific_prim_address, transform_id, picture_task_address
    //                         4 × i32
    //   user_data             vec4<i32>
    let mut header_bytes: Vec<u8> = Vec::with_capacity(64);
    for f in [-1.0_f32, -1.0, 1.0, 1.0] {
        header_bytes.extend_from_slice(&f.to_ne_bytes());
    }
    for f in [-1.0_f32, -1.0, 1.0, 1.0] {
        header_bytes.extend_from_slice(&f.to_ne_bytes());
    }
    // z, specific_prim_address, transform_id, picture_task_address
    for i in [0_i32, 0, 0, 0] {
        header_bytes.extend_from_slice(&i.to_ne_bytes());
    }
    for i in [0_i32; 4] {
        header_bytes.extend_from_slice(&i.to_ne_bytes());
    }
    let prim_headers =
        buffer::create_storage_buffer(&dev.device, &dev.queue, "P1 prim_headers", &header_bytes);

    // Transform storage: identity matrix for both `m` and `inv_m`.
    // 128 bytes std430 — see WGSL `Transform { m, inv_m }`.
    let identity: [f32; 16] = [
        1.0, 0.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, 1.0, 0.0,
        0.0, 0.0, 0.0, 1.0,
    ];
    let transform_bytes: Vec<u8> = identity
        .iter()
        .chain(identity.iter())
        .flat_map(|f| f.to_ne_bytes())
        .collect();
    let transforms =
        buffer::create_storage_buffer(&dev.device, &dev.queue, "P1 transforms", &transform_bytes);

    // GpuBuffer storage: slot 0 holds opaque red as a vec4<f32>.
    let color: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    let gpu_buffer_bytes: Vec<u8> = color.iter().flat_map(|f| f.to_ne_bytes()).collect();
    let gpu_buffer_f =
        buffer::create_storage_buffer(&dev.device, &dev.queue, "P1 gpu_buffer_f", &gpu_buffer_bytes);

    // RenderTaskData storage: one entry, identity-equivalent.
    //   task_rect = (0, 0, 0, 0): final_offset cancels with content_origin.
    //   user_data = (1.0, 0.0, 0.0, 0.0): device_pixel_scale=1, content_origin=(0,0).
    // 32 bytes std430 — see WGSL `RenderTaskData { task_rect, user_data }`.
    let render_task: [f32; 8] = [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
    let render_task_bytes: Vec<u8> = render_task.iter().flat_map(|f| f.to_ne_bytes()).collect();
    let render_tasks =
        buffer::create_storage_buffer(&dev.device, &dev.queue, "P1 render_tasks", &render_task_bytes);

    // PerFrame uniform: identity orthographic projection. Combined with
    // a clip-space-shaped local_rect (-1..1) and identity transform,
    // the full GL vertex math collapses cleanly.
    let identity_proj: [f32; 16] = [
        1.0, 0.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, 1.0, 0.0,
        0.0, 0.0, 0.0, 1.0,
    ];
    let per_frame_bytes: Vec<u8> = identity_proj.iter().flat_map(|f| f.to_ne_bytes()).collect();
    let per_frame =
        buffer::create_uniform_buffer(&dev.device, &dev.queue, "P1 per_frame", &per_frame_bytes);

    // Dummy 1×1 R8 clip mask. Layout requires the binding for both
    // pipelines, but the opaque (ALPHA_PASS=false) shader doesn't read
    // it; clip_address points at render_tasks[0] whose dummy zero
    // task_rect would also short-circuit `do_clip` even if it ran.
    let clip_mask = dev.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("P1.5 dummy clip mask"),
        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let clip_mask_view = clip_mask.create_view(&wgpu::TextureViewDescriptor::default());

    let bind_group = binding::brush_solid_bind_group(
        &dev.device,
        &pipe.layout,
        &prim_headers,
        &transforms,
        &gpu_buffer_f,
        &render_tasks,
        &per_frame,
        &clip_mask_view,
    );

    // Per-instance `a_data` vertex buffer. One ivec4 per primitive
    // (16 bytes). Field decoding matches GL `decode_instance_attributes`:
    //   x = prim_header_address (we point at PrimitiveHeader[0])
    //   y = clip_address (we point at render_tasks[0]: dummy bounds)
    //   z = (flags << 16) | segment_index (unused yet)
    //   w = (brush_kind << 24) | resource_address (unused — brush_solid
    //       has no resource address)
    let a_data: [i32; 4] = [0, 0, 0, 0];
    let a_data_bytes: Vec<u8> = a_data.iter().flat_map(|i| i.to_ne_bytes()).collect();
    let a_data_buffer =
        buffer::create_vertex_buffer(&dev.device, &dev.queue, "P1 a_data", &a_data_bytes);

    let draws = vec![pass::DrawIntent {
        pipeline: pipe.pipeline.clone(),
        bind_group: bind_group.clone(),
        vertex_buffers: vec![a_data_buffer],
        vertex_range: 0..4,
        instance_range: 0..1,
        dynamic_offsets: Vec::new(),
        push_constants: Vec::new(),
    }];

    let mut encoder = dev_adapter.create_encoder("P1.1 smoke encoder");
    dev_adapter.encode_pass(
        &mut encoder,
        pass::RenderPassTarget {
            label: "P1.1 smoke pass",
            color: pass::ColorAttachment::clear(&target_view, wgpu::Color::TRANSPARENT),
            depth: None,
        },
        &draws,
    );
    dev_adapter.submit(encoder);

    // The full-NDC quad covers the whole target. Centre row's first
    // pixel confirms the storage-buffer fetch path delivers the colour.
    let actual_rgba = dev_adapter.read_rgba8_texture(&target, dim, dim);
    let mid_row = (dim / 2) as usize;
    let row_start = mid_row * dim as usize * 4;
    assert_eq!(&actual_rgba[row_start..row_start + 4], &[255, 0, 0, 255]);
}

/// Alpha-pass override variant (`ALPHA_PASS=true`) builds a second
/// pipeline; vertex shader writes clip varyings; fragment shader runs
/// `do_clip` and multiplies the colour by the clip-mask sample. Smoke
/// uses an 8×8 R8 clip mask filled with `1.0` and a ClipArea entry
/// whose bounds cover the rendered quad, so every fragment passes the
/// bounds check, samples `1.0`, and the alpha-pass output matches the
/// opaque output (both red). Validates pipeline compilation + the
/// textureLoad + bounds-check path end-to-end.
#[test]
fn render_rect_alpha_smoke() {
    let dev_adapter = adapter::WgpuDevice::boot().expect("wgpu boot");
    let dev = &dev_adapter.core;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let dim = 8_u32;

    let target = dev.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("P1.5 alpha smoke target"),
        size: wgpu::Extent3d { width: dim, height: dim, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let pipe = pipeline::build_brush_solid_specialized(&dev.device, format, /* alpha_pass */ true);

    // PrimitiveHeader[0]: full-NDC local_rect, clip_address-pointed
    // task at index 1 (clip area), specific_prim_address = 0 (red).
    let mut header_bytes: Vec<u8> = Vec::with_capacity(64);
    for f in [-1.0_f32, -1.0, 1.0, 1.0] {
        header_bytes.extend_from_slice(&f.to_ne_bytes());
    }
    for f in [-1.0_f32, -1.0, 1.0, 1.0] {
        header_bytes.extend_from_slice(&f.to_ne_bytes());
    }
    for i in [0_i32, 0, 0, 0] {
        header_bytes.extend_from_slice(&i.to_ne_bytes());
    }
    for i in [0_i32; 4] {
        header_bytes.extend_from_slice(&i.to_ne_bytes());
    }
    let prim_headers = buffer::create_storage_buffer(
        &dev.device, &dev.queue, "P1.5 prim_headers", &header_bytes,
    );

    // Identity transform.
    let identity: [f32; 16] = [
        1.0, 0.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, 1.0, 0.0,
        0.0, 0.0, 0.0, 1.0,
    ];
    let transform_bytes: Vec<u8> = identity
        .iter()
        .chain(identity.iter())
        .flat_map(|f| f.to_ne_bytes())
        .collect();
    let transforms = buffer::create_storage_buffer(
        &dev.device, &dev.queue, "P1.5 transforms", &transform_bytes,
    );

    // GpuBuffer[0] = opaque red.
    let color: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    let gpu_buffer_bytes: Vec<u8> = color.iter().flat_map(|f| f.to_ne_bytes()).collect();
    let gpu_buffer_f = buffer::create_storage_buffer(
        &dev.device, &dev.queue, "P1.5 gpu_buffer_f", &gpu_buffer_bytes,
    );

    // Two render-task entries: [0] picture task (used by header),
    // [1] clip area (pointed at by a_data.y = 1).
    //
    // Picture task: identity-equivalent (zero offsets, scale=1).
    // Clip area: task_rect = (0, 0, 8, 8) in mask space;
    //   user_data.x = device_pixel_scale = 4 (clip-space [-1,1] → mask [-4,4]+offset);
    //   user_data.yz = screen_origin = (-4, -4) so the offset shifts mask_uv to [0, 8).
    // For local_pos in [-1, 1] and identity transform / proj,
    //   clip_uv = local_pos * 4 + ((0,0) - (-4,-4)) = local_pos * 4 + (4, 4)
    //   ∈ [(0, 0), (8, 8)] — bounds-check passes for all interior fragments.
    let render_task_data: [f32; 16] = [
        // [0] picture task
        0.0, 0.0, 0.0, 0.0,
        1.0, 0.0, 0.0, 0.0,
        // [1] clip area
        0.0, 0.0, 8.0, 8.0,
        4.0, -4.0, -4.0, 0.0,
    ];
    let render_task_bytes: Vec<u8> =
        render_task_data.iter().flat_map(|f| f.to_ne_bytes()).collect();
    let render_tasks = buffer::create_storage_buffer(
        &dev.device, &dev.queue, "P1.5 render_tasks", &render_task_bytes,
    );

    // Identity orthographic projection.
    let per_frame_bytes: Vec<u8> = identity.iter().flat_map(|f| f.to_ne_bytes()).collect();
    let per_frame = buffer::create_uniform_buffer(
        &dev.device, &dev.queue, "P1.5 per_frame", &per_frame_bytes,
    );

    // 8×8 R8Unorm clip mask filled with 0xFF (= 1.0). Every
    // fragment's textureLoad returns 1.0; multiplying by the colour
    // preserves red.
    let mask_size = 8_u32;
    let clip_mask = dev.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("P1.5 clip mask"),
        size: wgpu::Extent3d {
            width: mask_size,
            height: mask_size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mask_data = vec![0xFF_u8; (mask_size * mask_size) as usize];
    dev.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &clip_mask,
            mip_level: 0,
            origin: wgpu::Origin3d { x: 0, y: 0, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        &mask_data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(mask_size),
            rows_per_image: Some(mask_size),
        },
        wgpu::Extent3d {
            width: mask_size,
            height: mask_size,
            depth_or_array_layers: 1,
        },
    );
    let clip_mask_view = clip_mask.create_view(&wgpu::TextureViewDescriptor::default());

    let bind_group = binding::brush_solid_bind_group(
        &dev.device,
        &pipe.layout,
        &prim_headers,
        &transforms,
        &gpu_buffer_f,
        &render_tasks,
        &per_frame,
        &clip_mask_view,
    );

    // a_data.y = 1: clip_address points at render_tasks[1].
    let a_data: [i32; 4] = [0, 1, 0, 0];
    let a_data_bytes: Vec<u8> = a_data.iter().flat_map(|i| i.to_ne_bytes()).collect();
    let a_data_buffer = buffer::create_vertex_buffer(
        &dev.device, &dev.queue, "P1.5 a_data", &a_data_bytes,
    );

    let draws = vec![pass::DrawIntent {
        pipeline: pipe.pipeline.clone(),
        bind_group: bind_group.clone(),
        vertex_buffers: vec![a_data_buffer],
        vertex_range: 0..4,
        instance_range: 0..1,
        dynamic_offsets: Vec::new(),
        push_constants: Vec::new(),
    }];

    let mut encoder = dev_adapter.create_encoder("P1.5 alpha smoke encoder");
    dev_adapter.encode_pass(
        &mut encoder,
        pass::RenderPassTarget {
            label: "P1.5 alpha smoke pass",
            color: pass::ColorAttachment::clear(&target_view, wgpu::Color::TRANSPARENT),
            depth: None,
        },
        &draws,
    );
    dev_adapter.submit(encoder);

    let actual_rgba = dev_adapter.read_rgba8_texture(&target, dim, dim);
    let mid_row = (dim / 2) as usize;
    let row_start = mid_row * dim as usize * 4;
    // Clip mask = 1.0 everywhere → red survives the multiply.
    assert_eq!(&actual_rgba[row_start..row_start + 4], &[255, 0, 0, 255]);
}

/// `WgpuDevice::boot()` succeeds, the lazy `ensure_<family>` cache
/// pattern works for both repeated and distinct format keys, and
/// `REQUIRED_FEATURES` is empty post-Phase-0.5 demote (axiom 10
/// receipt: baseline adapters can boot without optional features).
#[test]
fn wgpu_device_a1_smoke() {
    // Phase 0.5 receipt: no optional features required at boot.
    assert_eq!(core::REQUIRED_FEATURES, wgpu::Features::empty());

    let dev = adapter::WgpuDevice::boot().expect("WgpuDevice boot");
    let _ = dev.ensure_brush_solid(wgpu::TextureFormat::Rgba8Unorm, false);
    let _ = dev.ensure_brush_solid(wgpu::TextureFormat::Rgba8Unorm, false);
    let _ = dev.ensure_brush_solid(wgpu::TextureFormat::Bgra8Unorm, false);
    let _ = dev.ensure_brush_solid(wgpu::TextureFormat::Rgba8Unorm, true);
}

/// `WgpuDevice::create_texture` works in isolation; produces a
/// `WgpuTexture` that can hand out a default view.
#[test]
fn wgpu_device_a2_create_texture_smoke() {
    let dev = adapter::WgpuDevice::boot().expect("WgpuDevice boot");
    let tex = dev.create_texture(&texture::TextureDesc {
        label: "A2 smoke",
        width: 16,
        height: 16,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
    });
    assert_eq!((tex.width, tex.height), (16, 16));
    assert_eq!(tex.format, wgpu::TextureFormat::Rgba8Unorm);
    let _view = tex.create_view();
}

/// Dither-shaped texture (8×8 R8) gets created and uploaded via
/// `WgpuDevice::create_texture` + `upload_texture`. Receipt for the
/// texture API surface that the dither migration will use once the
/// per-pass encoding is in place to handle the bind sites.
#[test]
fn wgpu_device_a21_dither_create_upload_smoke() {
    let dev = adapter::WgpuDevice::boot().expect("WgpuDevice boot");
    let tex = dev.create_texture(&texture::TextureDesc {
        label: "dither_matrix (A2.1 prep)",
        width: 8,
        height: 8,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
    });
    // Synthetic 8×8 dither pattern; just exercises upload.
    let data: Vec<u8> = (0..64).collect();
    dev.upload_texture(&tex, &data);
    // Force a flush so the upload is observable.
    dev.core
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
}

/// Pass targets carry depth load/store policy alongside colour. This
/// is the wgpu-native landing spot for renderer callsites that
/// previously paired `clear_target(..., Some(depth), ...)` with
/// `invalidate_depth_target()`.
#[test]
fn pass_target_depth_smoke() {
    let dev_adapter = adapter::WgpuDevice::boot().expect("wgpu boot");
    let dev = &dev_adapter.core;

    let color = dev.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("A2.X.2 color target"),
        size: wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth = dev.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("A2.X.2 depth target"),
        size: wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = dev_adapter.create_encoder("A2.X.2 depth encoder");
    dev_adapter.encode_pass(
        &mut encoder,
        pass::RenderPassTarget {
            label: "A2.X.2 depth pass",
            color: pass::ColorAttachment::clear(&color_view, wgpu::Color::TRANSPARENT),
            depth: Some(pass::DepthAttachment::clear(&depth_view, 1.0).discard()),
        },
        &[],
    );
    dev_adapter.submit(encoder);

    let actual_rgba = dev_adapter.read_rgba8_texture(&color, 4, 4);
    assert_eq!(actual_rgba, vec![0; 4 * 4 * 4]);
}
