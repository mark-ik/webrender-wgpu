/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 5' — netrender Scene → vello::Scene image ingestion.
//!
//! Three probes:
//!
//! - `p5prime_01_full_image_round_trip` — push a 4-color quadrant
//!   image with full UV; verify each quadrant of the output matches.
//! - `p5prime_02_uv_subregion` — push the same image with UV
//!   `[0, 0, 0.5, 0.5]` (top-left quadrant only) onto a small target
//!   rect; verify the quadrant fills the target.
//! - `p5prime_03_alpha_tint` — push image with achromatic tint
//!   `[0.5, 0.5, 0.5, 0.5]` (50% alpha multiplier on a premultiplied
//!   white image); verify alpha modulation.

use netrender::{ImageData, Scene, boot, vello_rasterizer::scene_to_vello};
use vello::{AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, peniko::Color};

const DIM: u32 = 64;

fn make_renderer(device: &wgpu::Device) -> Renderer {
    Renderer::new(
        device,
        RendererOptions {
            use_cpu: false,
            antialiasing_support: AaSupport::area_only(),
            num_init_threads: None,
            pipeline_cache: None,
        },
    )
    .expect("vello::Renderer::new")
}

fn make_target(device: &wgpu::Device) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("p5' image target"),
        size: wgpu::Extent3d {
            width: DIM,
            height: DIM,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("p5' image storage view"),
        format: Some(wgpu::TextureFormat::Rgba8Unorm),
        ..Default::default()
    });
    (texture, view)
}

fn render_params() -> RenderParams {
    RenderParams {
        base_color: Color::from_rgba8(0, 0, 0, 0),
        width: DIM,
        height: DIM,
        antialiasing_method: AaConfig::Area,
    }
}

fn read_pixel(bytes: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * DIM + x) * 4) as usize;
    [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]
}

#[track_caller]
fn assert_within_tol(actual: [u8; 4], expected: [u8; 4], tol: u8, where_: &str) {
    let max = (0..4)
        .map(|i| (actual[i] as i16 - expected[i] as i16).unsigned_abs() as u8)
        .max()
        .unwrap();
    assert!(
        max <= tol,
        "{}: actual {:?}, expected {:?} (max channel diff = {}, tol = {})",
        where_,
        actual,
        expected,
        max,
        tol
    );
}

fn render_scene(scene: &Scene) -> Vec<u8> {
    let handles = boot().expect("wgpu boot");
    let device = &handles.device;
    let queue = &handles.queue;
    let mut renderer = make_renderer(device);

    let vscene = scene_to_vello(scene);
    let (target, view) = make_target(device);
    renderer
        .render_to_texture(device, queue, &vscene, &view, &render_params())
        .expect("vello render_to_texture");

    let wgpu_device = netrender_device::WgpuDevice::with_external(handles.clone())
        .expect("WgpuDevice::with_external");
    wgpu_device.read_rgba8_texture(&target, DIM, DIM)
}

/// Build an 8×8 image with four colored quadrants (4×4 each):
///   top-left = red, top-right = green,
///   bottom-left = blue, bottom-right = yellow.
fn quadrant_image() -> ImageData {
    const SZ: u32 = 8;
    let half = SZ / 2;
    let mut bytes = Vec::with_capacity((SZ * SZ * 4) as usize);
    for y in 0..SZ {
        for x in 0..SZ {
            let pixel: [u8; 4] = match (x < half, y < half) {
                (true, true) => [255, 0, 0, 255],     // top-left red
                (false, true) => [0, 255, 0, 255],    // top-right green
                (true, false) => [0, 0, 255, 255],    // bottom-left blue
                (false, false) => [255, 255, 0, 255], // bottom-right yellow
            };
            bytes.extend_from_slice(&pixel);
        }
    }
    ImageData::from_bytes(SZ, SZ, bytes)
}

/// Build a uniformly-white opaque image at the given size.
fn solid_white_image(size: u32) -> ImageData {
    let bytes: Vec<u8> = (0..size * size)
        .flat_map(|_| [255u8, 255, 255, 255])
        .collect();
    ImageData::from_bytes(size, size, bytes)
}

const IMG_KEY: u64 = 0x01;

