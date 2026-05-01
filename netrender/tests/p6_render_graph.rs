/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 6 receipt — render-task graph + separable Gaussian blur.
//!
//! Tests:
//!   p6_01_blur_uniform_source — uniform-color source is invariant under blur
//!   p6_02_drop_shadow         — blur a white square, composite as dark shadow
//!                               under the original rect; golden oracle
//!
//! Run `NETRENDER_REGEN=1 cargo test -p netrender p6_02` to capture the
//! oracle on first use, then subsequent runs diff within tolerance ±2/channel.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use netrender::{
    BrushBlurPipeline, ColorLoad, EncodeCallback, FrameTarget, ImageKey, NO_CLIP,
    NetrenderOptions, RenderGraph, Scene, Task, TaskId, boot, create_netrender_instance,
};

const DIM: u32 = 64;
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const BLUR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

// ── Helpers ────────────────────────────────────────────────────────────────

fn oracle_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("oracle")
        .join("p6")
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).expect("create oracle/p6 dir");
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

/// Upload CPU bytes as a `Rgba8Unorm` TEXTURE_BINDING texture.
fn upload_rgba8(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    bytes: &[u8],
) -> wgpu::Texture {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("p6 source"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    tex
}

/// Bilinear-clamp sampler for blur passes.
fn make_bilinear_sampler(device: &wgpu::Device) -> Arc<wgpu::Sampler> {
    Arc::new(device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("p6 bilinear clamp"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    }))
}

/// Build an encode callback for one separable Gaussian blur pass.
///
/// `step_x` / `step_y` are the texel-space offsets: `(1/w, 0)` for
/// horizontal, `(0, 1/h)` for vertical. The `BlurParams` uniform buffer
/// and bind group are created inside the closure (at encode time) using
/// the `&wgpu::Device` the graph passes in.
fn blur_pass_callback(
    pipe: BrushBlurPipeline,
    sampler: Arc<wgpu::Sampler>,
    step_x: f32,
    step_y: f32,
) -> EncodeCallback {
    // Pre-pack the 16-byte BlurParams struct; captured by copy.
    let mut step_bytes = [0u8; 16];
    step_bytes[0..4].copy_from_slice(&step_x.to_ne_bytes());
    step_bytes[4..8].copy_from_slice(&step_y.to_ne_bytes());

    Box::new(move |device, encoder, inputs, output| {
        assert!(!inputs.is_empty(), "blur task: expected at least one input view");
        let input_view = &inputs[0];

        // Upload step params via mapped-at-creation (no queue needed in callback).
        let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("blur params"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: true,
        });
        {
            let mut view = params_buf.slice(..).get_mapped_range_mut();
            view.copy_from_slice(&step_bytes);
        }
        params_buf.unmap();

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blur bind group"),
            layout: &pipe.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(input_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params_buf.as_entire_binding(),
                },
            ],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("blur pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipe.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..4, 0..1);
    })
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// Uniform color is invariant under Gaussian blur: output must equal input
/// within ±2/255 per channel (rounding only).
#[test]
fn p6_01_blur_uniform_source() {
    let handles = boot().expect("wgpu boot");
    let device = handles.device.clone();
    let queue = handles.queue.clone();
    let renderer = create_netrender_instance(handles, NetrenderOptions::default())
        .expect("create_netrender_instance");

    let src_bytes: Vec<u8> = (0..DIM * DIM).flat_map(|_| [255u8, 0, 0, 255]).collect();
    let src_tex = upload_rgba8(&device, &queue, DIM, DIM, &src_bytes);

    let pipe = renderer.wgpu_device.ensure_brush_blur(BLUR_FORMAT);
    let sampler = make_bilinear_sampler(&device);
    let step = 1.0 / DIM as f32;

    const SRC: TaskId = 0;
    const BLUR_H: TaskId = 1;
    const BLUR_V: TaskId = 2;

    let mut graph = RenderGraph::new();
    graph.push(Task {
        id: BLUR_H,
        extent: wgpu::Extent3d { width: DIM, height: DIM, depth_or_array_layers: 1 },
        format: BLUR_FORMAT,
        inputs: vec![SRC],
        encode: blur_pass_callback(pipe.clone(), Arc::clone(&sampler), step, 0.0),
    });
    graph.push(Task {
        id: BLUR_V,
        extent: wgpu::Extent3d { width: DIM, height: DIM, depth_or_array_layers: 1 },
        format: BLUR_FORMAT,
        inputs: vec![BLUR_H],
        encode: blur_pass_callback(pipe, Arc::clone(&sampler), 0.0, step),
    });

    let mut externals = HashMap::new();
    externals.insert(SRC, src_tex);
    let outputs = graph.execute(&device, &queue, externals);
    let blur_v = outputs.get(&BLUR_V).expect("BLUR_V output");

    let actual = renderer.wgpu_device.read_rgba8_texture(blur_v, DIM, DIM);
    assert_eq!(actual.len(), (DIM * DIM * 4) as usize);

    let mut max_diff: u8 = 0;
    for chunk in actual.chunks_exact(4) {
        let [r, g, b, a] = [chunk[0], chunk[1], chunk[2], chunk[3]];
        max_diff = max_diff.max((r as i16 - 255).unsigned_abs() as u8);
        max_diff = max_diff.max(g);
        max_diff = max_diff.max(b);
        max_diff = max_diff.max((a as i16 - 255).unsigned_abs() as u8);
    }
    assert!(
        max_diff <= 2,
        "uniform source not invariant under blur: max channel deviation = {}",
        max_diff
    );
}

