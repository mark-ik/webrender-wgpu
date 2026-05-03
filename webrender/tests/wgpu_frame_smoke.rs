/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! P4f smoke: WgpuDevice's GpuFrame methods return sensible values and
//! the begin_frame counter advances monotonically.

#![cfg(feature = "wgpu_backend")]

use api::ImageFormat;
use std::sync::Arc;
use webrender::{GpuFrame, WgpuDevice};

fn try_create_device() -> Option<WgpuDevice> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::default(),
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("wgpu_frame_smoke device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
        experimental_features: wgpu::ExperimentalFeatures::default(),
    }))
    .ok()?;
    Some(WgpuDevice::from_parts(
        Arc::new(instance),
        Arc::new(adapter),
        Arc::new(device),
        Arc::new(queue),
        None,
        None,
    ))
}

#[test]
fn frame_id_advances_monotonically() {
    let Some(mut wgpu_device) = try_create_device() else {
        eprintln!("skip: no wgpu adapter available");
        return;
    };
    let f0 = wgpu_device.begin_frame();
    wgpu_device.end_frame();
    let f1 = wgpu_device.begin_frame();
    wgpu_device.end_frame();
    let f2 = wgpu_device.begin_frame();
    wgpu_device.end_frame();
    assert_ne!(f0, f1);
    assert_ne!(f1, f2);
    assert_ne!(f0, f2);
}

#[test]
fn capability_queries_return_sensible_values() {
    let Some(wgpu_device) = try_create_device() else {
        eprintln!("skip: no wgpu adapter available");
        return;
    };
    let caps = wgpu_device.get_capabilities();
    // wgpu always supports these basics:
    assert!(caps.supports_multisampling);
    assert!(caps.supports_copy_image_sub_data);
    assert!(caps.supports_texture_usage);
    assert!(caps.supports_render_target_invalidate);
    // wgpu doesn't have these GL-only concepts:
    assert!(!caps.supports_qcom_tiled_rendering);
    assert!(!caps.supports_image_external_essl3);
    assert!(!caps.requires_vao_rebind_after_orphaning);
    // Renderer name is non-empty.
    assert!(!caps.renderer_name.is_empty());
}

#[test]
fn limits_and_alignment_are_reasonable() {
    let Some(wgpu_device) = try_create_device() else {
        eprintln!("skip: no wgpu adapter available");
        return;
    };
    // Min spec wgpu max_texture_dimension_2d is 2048; most adapters give 8192+.
    assert!(wgpu_device.max_texture_size() >= 2048);
    // Surface origin is top-left in wgpu (always).
    assert!(wgpu_device.surface_origin_is_top_left());
    // Preferred format BGRA8 (most universally supported color attachment).
    let pair = wgpu_device.preferred_color_formats();
    assert_eq!(pair.internal, ImageFormat::BGRA8);
    assert_eq!(pair.external, ImageFormat::BGRA8);
    // Required PBO stride: 256 bytes (wgpu COPY_BYTES_PER_ROW_ALIGNMENT).
    let stride = wgpu_device.required_pbo_stride();
    assert_eq!(stride.num_bytes(ImageFormat::R8).get(), 256);
    // Depth bits = 24 (Depth24Plus).
    assert_eq!(wgpu_device.depth_bits(), 24);
    // No GL extensions reported.
    assert!(!wgpu_device.supports_extension("GL_KHR_debug"));
    assert!(!wgpu_device.supports_extension("anything"));
}
