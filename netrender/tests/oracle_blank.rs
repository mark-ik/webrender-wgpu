/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! First scene-level golden test, moved from the device crate at
//! Phase 0.5 (renderer-side, since goldens are scene-level — see
//! design plan §5 Phase 0.5). Renders the `blank` oracle scene
//! (full-frame white clear at wrench's 3840×2160 hidpi default)
//! through the wgpu device path and pixel-diffs against the captured
//! oracle PNG. Tolerance: 0 (exact match expected — clear-to-white
//! is the simplest possible scene).
//!
//! The corpus at [`netrender/tests/oracle/`](oracle/) carries five
//! frozen PNG/YAML pairs from `upstream/0.68` GL captured 2026-04-28;
//! Phase 2 promotes them one at a time as their primitives ship
//! through netrender.

use std::path::Path;

use netrender_device::{ColorAttachment, RenderPassTarget, WgpuDevice};

fn load_oracle_png(name: &str) -> (u32, u32, Vec<u8>) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("oracle")
        .join(name);
    let file = std::fs::File::open(&path)
        .unwrap_or_else(|e| panic!("opening {}: {}", path.display(), e));
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder.read_info().expect("png read_info");
    let info = reader.info();
    assert_eq!(
        info.color_type,
        png::ColorType::Rgba,
        "oracle PNGs are expected to be RGBA",
    );
    assert_eq!(info.bit_depth, png::BitDepth::Eight);
    let (w, h) = (info.width, info.height);
    let mut buf = vec![0u8; reader.output_buffer_size()];
    reader.next_frame(&mut buf).expect("png decode frame");
    (w, h, buf)
}

fn count_pixel_diffs(actual: &[u8], expected: &[u8], tolerance: u8) -> usize {
    assert_eq!(actual.len(), expected.len());
    let mut diffs = 0;
    for (a, b) in actual.chunks_exact(4).zip(expected.chunks_exact(4)) {
        for c in 0..4 {
            if a[c].abs_diff(b[c]) > tolerance {
                diffs += 1;
                break;
            }
        }
    }
    diffs
}

#[test]
fn oracle_blank_smoke() {
    let (oracle_w, oracle_h, oracle_rgba) = load_oracle_png("blank.png");
    assert_eq!((oracle_w, oracle_h), (3840, 2160));

    let dev = WgpuDevice::boot().expect("wgpu boot");
    let target = dev.core.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("oracle blank target"),
        size: wgpu::Extent3d {
            width: oracle_w,
            height: oracle_h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = dev.create_encoder("oracle blank encoder");
    dev.encode_pass(
        &mut encoder,
        RenderPassTarget {
            label: "oracle blank pass",
            color: ColorAttachment::clear(&view, wgpu::Color::WHITE),
            depth: None,
        },
        &[],
    );
    dev.submit(encoder);

    let actual_rgba = dev.read_rgba8_texture(&target, oracle_w, oracle_h);
    let diffs = count_pixel_diffs(&actual_rgba, &oracle_rgba, 0);
    assert_eq!(
        diffs, 0,
        "blank scene must match oracle exactly (got {} pixel mismatches)",
        diffs
    );
}
