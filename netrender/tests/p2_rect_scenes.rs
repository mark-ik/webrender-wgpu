/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 2 golden harness — rect-only YAML scenes.
//!
//! Each test: load YAML → build Scene → Renderer::prepare →
//! Renderer::render → readback → pixel-diff against oracle PNG.
//!
//! Golden capture: set env var `NETRENDER_REGEN=1` to write PNGs
//! instead of comparing them. On the initial run (no oracle PNG),
//! the PNG is written automatically.
//!
//! Plan reference: design plan §5 Phase 2.
//! Receipt: 10 rect-only YAML scenes pixel-match captured PNGs.

use std::path::{Path, PathBuf};

use netrender::{ColorLoad, FrameTarget, NetrenderOptions, Scene, boot, create_netrender_instance};

const DIM: u32 = 256;
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

// ── YAML scene format ──────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct YamlScene {
    #[serde(default = "default_viewport")]
    viewport: [u32; 2],
    /// Clear color before drawing rects. Default: transparent.
    #[serde(default)]
    clear: [f32; 4],
    #[serde(default)]
    rects: Vec<YamlRect>,
}

fn default_viewport() -> [u32; 2] {
    [DIM, DIM]
}

#[derive(serde::Deserialize)]
struct YamlRect {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    color: [f32; 4],
}

// ── PNG helpers ────────────────────────────────────────────────────

fn oracle_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("oracle")
        .join("p2")
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).expect("create oracle/p2 dir");
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

