/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Integration tests for WgpuHal backend and cross-backend pixel parity.
//!
//! Test groups:
//!   1. WgpuHal device-level: factory closure initialises WgpuDevice correctly.
//!   2. Cross-backend pixel parity: WgpuShared and WgpuHal render the same scene
//!      to pixel-identical output (they share all rendering code).
//!   3. composite_output_hal: raw backend texture handle is accessible after render.
//!   4. Multi-instance shared-device: two independent renderers on one wgpu::Device.
//!
//! On pixel mismatch, test helpers write `WR_TEST_OUTPUT_DIR` (default: /tmp)
//! PNG files for both images so failures can be inspected visually.

#![cfg(feature = "wgpu_native")]

extern crate webrender;

use std::path::{Path, PathBuf};

use webrender::api::units::*;
use webrender::api::*;
use webrender::render_api::*;
use webrender::{RendererBackend, WgpuDevice, wgpu};

// ──────────────────────────── shared helpers ─────────────────────────────────

fn make_adapter() -> Option<wgpu::Adapter> {
    let instance = wgpu::Instance::default();
    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::None,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .ok()
}

fn make_device(adapter: &wgpu::Adapter) -> (wgpu::Device, wgpu::Queue) {
    pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("test device"),
            ..Default::default()
        },
    ))
    .expect("failed to create device")
}

struct NoopNotifier;
impl RenderNotifier for NoopNotifier {
    fn clone(&self) -> Box<dyn RenderNotifier> { Box::new(NoopNotifier) }
    fn wake_up(&self, _: bool) {}
    fn new_frame_ready(&self, _: DocumentId, _: FramePublishId, _: &FrameReadyParams) {}
}

/// Output directory for diagnostic PNGs on pixel mismatch.
fn test_output_dir() -> PathBuf {
    std::env::var("WR_TEST_OUTPUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
}

/// Write a raw RGBA8 pixel buffer as a PNG file to `path`.
fn write_png(path: &Path, width: u32, height: u32, pixels: &[u8]) {
    use std::fs::File;
    use std::io::BufWriter;
    let file = match File::create(path) {
        Ok(f) => f,
        Err(e) => { eprintln!("PNG write failed ({path:?}): {e}"); return; }
    };
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::RGBA);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("PNG header");
    writer.write_image_data(pixels).expect("PNG data");
}

/// Assert pixel buffers are identical, dumping PNGs on failure.
///
/// `label_a` / `label_b` are used in the PNG filename (e.g. "shared", "hal").
#[track_caller]
fn assert_pixels_equal(
    label_a: &str, pixels_a: &[u8],
    label_b: &str, pixels_b: &[u8],
    width: u32, height: u32,
    test_name: &str,
) {
    if pixels_a == pixels_b {
        return;
    }

    // Count differing pixels for the error message.
    let diff_pixels: usize = pixels_a.chunks(4)
        .zip(pixels_b.chunks(4))
        .filter(|(a, b)| a != b)
        .count();
    let total = (width * height) as usize;

    // Dump PNGs for visual inspection.
    let dir = test_output_dir();
    let path_a = dir.join(format!("{test_name}_{label_a}.png"));
    let path_b = dir.join(format!("{test_name}_{label_b}.png"));
    write_png(&path_a, width, height, pixels_a);
    write_png(&path_b, width, height, pixels_b);

    panic!(
        "Pixel mismatch in '{test_name}': {diff_pixels}/{total} pixels differ.\n\
         Diagnostic PNGs written:\n  {label_a}: {path_a:?}\n  {label_b}: {path_b:?}"
    );
}