/// Push the 8×8 quadrant image stretched to a 32×32 target at
/// (16, 16)–(48, 48). Each output quadrant should hold its source
/// color (within bilinear-sample tolerance away from the
/// quadrant boundary).
#[test]
fn p5prime_01_full_image_round_trip() {
    let mut scene = Scene::new(DIM, DIM);
    scene.image_sources.insert(IMG_KEY, quadrant_image());
    scene.push_image_full(
        16.0,
        16.0,
        48.0,
        48.0,
        [0.0, 0.0, 1.0, 1.0],
        [1.0, 1.0, 1.0, 1.0],
        IMG_KEY,
        0,
        netrender::NO_CLIP,
    );

    let bytes = render_scene(&scene);

    // Sample well-inside each output quadrant (target is 32×32 from
    // 16..48). Quadrant centers in target space: TL=(24,24), TR=(40,24),
    // BL=(24,40), BR=(40,40). Use 4-pixel insets to avoid bilinear
    // smear across quadrant edges.
    assert_within_tol(read_pixel(&bytes, 20, 20), [255, 0, 0, 255], 4, "TL red");
    assert_within_tol(read_pixel(&bytes, 44, 20), [0, 255, 0, 255], 4, "TR green");
    assert_within_tol(read_pixel(&bytes, 20, 44), [0, 0, 255, 255], 4, "BL blue");
    assert_within_tol(
        read_pixel(&bytes, 44, 44),
        [255, 255, 0, 255],
        4,
        "BR yellow",
    );
    // Outside the target rect: clear.
    assert_within_tol(read_pixel(&bytes, 4, 4), [0, 0, 0, 0], 1, "outside TL");
}

/// Push only the top-left quadrant of the image (UV
/// `[0, 0, 0.5, 0.5]`) onto a 16×16 target at (24, 24)–(40, 40).
/// The whole target should be red.
#[test]
fn p5prime_02_uv_subregion() {
    let mut scene = Scene::new(DIM, DIM);
    scene.image_sources.insert(IMG_KEY, quadrant_image());
    scene.push_image_full(
        24.0,
        24.0,
        40.0,
        40.0,
        [0.0, 0.0, 0.5, 0.5],
        [1.0, 1.0, 1.0, 1.0],
        IMG_KEY,
        0,
        netrender::NO_CLIP,
    );

    let bytes = render_scene(&scene);

    // Several interior pixels of the target rect — all should be red
    // because the UV subregion is pure red.
    for &(x, y) in &[(28, 28), (32, 32), (36, 36), (28, 36), (36, 28)] {
        assert_within_tol(
            read_pixel(&bytes, x, y),
            [255, 0, 0, 255],
            3,
            &format!("UV-clipped red at ({}, {})", x, y),
        );
    }
    // Outside the target rect: clear.
    assert_within_tol(
        read_pixel(&bytes, 8, 8),
        [0, 0, 0, 0],
        1,
        "outside subregion",
    );
}

/// Push a 16×16 white-opaque image with achromatic tint
/// `[0.5, 0.5, 0.5, 0.5]` (50% alpha multiplier in premultiplied
/// terms) onto a 32×32 target. With straight-alpha storage, the
/// output is `(255, 255, 255, 128)` modulo bilinear-edge artifacts.
#[test]
fn p5prime_03_alpha_tint() {
    let mut scene = Scene::new(DIM, DIM);
    scene.image_sources.insert(IMG_KEY, solid_white_image(16));
    scene.push_image_full(
        16.0,
        16.0,
        48.0,
        48.0,
        [0.0, 0.0, 1.0, 1.0],
        [0.5, 0.5, 0.5, 0.5],
        IMG_KEY,
        0,
        netrender::NO_CLIP,
    );

    let bytes = render_scene(&scene);

    // Interior of the target: white-with-alpha-128 in straight-alpha
    // storage. Use ±3 tolerance for any rounding in the alpha
    // multiplier path.
    for &(x, y) in &[(24, 24), (32, 32), (40, 40)] {
        assert_within_tol(
            read_pixel(&bytes, x, y),
            [255, 255, 255, 128],
            3,
            &format!("alpha-tinted at ({}, {})", x, y),
        );
    }
}