fn run_scene_golden(name: &str, yaml_str: &str) {
    let yaml: YamlScene = serde_yaml::from_str(yaml_str)
        .unwrap_or_else(|e| panic!("YAML parse for {name}: {e}"));

    let [vw, vh] = yaml.viewport;
    let mut scene = Scene::new(vw, vh);
    for r in &yaml.rects {
        scene.push_rect(r.x0, r.y0, r.x1, r.y1, r.color);
    }

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
    let [cr, cg, cb, ca] = yaml.clear;
    renderer.render(
        &prepared,
        FrameTarget { view: &target_view, format: TARGET_FORMAT, width: vw, height: vh },
        ColorLoad::Clear(wgpu::Color {
            r: cr as f64, g: cg as f64, b: cb as f64, a: ca as f64,
        }),
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

// ── 10 rect-only scenes ────────────────────────────────────────────

#[test]
fn p2_01_solid_red() {
    run_scene_golden("p2_01_solid_red", r#"
viewport: [256, 256]
clear: [0.0, 0.0, 0.0, 0.0]
rects:
  - { x0: 0, y0: 0, x1: 256, y1: 256, color: [1.0, 0.0, 0.0, 1.0] }
"#);
}

#[test]
fn p2_02_solid_blue() {
    run_scene_golden("p2_02_solid_blue", r#"
viewport: [256, 256]
clear: [0.0, 0.0, 0.0, 0.0]
rects:
  - { x0: 0, y0: 0, x1: 256, y1: 256, color: [0.0, 0.0, 1.0, 1.0] }
"#);
}

#[test]
fn p2_03_solid_white() {
    run_scene_golden("p2_03_solid_white", r#"
viewport: [256, 256]
clear: [0.0, 0.0, 0.0, 0.0]
rects:
  - { x0: 0, y0: 0, x1: 256, y1: 256, color: [1.0, 1.0, 1.0, 1.0] }
"#);
}

#[test]
fn p2_04_blue_over_red() {
    // Red fills viewport first; blue 128×128 paints over top-left.
    run_scene_golden("p2_04_blue_over_red", r#"
viewport: [256, 256]
clear: [0.0, 0.0, 0.0, 0.0]
rects:
  - { x0: 0, y0: 0, x1: 256, y1: 256, color: [1.0, 0.0, 0.0, 1.0] }
  - { x0: 0, y0: 0, x1: 128, y1: 128, color: [0.0, 0.0, 1.0, 1.0] }
"#);
}

#[test]
fn p2_05_four_quadrants() {
    run_scene_golden("p2_05_four_quadrants", r#"
viewport: [256, 256]
clear: [0.0, 0.0, 0.0, 0.0]
rects:
  - { x0:   0, y0:   0, x1: 128, y1: 128, color: [1.0, 0.0, 0.0, 1.0] }
  - { x0: 128, y0:   0, x1: 256, y1: 128, color: [0.0, 1.0, 0.0, 1.0] }
  - { x0:   0, y0: 128, x1: 128, y1: 256, color: [0.0, 0.0, 1.0, 1.0] }
  - { x0: 128, y0: 128, x1: 256, y1: 256, color: [1.0, 1.0, 0.0, 1.0] }
"#);
}

#[test]
fn p2_06_vertical_halves() {
    run_scene_golden("p2_06_vertical_halves", r#"
viewport: [256, 256]
clear: [0.0, 0.0, 0.0, 0.0]
rects:
  - { x0:   0, y0: 0, x1: 128, y1: 256, color: [0.0, 1.0, 0.0, 1.0] }
  - { x0: 128, y0: 0, x1: 256, y1: 256, color: [1.0, 0.0, 1.0, 1.0] }
"#);
}

#[test]
fn p2_07_horizontal_stripes_4() {
    run_scene_golden("p2_07_horizontal_stripes_4", r#"
viewport: [256, 256]
clear: [0.0, 0.0, 0.0, 0.0]
rects:
  - { x0: 0, y0:   0, x1: 256, y1:  64, color: [1.0, 0.0, 0.0, 1.0] }
  - { x0: 0, y0:  64, x1: 256, y1: 128, color: [0.0, 1.0, 0.0, 1.0] }
  - { x0: 0, y0: 128, x1: 256, y1: 192, color: [0.0, 0.0, 1.0, 1.0] }
  - { x0: 0, y0: 192, x1: 256, y1: 256, color: [1.0, 1.0, 0.0, 1.0] }
"#);
}

#[test]
fn p2_08_checkerboard_2x2() {
    run_scene_golden("p2_08_checkerboard_2x2", r#"
viewport: [256, 256]
clear: [0.0, 0.0, 0.0, 0.0]
rects:
  - { x0:   0, y0:   0, x1: 128, y1: 128, color: [1.0, 1.0, 1.0, 1.0] }
  - { x0: 128, y0:   0, x1: 256, y1: 128, color: [0.0, 0.0, 0.0, 1.0] }
  - { x0:   0, y0: 128, x1: 128, y1: 256, color: [0.0, 0.0, 0.0, 1.0] }
  - { x0: 128, y0: 128, x1: 256, y1: 256, color: [1.0, 1.0, 1.0, 1.0] }
"#);
}

#[test]
fn p2_09_nested_3() {
    // White viewport, cyan inset, black innermost.
    run_scene_golden("p2_09_nested_3", r#"
viewport: [256, 256]
clear: [0.0, 0.0, 0.0, 0.0]
rects:
  - { x0:  0, y0:  0, x1: 256, y1: 256, color: [1.0, 1.0, 1.0, 1.0] }
  - { x0: 32, y0: 32, x1: 224, y1: 224, color: [0.0, 1.0, 1.0, 1.0] }
  - { x0: 64, y0: 64, x1: 192, y1: 192, color: [0.0, 0.0, 0.0, 1.0] }
"#);
}

#[test]
fn p2_10_sparse_corners() {
    // Four 32×32 red squares at each corner; transparent background.
    run_scene_golden("p2_10_sparse_corners", r#"
viewport: [256, 256]
clear: [0.0, 0.0, 0.0, 0.0]
rects:
  - { x0:   0, y0:   0, x1:  32, y1:  32, color: [1.0, 0.0, 0.0, 1.0] }
  - { x0: 224, y0:   0, x1: 256, y1:  32, color: [1.0, 0.0, 0.0, 1.0] }
  - { x0:   0, y0: 224, x1:  32, y1: 256, color: [1.0, 0.0, 0.0, 1.0] }
  - { x0: 224, y0: 224, x1: 256, y1: 256, color: [1.0, 0.0, 0.0, 1.0] }
"#);
}