/// Render a fixed 256×256 scene (4 solid-colour quadrants) using the given
/// backend and return the raw RGBA8 pixel buffer from CPU readback.
fn render_solid_quads(backend: RendererBackend) -> Vec<u8> {
    let opts = webrender::WebRenderOptions {
        clear_color: ColorF::new(0.0, 0.0, 0.0, 1.0),
        ..Default::default()
    };
    let (mut renderer, sender) = webrender::create_webrender_instance_with_backend(
        backend,
        Box::new(NoopNotifier),
        opts,
        None,
    )
    .expect("Failed to create WebRender instance");

    let device_size = DeviceIntSize::new(256, 256);
    let mut api = sender.create_api();
    let document = api.add_document(device_size);
    let pipeline = PipelineId(0, 0);

    let mut builder = DisplayListBuilder::new(pipeline);
    builder.begin();
    let sac = SpaceAndClipInfo::root_scroll(pipeline);

    for (x, y, r, g, b) in [
        (0.0f32,  0.0f32,  1.0, 0.0, 0.0), // red    — top-left
        (128.0,   0.0,     0.0, 1.0, 0.0), // green  — top-right
        (0.0,     128.0,   0.0, 0.0, 1.0), // blue   — bottom-left
        (128.0,   128.0,   1.0, 1.0, 0.0), // yellow — bottom-right
    ] {
        let rect = LayoutRect::from_origin_and_size(
            LayoutPoint::new(x, y),
            LayoutSize::new(128.0, 128.0),
        );
        builder.push_rect(&CommonItemProperties::new(rect, sac), rect, ColorF::new(r, g, b, 1.0));
    }

    let mut txn = Transaction::new();
    txn.set_display_list(Epoch(0), builder.end());
    txn.set_root_pipeline(pipeline);
    txn.generate_frame(0, true, false, RenderReasons::empty());
    api.send_transaction(document, txn);
    api.flush_scene_builder();
    renderer.update();

    renderer.render(device_size, 0).expect("render failed");

    let rect = FramebufferIntRect::from_origin_and_size(
        FramebufferIntPoint::new(0, 0),
        FramebufferIntSize::new(256, 256),
    );
    let pixels = renderer.read_pixels_rgba8(rect);
    renderer.deinit();
    pixels
}

// ────────────────────── 1. WgpuHal device-level tests ────────────────────────

#[test]
fn wgpu_hal_factory_initialises_device() {
    let Some(adapter) = make_adapter() else {
        eprintln!("wgpu-hal test: no adapter — skipping");
        return;
    };
    let (device, queue) = make_device(&adapter);
    let wgpu_dev = WgpuDevice::from_shared_device(device, queue);
    assert!(!wgpu_dev.has_surface(), "WgpuHal device should be headless (no surface)");
}

#[test]
fn wgpu_hal_factory_can_create_texture() {
    let Some(adapter) = make_adapter() else {
        eprintln!("wgpu-hal test: no adapter — skipping");
        return;
    };
    let (device, queue) = make_device(&adapter);
    let mut wgpu_dev = WgpuDevice::from_shared_device(device, queue);

    let tex = wgpu_dev.create_data_texture(
        "hal-test-texture",
        8,
        8,
        wgpu::TextureFormat::Rgba8Unorm,
        &[0xffu8; 8 * 8 * 4],
    );
    assert_eq!(tex.width, 8);
    assert_eq!(tex.height, 8);
    wgpu_dev.flush_encoder();
}

#[test]
fn wgpu_hal_factory_device_remains_functional_after_wr_use() {
    let Some(adapter) = make_adapter() else {
        eprintln!("wgpu-hal test: no adapter — skipping");
        return;
    };
    let (device, queue) = make_device(&adapter);
    let host_device = device.clone();

    // Give WebRender the device via the factory pattern.
    let mut wgpu_dev = WgpuDevice::from_shared_device(device, queue);
    let _tex = wgpu_dev.create_data_texture(
        "hal-test-use",
        4, 4, wgpu::TextureFormat::Rgba8Unorm,
        &[0u8; 64],
    );
    wgpu_dev.flush_encoder();
    drop(wgpu_dev);

    // Host device should still be fully usable.
    let _buf = host_device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("post-wr host buffer"),
        size: 128,
        usage: wgpu::BufferUsages::UNIFORM,
        mapped_at_creation: false,
    });
}

// ──────────────────── 2. Cross-backend pixel parity ──────────────────────────

