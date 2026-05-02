/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 1' first-light receipt — vello renders a rect into our wgpu
//! device and we read it back.
//!
//! Smallest possible end-to-end test that proves:
//!   - vello compiles + links into our project
//!   - vello's `Renderer` boots on the wgpu device our `boot()` returns
//!   - vello renders a single filled rect into a `Rgba8Unorm` storage
//!     texture with `view_formats: &[Rgba8UnormSrgb]`
//!   - `WgpuDevice::read_rgba8_texture` reads back the bytes vello wrote
//!
//! This test doubles as the §11.6 runtime spike. If it passes, we've
//! confirmed:
//!   1. wgpu validation accepts the (Rgba8Unorm storage, Rgba8UnormSrgb
//!      view-format) pair on this adapter.
//!   2. Vello's quantization round-trip lands the expected sRGB-encoded
//!      bytes into the storage texture.
//!
//! Receipt: a 64×64 red rect renders to all-(255, 0, 0, 255) bytes.
//! Vello blends in sRGB-encoded space; for primary opaque red the
//! storage value is sRGB(1.0, 0, 0, 1.0) = (255, 0, 0, 255).

use vello::{
    AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, Scene,
    kurbo::{Affine, Rect},
    peniko::{Color, Fill},
};

use netrender::boot;

const DIM: u32 = 64;

#[test]
fn p1prime_01_vello_renders_red_rect() {
    let handles = boot().expect("wgpu boot");
    let device = &handles.device;
    let queue = &handles.queue;

    let mut renderer = Renderer::new(
        device,
        RendererOptions {
            use_cpu: false,
            antialiasing_support: AaSupport::area_only(),
            num_init_threads: None,
            pipeline_cache: None,
        },
    )
    .expect("vello::Renderer::new");

    // Build a vello scene with one full-canvas red rect.
    let mut scene = Scene::new();
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgba8(255, 0, 0, 255),
        None,
        &Rect::new(0.0, 0.0, DIM as f64, DIM as f64),
    );

    // Target: Rgba8Unorm storage texture with an Rgba8UnormSrgb view-
    // format slot reserved. Vello writes sRGB-encoded bytes into the
    // storage view; downstream sampling through the Rgba8UnormSrgb
    // view will hardware-decode to linear (verified separately, see
    // §6.1 of the vello rasterizer plan). For this first-light test
    // we just read the raw bytes back via COPY_SRC.
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("p1' vello target"),
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
    let storage_view = target.create_view(&wgpu::TextureViewDescriptor {
        label: Some("p1' vello storage view"),
        format: Some(wgpu::TextureFormat::Rgba8Unorm),
        ..Default::default()
    });

    renderer
        .render_to_texture(
            device,
            queue,
            &scene,
            &storage_view,
            &RenderParams {
                base_color: Color::from_rgba8(0, 0, 0, 0),
                width: DIM,
                height: DIM,
                antialiasing_method: AaConfig::Area,
            },
        )
        .expect("vello render_to_texture");

    // Read back what vello wrote. We need a WgpuDevice to use
    // read_rgba8_texture — wrap our handles. (Boot already gave us
    // the WgpuDevice-equivalent handles; netrender's read helper is
    // on WgpuDevice. Construct one over the same handles.)
    let wgpu_device = netrender_device::WgpuDevice::with_external(handles.clone())
        .expect("WgpuDevice::with_external");
    let bytes = wgpu_device.read_rgba8_texture(&target, DIM, DIM);

    assert_eq!(bytes.len(), (DIM * DIM * 4) as usize);

    // Every pixel: red, opaque. Storage holds sRGB-encoded values;
    // sRGB(1.0) at the endpoints is identity, so primary red round-
    // trips to (255, 0, 0, 255).
    let mut max_diff: u8 = 0;
    let mut diff_count = 0usize;
    for chunk in bytes.chunks_exact(4) {
        for (i, &expected) in [255u8, 0, 0, 255].iter().enumerate() {
            let d = (chunk[i] as i16 - expected as i16).unsigned_abs() as u8;
            if d > 2 {
                diff_count += 1;
            }
            max_diff = max_diff.max(d);
        }
    }
    assert_eq!(
        diff_count, 0,
        "p1' first-light: {} channel values differ from (255,0,0,255) by >2 (max diff = {})",
        diff_count, max_diff
    );
}
