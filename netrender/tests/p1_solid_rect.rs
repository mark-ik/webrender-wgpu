/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Phase 1 internal smoke receipt — surface ↔ skeleton handshake.
//!
//! Plan reference:
//! [`netrender-notes/2026-04-30_netrender_design_plan.md`](../../netrender-notes/2026-04-30_netrender_design_plan.md) §5 Phase 1.
//!
//! Builds a [`PreparedFrame`] containing one full-NDC red brush_solid
//! draw, hands it (with a caller-supplied [`FrameTarget`]) to
//! [`Renderer::render`], reads the target back, and asserts every
//! pixel is opaque red. Receipt: the target is a 256×256 offscreen
//! `Rgba8UnormSrgb` `wgpu::Texture` — no swapchain, no
//! `wgpu::Surface`, headless on Lavapipe / WARP / SwiftShader.
//!
//! The caller-supplied target is the architectural shape the
//! embedder hookup uses (Phase 1 second receipt): the embedder
//! acquires a `wgpu::SurfaceTexture` and hands its `TextureView` to
//! the same `Renderer::render` entry point. This test stands in for
//! that hookup with an offscreen texture.
//!
//! The bind-group / input-buffer construction below mirrors
//! `netrender_device::tests::render_rect_smoke` because Phase 1 has
//! no batch builder — Phase 2's `BatchBuilder` obsoletes this
//! boilerplate. Per-allocation export class (axiom 14): the target
//! texture here is `internal-only` (offscreen receipt); the
//! embedder hookup variant treats the surface texture as
//! `compositor-exportable`.

// Phase 5: render() now always attaches depth; p1 uses brush_solid (no depth
// stencil state), so we bypass render() and call encode_pass directly.
use netrender::{
    ColorAttachment, DrawIntent, NetrenderOptions, RenderPassTarget, boot,
    create_netrender_instance,
};

const DIM: u32 = 256;
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