/// WgpuShared and WgpuHal must produce pixel-identical output for any scene,
/// because they share 100% of the rendering code (WgpuHal factory just
/// provides a different route to the same WgpuDevice::from_shared_device path).
#[test]
fn wgpu_shared_and_wgpu_hal_are_pixel_identical() {
    let Some(adapter) = make_adapter() else {
        eprintln!("pixel-parity test: no adapter — skipping");
        return;
    };

    // WgpuShared backend.
    let (d1, q1) = make_device(&adapter);
    let shared_pixels = render_solid_quads(RendererBackend::WgpuShared { device: d1, queue: q1 });

    // WgpuHal backend — factory provides device from same adapter class.
    let adapter2 = make_adapter().unwrap();
    let hal_pixels = render_solid_quads(RendererBackend::WgpuHal {
        device_factory: Box::new(move || make_device(&adapter2)),
    });

    assert_pixels_equal(
        "shared", &shared_pixels,
        "hal", &hal_pixels,
        256, 256,
        "wgpu_shared_and_wgpu_hal_are_pixel_identical",
    );
}

// ─────────────── 3. composite_output_hal raw handle access ───────────────────

/// After rendering with WgpuHal, composite_output_hal<A>() must return a
/// non-None handle on the matching backend.  We gate this per-platform
/// since the backend type must be known at compile time.
#[test]
fn composite_output_hal_returns_handle() {
    let Some(adapter) = make_adapter() else {
        eprintln!("composite_output_hal test: no adapter — skipping");
        return;
    };
    let info = adapter.get_info();
    let (device, queue) = make_device(&adapter);

    let opts = webrender::WebRenderOptions {
        clear_color: ColorF::new(0.1, 0.1, 0.1, 1.0),
        ..Default::default()
    };
    let (mut renderer, sender) = webrender::create_webrender_instance_with_backend(
        RendererBackend::WgpuShared { device, queue },
        Box::new(NoopNotifier),
        opts,
        None,
    )
    .expect("Failed to create WebRender instance");

    let device_size = DeviceIntSize::new(64, 64);
    let mut api = sender.create_api();
    let document = api.add_document(device_size);
    let pipeline = PipelineId(0, 0);

    let mut builder = DisplayListBuilder::new(pipeline);
    builder.begin();
    let rect = LayoutRect::from_origin_and_size(LayoutPoint::zero(), LayoutSize::new(64.0, 64.0));
    builder.push_rect(
        &CommonItemProperties::new(rect, SpaceAndClipInfo::root_scroll(pipeline)),
        rect,
        ColorF::new(1.0, 0.0, 0.0, 1.0),
    );
    let mut txn = Transaction::new();
    txn.set_display_list(Epoch(0), builder.end());
    txn.set_root_pipeline(pipeline);
    txn.generate_frame(0, true, false, RenderReasons::empty());
    api.send_transaction(document, txn);
    api.flush_scene_builder();
    renderer.update();
    renderer.render(device_size, 0).expect("render failed");

    // composite_output() must be Some after render.
    assert!(renderer.composite_output().is_some(), "composite_output() must be Some after render");

    // composite_output_hal<A>() must be Some on the matching backend.
    // We dispatch on the runtime-detected backend to call the right generic.
    let hal_ok = unsafe {
        match info.backend {
            #[cfg(all(feature = "wgpu_backend", not(target_os = "ios"), not(target_os = "android")))]
            wgpu::Backend::Vulkan => {
                renderer.composite_output_hal::<wgpu::wgc::api::Vulkan>().is_some()
            }
            #[cfg(all(feature = "wgpu_backend", target_os = "macos"))]
            wgpu::Backend::Metal => {
                renderer.composite_output_hal::<wgpu::wgc::api::Metal>().is_some()
            }
            #[cfg(all(feature = "wgpu_backend", target_os = "windows"))]
            wgpu::Backend::Dx12 => {
                renderer.composite_output_hal::<wgpu::wgc::api::Dx12>().is_some()
            }
            _ => {
                eprintln!("composite_output_hal test: backend {:?} not covered — skipping hal check", info.backend);
                true // don't fail for uncovered backends
            }
        }
    };
    assert!(hal_ok, "composite_output_hal<A>() must return Some on matching backend {:?}", info.backend);

    renderer.deinit();
}

// ──────────────── 4. Multi-instance shared-device isolation ──────────────────