/// Drop-shadow scene: blur a white square, composite as a dark overlay
/// under the original white rect. Oracle captured via NETRENDER_REGEN=1.
#[test]
fn p6_02_drop_shadow() {
    let handles = boot().expect("wgpu boot");
    let device = handles.device.clone();
    let queue = handles.queue.clone();
    let renderer = create_netrender_instance(handles, NetrenderOptions::default())
        .expect("create_netrender_instance");

    // White square (16,16)-(48,48) on transparent background.
    let src_bytes: Vec<u8> = (0..DIM * DIM)
        .flat_map(|i| {
            let x = (i % DIM) as i32;
            let y = (i / DIM) as i32;
            if x >= 16 && x < 48 && y >= 16 && y < 48 {
                [255u8, 255, 255, 255]
            } else {
                [0u8, 0, 0, 0]
            }
        })
        .collect();
    let src_tex = upload_rgba8(&device, &queue, DIM, DIM, &src_bytes);

    let pipe = renderer.wgpu_device.ensure_brush_blur(BLUR_FORMAT);
    let sampler = make_bilinear_sampler(&device);
    let step = 1.0 / DIM as f32;

    const SRC: TaskId = 10;
    const BLUR_H: TaskId = 11;
    const BLUR_V: TaskId = 12;

    let mut graph = RenderGraph::new();
    graph.push(Task {
        id: BLUR_H,
        extent: wgpu::Extent3d { width: DIM, height: DIM, depth_or_array_layers: 1 },
        format: BLUR_FORMAT,
        inputs: vec![SRC],
        encode: blur_pass_callback(pipe.clone(), Arc::clone(&sampler), step, 0.0),
    });
    graph.push(Task {
        id: BLUR_V,
        extent: wgpu::Extent3d { width: DIM, height: DIM, depth_or_array_layers: 1 },
        format: BLUR_FORMAT,
        inputs: vec![BLUR_H],
        encode: blur_pass_callback(pipe, Arc::clone(&sampler), 0.0, step),
    });

    let mut externals = HashMap::new();
    externals.insert(SRC, src_tex);
    let mut outputs = graph.execute(&device, &queue, externals);
    let blur_v = Arc::new(outputs.remove(&BLUR_V).expect("BLUR_V output"));

    const SHADOW_KEY: ImageKey = 0xDEAD_6666;
    renderer.insert_image_gpu(SHADOW_KEY, blur_v);

    // Composite: white fg rect + blurred shadow as dark overlay (offset +2,+2)
    // Shadow tint: premultiplied [0.1, 0.1, 0.1, 0.5]
    let mut scene = Scene::new(DIM, DIM);
    scene.push_rect(16.0, 16.0, 48.0, 48.0, [1.0, 1.0, 1.0, 1.0]);
    scene.push_image_full(
        18.0, 18.0, 50.0, 50.0,
        [0.0, 0.0, 1.0, 1.0],
        [0.1, 0.1, 0.1, 0.5],
        SHADOW_KEY,
        0,
        NO_CLIP,
    );

    let target_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("p6_02 target"),
        size: wgpu::Extent3d { width: DIM, height: DIM, depth_or_array_layers: 1 },
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
        FrameTarget { view: &target_view, format: TARGET_FORMAT, width: DIM, height: DIM },
        ColorLoad::Clear(wgpu::Color::BLACK),
    );

    let actual = renderer.wgpu_device.read_rgba8_texture(&target_tex, DIM, DIM);

    let oracle_path = oracle_dir().join("p6_02_drop_shadow.png");
    if should_regen() {
        write_png(&oracle_path, DIM, DIM, &actual);
        return;
    }

    let (ow, oh, expected) = read_png(&oracle_path);
    assert_eq!((ow, oh), (DIM, DIM), "oracle dimensions mismatch");
    assert_eq!(actual.len(), expected.len(), "pixel buffer length mismatch");

    let mut max_diff: u8 = 0;
    let mut diff_count = 0usize;
    for (a, e) in actual.chunks_exact(4).zip(expected.chunks_exact(4)) {
        for (&av, &ev) in a.iter().zip(e.iter()) {
            let d = (av as i16 - ev as i16).unsigned_abs() as u8;
            if d > 2 {
                diff_count += 1;
            }
            max_diff = max_diff.max(d);
        }
    }
    assert_eq!(
        diff_count, 0,
        "p6_02 drop-shadow: {} channel values differ by >2 (max diff = {}); \
         re-run with NETRENDER_REGEN=1 to update oracle",
        diff_count, max_diff
    );
}
