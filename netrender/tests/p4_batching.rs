/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 4 golden harness — depth sorting + alpha blending.
//!
//! Receipt: 100-overlapping-rect scene with mixed opacity renders
//! correctly; opaque early-Z visible in profile (front-to-back sort
//! ensures back fragments are depth-discarded by the GPU).
//!
//! Tests:
//!   p4_01_opaque_occlusion — opaque front-to-back sort correctness
//!   p4_02_alpha_blend       — premultiplied-alpha blending
//!   p4_03_mixed_opaque_alpha — opaques + alphas in same scene
//!   p4_04_100_rects          — receipt: 100 overlapping rects, mixed opacity
//!   p4_05_phase2_regression  — Phase 2 scenes pass through Phase 4 unchanged

use std::path::{Path, PathBuf};

use netrender::{
    ColorLoad, FrameTarget, NetrenderOptions, Scene, boot, create_netrender_instance,
};

const DIM: u32 = 256;
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

// ── PNG helpers ────────────────────────────────────────────────────

fn oracle_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("oracle")
        .join("p4")
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).expect("create oracle/p4 dir");
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

/// Opaque front-to-back sort: blue square (top/front) must occlude red
/// background everywhere they overlap. Verifies depth write + compare.
#[test]
fn p4_01_opaque_occlusion() {
    let mut scene = Scene::new(DIM, DIM);
    // Red fills viewport (painter index 0 = back).
    scene.push_rect(0.0, 0.0, 256.0, 256.0, [1.0, 0.0, 0.0, 1.0]);
    // Blue 128×128 top-left (painter index 1 = front).
    scene.push_rect(0.0, 0.0, 128.0, 128.0, [0.0, 0.0, 1.0, 1.0]);
    run_scene_golden("p4_01_opaque_occlusion", scene);
}

/// Premultiplied-alpha blending: 50%-transparent red over white gives
/// pink; opaque white corners untouched.
#[test]
fn p4_02_alpha_blend() {
    let mut scene = Scene::new(DIM, DIM);
    // White base (opaque, index 0).
    scene.push_rect(0.0, 0.0, 256.0, 256.0, [1.0, 1.0, 1.0, 1.0]);
    // 50% transparent red center (premultiplied: rgb *= alpha).
    scene.push_rect(64.0, 64.0, 192.0, 192.0, [0.5, 0.0, 0.0, 0.5]);
    run_scene_golden("p4_02_alpha_blend", scene);
}

/// Mixed scene: opaque background + opaque top layer + alpha layers in
/// between. Opaques should occlude; alphas should blend correctly.
#[test]
fn p4_03_mixed_opaque_alpha() {
    let mut scene = Scene::new(DIM, DIM);
    // White background (opaque, index 0).
    scene.push_rect(0.0, 0.0, 256.0, 256.0, [1.0, 1.0, 1.0, 1.0]);
    // 50% cyan over center (alpha, index 1).
    scene.push_rect(32.0, 32.0, 224.0, 224.0, [0.0, 0.5, 0.5, 0.5]);
    // 50% magenta over inner area (alpha, index 2).
    scene.push_rect(64.0, 64.0, 192.0, 192.0, [0.5, 0.0, 0.5, 0.5]);
    // Opaque black square in the very center, covers everything (opaque, index 3).
    scene.push_rect(96.0, 96.0, 160.0, 160.0, [0.0, 0.0, 0.0, 1.0]);
    run_scene_golden("p4_03_mixed_opaque_alpha", scene);
}

/// Receipt: 100 overlapping rects with mixed opacity render correctly.
/// 50 opaque rows + 50 semi-transparent stripes, all overlapping in a
/// 192×192 center region. Opaques are sorted front-to-back for early-Z.
#[test]
fn p4_04_100_rects() {
    let mut scene = Scene::new(DIM, DIM);

    // Black opaque background.
    scene.push_rect(0.0, 0.0, 256.0, 256.0, [0.0, 0.0, 0.0, 1.0]);

    // 49 stripes: alternating opaque and 50%-transparent bands.
    // Each stripe is full-width within [32, 224], 4px tall.
    let y0 = 32.0_f32;
    for i in 0..49_u32 {
        let sy = y0 + (i as f32) * 4.0;
        let ey = sy + 4.0;
        if i % 2 == 0 {
            // Opaque: hue cycles R→G→B over 49 stripes.
            let t = i as f32 / 48.0;
            let (r, g, b) = if t < 0.5 {
                (1.0 - 2.0 * t, 2.0 * t, 0.0)
            } else {
                (0.0, 2.0 - 2.0 * t, 2.0 * t - 1.0)
            };
            scene.push_rect(32.0, sy, 224.0, ey, [r, g, b, 1.0]);
        } else {
            // 50% transparent white overlay.
            scene.push_rect(32.0, sy, 224.0, ey, [0.5, 0.5, 0.5, 0.5]);
        }
    }

    // Opaque white column on top of everything (rightmost 32px of the stripe band).
    scene.push_rect(192.0, y0, 224.0, y0 + 49.0 * 4.0, [1.0, 1.0, 1.0, 1.0]);

    run_scene_golden("p4_04_100_rects", scene);
}

/// Phase 2 regression: four-quadrant scene produces identical pixels
/// through the Phase 4 renderer. All rects are opaque; depth sort
/// must match painter's algorithm output.
#[test]
fn p4_05_phase2_regression() {
    let mut scene = Scene::new(DIM, DIM);
    scene.push_rect(0.0, 0.0, 128.0, 128.0, [1.0, 0.0, 0.0, 1.0]);
    scene.push_rect(128.0, 0.0, 256.0, 128.0, [0.0, 1.0, 0.0, 1.0]);
    scene.push_rect(0.0, 128.0, 128.0, 256.0, [0.0, 0.0, 1.0, 1.0]);
    scene.push_rect(128.0, 128.0, 256.0, 256.0, [1.0, 1.0, 0.0, 1.0]);
    run_scene_golden("p4_05_phase2_regression", scene);
}