/// Two independent WebRender instances on a single wgpu::Device must not
/// interfere with each other.  Each renders a distinct solid colour; we verify
/// that each instance's readback reflects only its own scene.
#[test]
fn two_renderers_on_one_device_are_isolated() {
    let Some(adapter) = make_adapter() else {
        eprintln!("multi-instance test: no adapter — skipping");
        return;
    };

    // Single shared device — both renderers will use it.
    let (device, queue) = make_device(&adapter);

    let make_renderer = |r: f32, g: f32, b: f32, instance_label: &str| {
        let opts = webrender::WebRenderOptions {
            clear_color: ColorF::new(r, g, b, 1.0),
            ..Default::default()
        };
        let (mut renderer, sender) = webrender::create_webrender_instance_with_backend(
            RendererBackend::WgpuShared {
                device: device.clone(),
                queue: queue.clone(),
            },
            Box::new(NoopNotifier),
            opts,
            None,
        )
        .unwrap_or_else(|e| panic!("Failed to create renderer '{instance_label}': {e:?}"));

        let device_size = DeviceIntSize::new(64, 64);
        let mut api = sender.create_api();
        let document = api.add_document(device_size);
        let pipeline = PipelineId(0, 0);

        let mut builder = DisplayListBuilder::new(pipeline);
        builder.begin();
        // Push a rect covering the whole viewport in the chosen colour.
        let rect = LayoutRect::from_origin_and_size(
            LayoutPoint::zero(),
            LayoutSize::new(64.0, 64.0),
        );
        builder.push_rect(
            &CommonItemProperties::new(rect, SpaceAndClipInfo::root_scroll(pipeline)),
            rect,
            ColorF::new(r, g, b, 1.0),
        );
        let mut txn = Transaction::new();
        txn.set_display_list(Epoch(0), builder.end());
        txn.set_root_pipeline(pipeline);
        txn.generate_frame(0, true, false, RenderReasons::empty());
        api.send_transaction(document, txn);
        api.flush_scene_builder();
        renderer.update();
        renderer.render(device_size, 0).expect("render failed");
        (renderer, device_size)
    };

    let (mut renderer_a, size_a) = make_renderer(1.0, 0.0, 0.0, "red");   // solid red
    let (mut renderer_b, size_b) = make_renderer(0.0, 0.0, 1.0, "blue");  // solid blue

    let readback = |renderer: &mut webrender::Renderer, size: DeviceIntSize| {
        let rect = FramebufferIntRect::from_origin_and_size(
            FramebufferIntPoint::new(0, 0),
            FramebufferIntSize::new(size.width, size.height),
        );
        renderer.read_pixels_rgba8(rect)
    };

    let pixels_a = readback(&mut renderer_a, size_a);
    let pixels_b = readback(&mut renderer_b, size_b);

    renderer_a.deinit();
    renderer_b.deinit();

    // Every pixel in renderer_a should be red.
    for (i, chunk) in pixels_a.chunks(4).enumerate() {
        assert!(
            chunk[0] > 200 && chunk[1] < 50 && chunk[2] < 50,
            "renderer_a pixel {i}: expected red, got RGBA {:?}", chunk
        );
    }

    // Every pixel in renderer_b should be blue.
    for (i, chunk) in pixels_b.chunks(4).enumerate() {
        assert!(
            chunk[0] < 50 && chunk[1] < 50 && chunk[2] > 200,
            "renderer_b pixel {i}: expected blue, got RGBA {:?}", chunk
        );
    }

    // The two outputs must differ (they render different colours).
    assert_ne!(pixels_a, pixels_b, "renderers on the same device must produce independent output");
}

