/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 5 golden harness — image primitives + image cache.
//!
//! Receipt: image-rect scene with checkerboard source pixel-matches
//! reference. Tests nearest-neighbor UV mapping, tint, alpha-blend,
//! and UV sub-rect selection.
//!
//! Tests:
//!   p5_01_checkerboard   — 2×2 source → nearest-neighbor, black background
//!   p5_02_uv_subrect     — draw only the top-left quadrant of the image
//!   p5_03_tint           — red tint on a white image
//!   p5_04_alpha_image    — 50%-transparent image over a blue background
//!   p5_05_phase4_regression — opaque rect scene unchanged by Phase 5 path

use std::path::{Path, PathBuf};

use netrender::{
    ColorLoad, FrameTarget, ImageData, ImageKey, NetrenderOptions, Scene, boot,
    create_netrender_instance,
};

const DIM: u32 = 256;
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

// ── Image generators ───────────────────────────────────────────────

/// 2×2 checkerboard: top-left/bottom-right = white, top-right/bottom-left = black.
fn checkerboard_2x2() -> ImageData {
    // Pixels: (0,0)=white, (1,0)=black, (0,1)=black, (1,1)=white
    let bytes = vec![
        255, 255, 255, 255,   0,   0,   0, 255,
          0,   0,   0, 255, 255, 255, 255, 255,
    ];
    ImageData { width: 2, height: 2, bytes }
}

/// Solid white 1×1 image.
fn white_1x1() -> ImageData {
    ImageData { width: 1, height: 1, bytes: vec![255, 255, 255, 255] }
}

// ── PNG helpers ────────────────────────────────────────────────────

fn oracle_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("oracle")
        .join("p5")
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).expect("create oracle/p5 dir");
    let file = std::fs::File::create(path)
        .unwrap_or_else(|e| panic!("creating {}: {}", path.display(), e));
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().expect("png header");
    writer.write_image_data(rgba).expect("png pixels");
}

fn read_png(path: &Path) -> (u32, u32, Vec<u8>) {
    let file = std::fs::File::open(path)
        .unwrap_or_else(|e| panic!("opening {}: {}", path.display(), e));
    let dec = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = dec.read_info().expect("png read_info");
    let info = reader.info();
    assert_eq!(info.color_type, png::ColorType::Rgba);
    assert_eq!(info.bit_depth, png::BitDepth::Eight);
    let (w, h) = (info.width, info.height);
    let mut buf = vec![0u8; reader.output_buffer_size()];
    reader.next_frame(&mut buf).expect("png decode");
    (w, h, buf)
}

fn should_regen() -> bool {
    std::env::var("NETRENDER_REGEN").map_or(false, |v| v == "1")
}

// ── Core runner ────────────────────────────────────────────────────

fn run_scene_golden(name: &str, scene: Scene) {
    let [vw, vh] = [scene.viewport_width, scene.viewport_height];

    let handles = boot().expect("wgpu boot");
    let device = handles.device.clone();
    let renderer = create_netrender_instance(handles, NetrenderOptions::default())
        .expect("create_netrender_instance");

    let target_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(name),
        size: wgpu::Extent3d { width: vw, height: vh, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TARGET_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target_tex.create_view(&wgpu::TextureViewDescriptor::default());

    let prepared = renderer.prepare(&scene);
    renderer.render(
        &prepared,
        FrameTarget { view: &target_view, format: TARGET_FORMAT, width: vw, height: vh },
        ColorLoad::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }),
    );

    let actual = renderer.wgpu_device.read_rgba8_texture(&target_tex, vw, vh);

    let oracle_path = oracle_dir().join(format!("{name}.png"));
    if should_regen() || !oracle_path.exists() {
        write_png(&oracle_path, vw, vh, &actual);
        println!("  captured oracle: {}", oracle_path.display());
        return;
    }

    let (ow, oh, oracle) = read_png(&oracle_path);
    assert_eq!((ow, oh), (vw, vh), "{name}: oracle size mismatch");
    assert_eq!(actual.len(), oracle.len(), "{name}: readback length mismatch");

    let mut diffs = 0usize;
    for (a, b) in actual.chunks_exact(4).zip(oracle.chunks_exact(4)) {
        if a != b {
            diffs += 1;
        }
    }
    assert_eq!(diffs, 0, "{name}: {diffs} pixels differ from oracle");
}

