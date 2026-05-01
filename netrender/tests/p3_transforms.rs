/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 3 golden harness — transforms + axis-aligned clip.
//!
//! Receipt: scene with one transform chain (translate + rotate + scale)
//! + one axis-aligned clip rectangle pixel-matches reference.
//!
//! Each test: build Scene → Renderer::prepare → Renderer::render →
//! readback → pixel-diff against oracle PNG.
//!
//! Golden capture: set env var `NETRENDER_REGEN=1` to write PNGs
//! instead of comparing them. On the initial run (no oracle PNG),
//! the PNG is written automatically.
//!
//! Also includes a Phase 2 regression test to verify that scenes built
//! via `push_rect` (identity transform, no clip) produce identical
//! output to their Phase 2 golden counterparts.

use std::f32::consts::PI;
use std::path::{Path, PathBuf};

use netrender::{
    ColorLoad, FrameTarget, NetrenderOptions, Scene, Transform, boot, create_netrender_instance,
};

const DIM: u32 = 256;
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

// ── PNG helpers (duplicated from p2; shared test-util is Phase 4) ──

fn oracle_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("oracle")
        .join("p3")
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).expect("create oracle/p3 dir");
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

// ── Core test runner ───────────────────────────────────────────────

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

// ── Phase 2 regression — identity transform / no clip ─────────────

#[test]
fn p3_00_p2_regression_solid_red() {
    // Same scene as p2_01_solid_red; Phase 3 shader must produce
    // identical output (identity transform, NO_CLIP).
    let mut scene = Scene::new(DIM, DIM);
    scene.push_rect(0.0, 0.0, 256.0, 256.0, [1.0, 0.0, 0.0, 1.0]);
    run_scene_golden("p3_00_p2_regression_solid_red", scene);
}

// ── Translate ──────────────────────────────────────────────────────

#[test]
fn p3_01_translate_offset() {
    // 64×64 red rect translated to (96, 96); black background.
    let mut scene = Scene::new(DIM, DIM);
    let tid = scene.push_transform(Transform::translate_2d(96.0, 96.0));
    scene.push_rect_transformed(0.0, 0.0, 64.0, 64.0, [1.0, 0.0, 0.0, 1.0], tid);
    run_scene_golden("p3_01_translate_offset", scene);
}

// ── Scale ──────────────────────────────────────────────────────────

#[test]
fn p3_02_scale_up() {
    // Unit rect scaled to 128×128; black background.
    let mut scene = Scene::new(DIM, DIM);
    let tid = scene.push_transform(Transform::scale_2d(128.0, 128.0));
    scene.push_rect_transformed(0.0, 0.0, 1.0, 1.0, [0.0, 1.0, 0.0, 1.0], tid);
    run_scene_golden("p3_02_scale_up", scene);
}

// ── Axis-aligned clip ──────────────────────────────────────────────

#[test]
fn p3_03_clip_center() {
    // Full-screen red rect clipped to the center 128×128 region.
    let mut scene = Scene::new(DIM, DIM);
    scene.push_rect_clipped(
        0.0, 0.0, 256.0, 256.0,
        [1.0, 0.0, 0.0, 1.0],
        0, // identity
        [64.0, 64.0, 192.0, 192.0],
    );
    run_scene_golden("p3_03_clip_center", scene);
}

// ── Rotation ──────────────────────────────────────────────────────

#[test]
fn p3_04_rotate_45_diamond() {
    // 90×90 white rect centered at origin, rotated 45°, then translated
    // to the viewport center — produces a diamond silhouette.
    let mut scene = Scene::new(DIM, DIM);
    let tid = scene.push_transform(
        Transform::translate_2d(-45.0, -45.0) // center rect at origin
            .then(&Transform::rotate_2d(PI / 4.0))
            .then(&Transform::translate_2d(128.0, 128.0)),
    );
    scene.push_rect_transformed(0.0, 0.0, 90.0, 90.0, [1.0, 1.0, 1.0, 1.0], tid);
    run_scene_golden("p3_04_rotate_45_diamond", scene);
}

// ── Transform chain: translate + rotate + scale ────────────────────
// This test is the Phase 3 receipt.

#[test]
fn p3_05_chain_trs() {
    // Receipt: one scene with translate + rotate + scale chained.
    // Build a 1×1 white rect, scale 64×, rotate 30°, translate to (128,96).
    // Then clip to a 160×160 window centered at (128,128).
    let mut scene = Scene::new(DIM, DIM);
    let tid = scene.push_transform(
        Transform::scale_2d(64.0, 64.0)               // scale first (inner)
            .then(&Transform::rotate_2d(PI / 6.0))    // then rotate 30°
            .then(&Transform::translate_2d(128.0, 96.0)), // then translate (outer)
    );
    scene.push_rect_clipped(
        0.0, 0.0, 1.0, 1.0,
        [1.0, 1.0, 0.0, 1.0], // yellow
        tid,
        [48.0, 48.0, 208.0, 208.0],
    );
    run_scene_golden("p3_05_chain_trs", scene);
}

// ── Two-layer scene: untransformed + transformed ───────────────────

#[test]
fn p3_06_two_layers() {
    // Blue background (identity), red 64×64 translated rect on top.
    let mut scene = Scene::new(DIM, DIM);
    scene.push_rect(0.0, 0.0, 256.0, 256.0, [0.0, 0.0, 1.0, 1.0]);
    let tid = scene.push_transform(Transform::translate_2d(96.0, 96.0));
    scene.push_rect_transformed(0.0, 0.0, 64.0, 64.0, [1.0, 0.0, 0.0, 1.0], tid);
    run_scene_golden("p3_06_two_layers", scene);
}