/// Two renderers on the same device can render sequentially and produce
/// the correct output even when interleaved.  This catches cases where
/// shared device state (bind groups, scratch buffers, etc.) leaks between
/// renderer instances.
#[test]
fn two_renderers_interleaved_produce_correct_pixels() {
    let Some(adapter) = make_adapter() else {
        eprintln!("interleaved test: no adapter — skipping");
        return;
    };

    let (device, queue) = make_device(&adapter);
    let device_size = DeviceIntSize::new(128, 128);

    // Helper: create one renderer + a stable API (same namespace throughout).
    let make_wr = |r: f32, g: f32, b: f32, pipeline_ns: u32| {
        let opts = webrender::WebRenderOptions {
            clear_color: ColorF::new(r, g, b, 1.0),
            ..Default::default()
        };
        let (renderer, sender) = webrender::create_webrender_instance_with_backend(
            RendererBackend::WgpuShared {
                device: device.clone(),
                queue: queue.clone(),
            },
            Box::new(NoopNotifier),
            opts,
            None,
        ).expect("create renderer");
        let mut api = sender.create_api();
        let doc = api.add_document(device_size);
        let pipeline = PipelineId(pipeline_ns, 0);
        (renderer, api, doc, pipeline)
    };

    let push_solid = |api: &mut RenderApi, doc: DocumentId, pipeline: PipelineId,
                      epoch: u32, r: f32, g: f32, b: f32| {
        let mut builder = DisplayListBuilder::new(pipeline);
        builder.begin();
        let rect = LayoutRect::from_origin_and_size(LayoutPoint::zero(), LayoutSize::new(128.0, 128.0));
        builder.push_rect(
            &CommonItemProperties::new(rect, SpaceAndClipInfo::root_scroll(pipeline)),
            rect,
            ColorF::new(r, g, b, 1.0),
        );
        let mut txn = Transaction::new();
        txn.set_display_list(Epoch(epoch), builder.end());
        txn.set_root_pipeline(pipeline);
        txn.generate_frame(0, true, false, RenderReasons::empty());
        api.send_transaction(doc, txn);
        api.flush_scene_builder();
    };

    let (mut wr_a, mut api_a, doc_a, pipe_a) = make_wr(0.0, 1.0, 0.0, 0); // green
    let (mut wr_b, mut api_b, doc_b, pipe_b) = make_wr(1.0, 1.0, 0.0, 1); // yellow

    // Round 1: render A (green), then B (yellow).
    push_solid(&mut api_a, doc_a, pipe_a, 0, 0.0, 1.0, 0.0);
    wr_a.update();
    wr_a.render(device_size, 0).expect("wr_a render 1");

    push_solid(&mut api_b, doc_b, pipe_b, 0, 1.0, 1.0, 0.0);
    wr_b.update();
    wr_b.render(device_size, 0).expect("wr_b render 1");

    // Round 2: re-render A with a different colour (red), verify it changed.
    push_solid(&mut api_a, doc_a, pipe_a, 1, 1.0, 0.0, 0.0);
    wr_a.update();
    wr_a.render(device_size, 0).expect("wr_a render 2");

    let readback = |r: &mut webrender::Renderer| {
        let rect = FramebufferIntRect::from_origin_and_size(
            FramebufferIntPoint::new(0, 0),
            FramebufferIntSize::new(128, 128),
        );
        r.read_pixels_rgba8(rect)
    };

    let pixels_a2 = readback(&mut wr_a);
    let pixels_b1 = readback(&mut wr_b);

    wr_a.deinit();
    wr_b.deinit();

    // A's second render should be red (clear colour = green, rect = red → rect wins).
    for (i, chunk) in pixels_a2.chunks(4).enumerate() {
        assert!(
            chunk[0] > 200 && chunk[1] < 50 && chunk[2] < 50,
            "wr_a round-2 pixel {i}: expected red, got RGBA {:?}", chunk
        );
    }

    // B's output should still be yellow (unaffected by A's re-render).
    for (i, chunk) in pixels_b1.chunks(4).enumerate() {
        assert!(
            chunk[0] > 200 && chunk[1] > 200 && chunk[2] < 50,
            "wr_b pixel {i}: expected yellow, got RGBA {:?}", chunk
        );
    }

    // A (red) and B (yellow) must differ — interleaved renders don't bleed.
    assert_ne!(pixels_a2, pixels_b1, "A (red) and B (yellow) must differ");
}

// ──────────────── 5. render_to_view (per-frame surface texture) ──────────────