// ── Tests ──────────────────────────────────────────────────────────

/// Receipt: 2×2 checkerboard drawn at 128×128 on black background.
/// Nearest-neighbor UV mapping produces 4 quadrants of 64×64 pixels:
/// top-left=white, top-right=black, bottom-left=black, bottom-right=white.
#[test]
fn p5_01_checkerboard() {
    const KEY: ImageKey = 1;
    let mut scene = Scene::new(DIM, DIM);
    // Black background
    scene.push_rect(0.0, 0.0, 256.0, 256.0, [0.0, 0.0, 0.0, 1.0]);
    scene.push_image(64.0, 64.0, 192.0, 192.0, KEY, checkerboard_2x2());
    run_scene_golden("p5_01_checkerboard", scene);
}

/// UV sub-rect: draw only the top-left texel of the 2×2 checkerboard
/// (UV [0, 0, 0.5, 0.5] → all white).
#[test]
fn p5_02_uv_subrect() {
    const KEY: ImageKey = 2;
    let mut scene = Scene::new(DIM, DIM);
    scene.push_rect(0.0, 0.0, 256.0, 256.0, [0.0, 0.0, 0.0, 1.0]);
    scene.set_image_source(KEY, checkerboard_2x2());
    scene.push_image_full(
        64.0, 64.0, 192.0, 192.0,
        [0.0, 0.0, 0.5, 0.5], // top-left texel only
        [1.0, 1.0, 1.0, 1.0], // white tint = no-op
        KEY,
        0,
        [f32::NEG_INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::INFINITY],
    );
    run_scene_golden("p5_02_uv_subrect", scene);
}

/// Tint: a solid-white 1×1 image with a red premultiplied tint should
/// produce a red rect at the image position.
#[test]
fn p5_03_tint() {
    const KEY: ImageKey = 3;
    let mut scene = Scene::new(DIM, DIM);
    // White background so the tint contrast is visible
    scene.push_rect(0.0, 0.0, 256.0, 256.0, [1.0, 1.0, 1.0, 1.0]);
    scene.set_image_source(KEY, white_1x1());
    scene.push_image_full(
        64.0, 64.0, 192.0, 192.0,
        [0.0, 0.0, 1.0, 1.0],
        [1.0, 0.0, 0.0, 1.0], // red tint (premultiplied: r*a=1, g=0, b=0, a=1)
        KEY,
        0,
        [f32::NEG_INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::INFINITY],
    );
    run_scene_golden("p5_03_tint", scene);
}

/// Alpha image: 50%-transparent white image over a blue background.
/// Premultiplied tint [0.5, 0.5, 0.5, 0.5] on a white texture gives
/// premult pink-ish blend (0.5 alpha over blue).
#[test]
fn p5_04_alpha_image() {
    const KEY: ImageKey = 4;
    let mut scene = Scene::new(DIM, DIM);
    // Blue background
    scene.push_rect(0.0, 0.0, 256.0, 256.0, [0.0, 0.0, 1.0, 1.0]);
    scene.set_image_source(KEY, white_1x1());
    scene.push_image_full(
        64.0, 64.0, 192.0, 192.0,
        [0.0, 0.0, 1.0, 1.0],
        [0.5, 0.5, 0.5, 0.5], // 50% transparent white (premultiplied)
        KEY,
        0,
        [f32::NEG_INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::INFINITY],
    );
    run_scene_golden("p5_04_alpha_image", scene);
}

/// Phase 4 regression: opaque rect scene must produce identical pixels
/// through the Phase 5 renderer (no image draws in this scene).
#[test]
fn p5_05_phase4_regression() {
    let mut scene = Scene::new(DIM, DIM);
    scene.push_rect(0.0, 0.0, 128.0, 128.0, [1.0, 0.0, 0.0, 1.0]);
    scene.push_rect(128.0, 0.0, 256.0, 128.0, [0.0, 1.0, 0.0, 1.0]);
    scene.push_rect(0.0, 128.0, 128.0, 256.0, [0.0, 0.0, 1.0, 1.0]);
    scene.push_rect(128.0, 128.0, 256.0, 256.0, [1.0, 1.0, 0.0, 1.0]);
    run_scene_golden("p5_05_phase4_regression", scene);
}
