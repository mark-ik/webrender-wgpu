/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase G4 receipt — same-device WebGL canvas texture composition.
//!
//! The source texture intentionally does **not** have `COPY_SRC`
//! usage. That distinguishes the zero-copy external-texture overlay
//! from vello's `register_texture` path, which copies registered
//! textures into vello's image atlas at frame start.

use netrender::{
    ColorLoad, ExternalTexturePlacement, NetrenderOptions, Scene, boot, create_netrender_instance,
};

const DIM: u32 = 64;
const SRC_DIM: u32 = 16;
const TILE_SIZE: u32 = 32;

fn make_target(device: &wgpu::Device) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pg4 target"),
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
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("pg4 target view"),
        format: Some(wgpu::TextureFormat::Rgba8Unorm),
        ..Default::default()
    });
    (texture, view)
}

fn make_canvas_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pg4 webgl canvas texture"),
        size: wgpu::Extent3d {
            width: SRC_DIM,
            height: SRC_DIM,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    let half = SRC_DIM / 2;
    let mut bytes = Vec::with_capacity((SRC_DIM * SRC_DIM * 4) as usize);
    for y in 0..SRC_DIM {
        for x in 0..SRC_DIM {
            let pixel: [u8; 4] = match (x < half, y < half) {
                (true, true) => [0, 255, 0, 255],
                (false, true) => [0, 0, 255, 255],
                (true, false) => [255, 255, 0, 255],
                (false, false) => [255, 255, 255, 255],
            };
            bytes.extend_from_slice(&pixel);
        }
    }

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(SRC_DIM * 4),
            rows_per_image: Some(SRC_DIM),
        },
        wgpu::Extent3d {
            width: SRC_DIM,
            height: SRC_DIM,
            depth_or_array_layers: 1,
        },
    );

    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
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

#[test]
fn pg4_webgl_canvas_texture_composes_without_copy_src_usage() {
    let handles = boot().expect("wgpu boot");
    let device = handles.device.clone();
    let queue = handles.queue.clone();
    let renderer = create_netrender_instance(
        handles,
        NetrenderOptions {
            tile_cache_size: Some(TILE_SIZE),
            enable_vello: true,
            ..Default::default()
        },
    )
    .expect("create_netrender_instance");

    let (target, target_view) = make_target(&device);
    let (_canvas_texture, canvas_view) = make_canvas_texture(&device, &queue);

    let mut scene = Scene::new(DIM, DIM);
    scene.push_rect(0.0, 0.0, DIM as f32, DIM as f32, [1.0, 0.0, 0.0, 1.0]);
    renderer.render_vello(&scene, &target_view, ColorLoad::default());

    renderer.compose_external_texture(
        &canvas_view,
        &target_view,
        wgpu::TextureFormat::Rgba8Unorm,
        DIM,
        DIM,
        ExternalTexturePlacement::new([16.0, 16.0, 48.0, 48.0]),
    );

    let bytes = renderer.wgpu_device.read_rgba8_texture(&target, DIM, DIM);

    assert_within_tol(
        read_pixel(&bytes, 4, 4),
        [255, 0, 0, 255],
        1,
        "background red",
    );
    assert_within_tol(
        read_pixel(&bytes, 20, 20),
        [0, 255, 0, 255],
        1,
        "canvas TL green",
    );
    assert_within_tol(
        read_pixel(&bytes, 44, 20),
        [0, 0, 255, 255],
        1,
        "canvas TR blue",
    );
    assert_within_tol(
        read_pixel(&bytes, 20, 44),
        [255, 255, 0, 255],
        1,
        "canvas BL yellow",
    );
    assert_within_tol(
        read_pixel(&bytes, 44, 44),
        [255, 255, 255, 255],
        1,
        "canvas BR white",
    );
}

#[test]
fn pg4_webgl_canvas_texture_opacity_blends_over_vello_scene() {
    let handles = boot().expect("wgpu boot");
    let device = handles.device.clone();
    let queue = handles.queue.clone();
    let renderer = create_netrender_instance(
        handles,
        NetrenderOptions {
            tile_cache_size: Some(TILE_SIZE),
            enable_vello: true,
            ..Default::default()
        },
    )
    .expect("create_netrender_instance");

    let (target, target_view) = make_target(&device);
    let (_canvas_texture, canvas_view) = make_canvas_texture(&device, &queue);

    let mut scene = Scene::new(DIM, DIM);
    scene.push_rect(0.0, 0.0, DIM as f32, DIM as f32, [1.0, 0.0, 0.0, 1.0]);
    renderer.render_vello(&scene, &target_view, ColorLoad::default());

    renderer.compose_external_texture(
        &canvas_view,
        &target_view,
        wgpu::TextureFormat::Rgba8Unorm,
        DIM,
        DIM,
        ExternalTexturePlacement::new([16.0, 16.0, 48.0, 48.0]).with_opacity(0.5),
    );

    let bytes = renderer.wgpu_device.read_rgba8_texture(&target, DIM, DIM);
    assert_within_tol(
        read_pixel(&bytes, 20, 20),
        [128, 128, 0, 255],
        2,
        "50% green over red",
    );
}