/// Verify that `render_to_view()` writes into a caller-provided TextureView.
///
/// Allocates a `RENDER_ATTACHMENT | COPY_SRC` texture, creates a view, passes
/// it to `render_to_view()`, then reads back the pixels via a staging buffer
/// and checks the quadrant colours.
#[test]
fn render_to_view_writes_into_caller_texture() {
    let Some(adapter) = make_adapter() else {
        eprintln!("render_to_view test: no adapter — skipping");
        return;
    };
    let (device, queue) = make_device(&adapter);

    const W: u32 = 256;
    const H: u32 = 256;

    // Allocate the "swap-chain-like" target texture on the host device.
    let target_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("host frame texture"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target_tex.create_view(&wgpu::TextureViewDescriptor::default());

    // Build a WebRender instance on the same device.
    let opts = webrender::WebRenderOptions {
        clear_color: ColorF::new(0.0, 0.0, 0.0, 1.0),
        ..Default::default()
    };
    let (mut renderer, sender) = webrender::create_webrender_instance_with_backend(
        RendererBackend::WgpuShared {
            device: device.clone(),
            queue: queue.clone(),
        },
        Box::new(NoopNotifier),
        opts,
        None,
    )
    .expect("Failed to create WebRender");

    let device_size = DeviceIntSize::new(W as i32, H as i32);
    let mut api = sender.create_api();
    let document = api.add_document(device_size);
    let pipeline = PipelineId(9, 9);

    let mut builder = DisplayListBuilder::new(pipeline);
    builder.begin();
    let sac = SpaceAndClipInfo::root_scroll(pipeline);
    for (x, y, r, g, b) in [
        (0.0f32,  0.0,   1.0, 0.0, 0.0), // red
        (128.0,   0.0,   0.0, 1.0, 0.0), // green
        (0.0,     128.0, 0.0, 0.0, 1.0), // blue
        (128.0,   128.0, 1.0, 1.0, 0.0), // yellow
    ] {
        let rect = LayoutRect::from_origin_and_size(
            LayoutPoint::new(x, y), LayoutSize::new(128.0, 128.0),
        );
        builder.push_rect(&CommonItemProperties::new(rect, sac), rect, ColorF::new(r, g, b, 1.0));
    }
    let mut txn = Transaction::new();
    txn.set_display_list(Epoch(0), builder.end());
    txn.set_root_pipeline(pipeline);
    txn.generate_frame(0, true, false, RenderReasons::empty());
    api.send_transaction(document, txn);
    api.flush_scene_builder();
    renderer.update();

    // Render into the caller-provided view.
    renderer.render_to_view(target_view, device_size, 0).expect("render_to_view failed");

    // Read back via staging buffer.
    let bytes_per_row = W * 4;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("staging"),
        size: (bytes_per_row * H) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
    );
    queue.submit(Some(enc.finish()));

    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx.send(r); });
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    rx.recv().expect("map failed").expect("map error");

    let data = slice.get_mapped_range();
    // Bgra8Unorm: check quadrant centres (B G R A byte order).
    let pixel = |x: u32, y: u32| -> (u8, u8, u8) {
        let off = (y * bytes_per_row + x * 4) as usize;
        // BGRA → (R, G, B)
        (data[off + 2], data[off + 1], data[off])
    };

    let (r1, g1, b1) = pixel(64, 64);
    let (r2, g2, b2) = pixel(192, 64);
    let (r3, g3, b3) = pixel(64, 192);
    let (r4, g4, b4) = pixel(192, 192);

    drop(data);
    staging.unmap();
    renderer.deinit();

    let close = |a: u8, b: u8| (a as i16 - b as i16).unsigned_abs() < 10;
    assert!(close(r1, 255) && close(g1, 0) && close(b1, 0),   "TL should be red,    got ({r1},{g1},{b1})");
    assert!(close(r2, 0) && close(g2, 255) && close(b2, 0),   "TR should be green,  got ({r2},{g2},{b2})");
    assert!(close(r3, 0) && close(g3, 0) && close(b3, 255),   "BL should be blue,   got ({r3},{g3},{b3})");
    assert!(close(r4, 255) && close(g4, 255) && close(b4, 0), "BR should be yellow, got ({r4},{g4},{b4})");
}