#[test]
fn p1_solid_rect_full_extent_red() {
    let handles = boot().expect("wgpu boot");
    let device = handles.device.clone();
    let queue = handles.queue.clone();

    let renderer = create_netrender_instance(handles, NetrenderOptions::default())
        .expect("create_netrender_instance");

    // Caller-supplied target. RENDER_ATTACHMENT for the dispatch +
    // COPY_SRC for the readback. Real embedders wouldn't use
    // COPY_SRC on a swapchain texture; we add it here only so the
    // headless receipt can read back via `read_rgba8_texture`.
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("P1 solid-rect target"),
        size: wgpu::Extent3d {
            width: DIM,
            height: DIM,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TARGET_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let pipe = renderer.wgpu_device.ensure_brush_solid(TARGET_FORMAT, /* alpha_pass */ false);

    let draw = build_full_ndc_red_draw(&device, &queue, &pipe);

    // Phase 5: render() always attaches depth; brush_solid has no depth stencil
    // state and is incompatible with a depth-attached pass. Use the lower-level
    // encode_pass with no depth attachment instead.
    let color = ColorAttachment::clear(&target_view, wgpu::Color::TRANSPARENT);
    let mut encoder = renderer.wgpu_device.create_encoder("P1 test");
    renderer.wgpu_device.encode_pass(
        &mut encoder,
        RenderPassTarget { label: "P1 test pass", color, depth: None },
        &[draw],
    );
    renderer.wgpu_device.submit(encoder);

    let actual = renderer.wgpu_device.read_rgba8_texture(&target, DIM, DIM);

    // Expected: every pixel is opaque red. Rgba8UnormSrgb encoding
    // of (1.0, 0.0, 0.0, 1.0) → bytes (255, 0, 0, 255) — sRGB curve
    // is identity at the endpoints; alpha is not gamma-encoded.
    let mut expected = vec![0u8; (DIM * DIM * 4) as usize];
    for chunk in expected.chunks_exact_mut(4) {
        chunk.copy_from_slice(&[255, 0, 0, 255]);
    }

    assert_eq!(actual.len(), expected.len(), "readback length");

    let diffs = count_pixel_diffs(&actual, &expected);
    assert_eq!(
        diffs, 0,
        "P1 solid-rect receipt: {} pixels diverged from full-extent red",
        diffs
    );
}

fn count_pixel_diffs(actual: &[u8], expected: &[u8]) -> usize {
    let mut diffs = 0;
    for (a, b) in actual.chunks_exact(4).zip(expected.chunks_exact(4)) {
        if a != b {
            diffs += 1;
        }
    }
    diffs
}

/// Construct a single brush_solid `DrawIntent` whose primitive
/// covers all of NDC (-1..1) and produces opaque red.
///
/// Mirrors `netrender_device::tests::render_rect_smoke` setup. Each
/// input buffer is identity-shaped except `gpu_buffer_f[0]`, which
/// holds the colour. The bind-group layout (clip-mask texture
/// included) is the same one the alpha pipeline needs; in opaque
/// mode the clip mask isn't sampled.
fn build_full_ndc_red_draw(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipe: &netrender::BrushSolidPipeline,
) -> DrawIntent {
    // PrimitiveHeader[0]: full-NDC local_rect, identity transform/clip
    // pointers, specific_prim_address pointing at gpu_buffer_f[0].
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
    let prim_headers = upload_storage(device, queue, "P1 prim_headers", &header_bytes);

    // Transform storage: identity m + identity inv_m.
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
    let transforms = upload_storage(device, queue, "P1 transforms", &transform_bytes);

    // GpuBuffer[0] = opaque red.
    let color: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    let gpu_buffer_bytes: Vec<u8> = color.iter().flat_map(|f| f.to_ne_bytes()).collect();
    let gpu_buffer_f = upload_storage(device, queue, "P1 gpu_buffer_f", &gpu_buffer_bytes);

    // RenderTaskData[0]: identity-equivalent picture task
    // (zero offsets, scale=1) — math collapses to clip-space pos.
    let render_task: [f32; 8] = [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
    let render_task_bytes: Vec<u8> = render_task.iter().flat_map(|f| f.to_ne_bytes()).collect();
    let render_tasks = upload_storage(device, queue, "P1 render_tasks", &render_task_bytes);

    // PerFrame: identity orthographic projection.
    let per_frame_bytes: Vec<u8> = identity.iter().flat_map(|f| f.to_ne_bytes()).collect();
    let per_frame = upload_uniform(device, queue, "P1 per_frame", &per_frame_bytes);

    // Dummy 1×1 R8 clip mask. Layout demands the binding for both
    // pipelines; the opaque shader doesn't read it.
    let clip_mask = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("P1 dummy clip mask"),
        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let clip_mask_view = clip_mask.create_view(&wgpu::TextureViewDescriptor::default());

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("P1 brush_solid bind group"),
        layout: &pipe.layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: prim_headers.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: transforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: gpu_buffer_f.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: render_tasks.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 4, resource: per_frame.as_entire_binding() },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(&clip_mask_view),
            },
        ],
    });

    // Per-instance a_data = (prim_header_address=0, clip_address=0, ...).
    let a_data: [i32; 4] = [0, 0, 0, 0];
    let a_data_bytes: Vec<u8> = a_data.iter().flat_map(|i| i.to_ne_bytes()).collect();
    let a_data_buffer = upload_vertex(device, queue, "P1 a_data", &a_data_bytes);

    DrawIntent {
        pipeline: pipe.pipeline.clone(),
        bind_group,
        vertex_buffers: vec![a_data_buffer],
        vertex_range: 0..4,
        instance_range: 0..1,
        dynamic_offsets: Vec::new(),
        push_constants: Vec::new(),
    }
}

fn upload_storage(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    bytes: &[u8],
) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, bytes);
    buffer
}

fn upload_uniform(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    bytes: &[u8],
) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, bytes);
    buffer
}

fn upload_vertex(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    bytes: &[u8],
) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, bytes);
    buffer
}
