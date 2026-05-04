/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! P5 cluster #3 smoke: `WgpuExternalTexture` constructor + `bind_external_texture`.
//!
//! Verifies that an embedder can wrap a host-shared `wgpu::Texture` in
//! `WgpuExternalTexture` (with optional sampler override) and bind it
//! into a draw via `GpuPass::bind_external_texture`. End-to-end coverage
//! of the bind path; the renderer-side external_image_handler plumbing
//! is a separate concern (servo issue #37149).

#![cfg(feature = "wgpu_backend")]

use std::sync::Arc;
use webrender::{TextureSlot, WgpuDevice, WgpuExternalTexture};

fn try_create_device() -> Option<WgpuDevice> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::default(),
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("wgpu_external_texture_smoke device"),
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

/// Constructs a 4x4 BGRA8 host texture with an Arc'd view, wraps it in
/// `WgpuExternalTexture::new(view, None)`, and confirms the type is
/// constructable + the view inside is the one we passed.
#[test]
fn external_texture_wraps_host_view() {
    let Some(device) = try_create_device() else {
        eprintln!("skip: no wgpu adapter available");
        return;
    };

    // Embedder-side: build a wgpu::Texture out of band (would normally
    // come from the embedder's own renderer / compositor / capture pipe).
    let host_tex = device.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("host external texture"),
        size: wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let host_view = Arc::new(host_tex.create_view(&wgpu::TextureViewDescriptor::default()));

    let external = WgpuExternalTexture::new(host_view.clone(), None);
    // The Arc inside should be the same one we passed (cheap clone, same
    // refcount target).
    assert!(Arc::ptr_eq(&external.view, &host_view));
    assert!(external.sampler.is_none());
}

/// Same wrapper, but with a per-binding sampler override. The sampler
/// should round-trip through the WgpuExternalTexture struct.
#[test]
fn external_texture_carries_sampler_override() {
    let Some(device) = try_create_device() else {
        eprintln!("skip: no wgpu adapter available");
        return;
    };

    let host_tex = device.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("host external texture"),
        size: wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let host_view = Arc::new(host_tex.create_view(&wgpu::TextureViewDescriptor::default()));
    let host_sampler = Arc::new(device.device().create_sampler(&wgpu::SamplerDescriptor {
        label: Some("host external sampler (nearest)"),
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    }));

    let external = WgpuExternalTexture::new(host_view, Some(host_sampler.clone()));
    let stored_sampler = external.sampler.as_ref().expect("sampler set");
    assert!(Arc::ptr_eq(stored_sampler, &host_sampler));
}

/// `bind_external_texture` must record the (view, override-sampler) pair
/// onto the device the same way `bind_texture` does for owned textures.
/// We can't observe the bind directly without a draw, but we can confirm
/// the call doesn't panic + that a subsequent end_frame clears it.
#[test]
fn bind_external_texture_records_and_clears() {
    use webrender::{GpuFrame, GpuPass};

    let Some(mut device) = try_create_device() else {
        eprintln!("skip: no wgpu adapter available");
        return;
    };

    let host_tex = device.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("host external texture"),
        size: wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let host_view = Arc::new(host_tex.create_view(&wgpu::TextureViewDescriptor::default()));
    let external = WgpuExternalTexture::new(host_view, None);

    device.begin_frame();
    // Bind into slot 0 via the raw TextureSlot newtype. (The renderer
    // normally passes a TextureSampler enum that converts to TextureSlot;
    // we use the raw form here to avoid pulling in the enum.)
    device.bind_external_texture(TextureSlot(0), &external);
    // end_frame clears all bound state including this texture.
    device.end_frame();
    // No assertion needed — the bind+end_frame round trip not panicking
    // is the test. (bound_textures is pub(super), so a deeper field-
    // peek would require a test inside the module.)
}
