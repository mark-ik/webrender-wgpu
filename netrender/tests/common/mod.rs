/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Shared test helpers for the integration tests in this directory.
//!
//! Each `tests/<name>.rs` file is its own crate, so any helper used
//! by more than one test file gets duplicated unless it lives in a
//! shared module under `tests/common/`. Cargo treats `tests/common/`
//! specially — files inside it are NOT compiled as separate test
//! binaries; they're available only via `mod common;` in a test file.
//!
//! Helpers here are `#[allow(dead_code)]` because each consuming
//! test file pulls in only a subset, and the unused-fn lint
//! otherwise fires per file.

#![allow(dead_code)]

use std::sync::Arc;

use netrender::{BrushBlurPipeline, ClipRectanglePipeline, EncodeCallback};

/// Bilinear-clamp sampler. Used by `brush_blur` and any other test
/// that needs a filtering sampler over a `filterable: true` texture.
pub fn make_bilinear_sampler(device: &wgpu::Device) -> Arc<wgpu::Sampler> {
    Arc::new(device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("test bilinear clamp"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    }))
}

/// Build an encode callback that runs `cs_clip_rectangle` with the
/// given `bounds` (target-pixel space) and uniform corner radius.
/// Used by `p9a` / `p9b` / `p9c` clip-mask tests.
pub fn clip_rectangle_callback(
    pipe: ClipRectanglePipeline,
    bounds: [f32; 4],
    radius: f32,
) -> EncodeCallback {
    // ClipParams: bounds (vec4) + radii (vec4) = 32 bytes.
    let mut bytes = [0u8; 32];
    for (i, f) in bounds.iter().enumerate() {
        bytes[i * 4..(i + 1) * 4].copy_from_slice(&f.to_ne_bytes());
    }
    for i in 4..8 {
        bytes[i * 4..(i + 1) * 4].copy_from_slice(&radius.to_ne_bytes());
    }

    Box::new(move |device, encoder, _inputs, output| {
        let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("clip_rectangle params"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: true,
        });
        {
            let mut view = params_buf.slice(..).get_mapped_range_mut();
            view.copy_from_slice(&bytes);
        }
        params_buf.unmap();

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("clip_rectangle bind group"),
            layout: &pipe.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buf.as_entire_binding(),
            }],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("clip_rectangle pass"),
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

/// Build an encode callback for a single `brush_blur` pass.
/// `step_x` / `step_y` are the texel-space offsets:
/// `(1/w, 0)` for horizontal, `(0, 1/h)` for vertical.
pub fn blur_pass_callback(
    pipe: BrushBlurPipeline,
    sampler: Arc<wgpu::Sampler>,
    step_x: f32,
    step_y: f32,
) -> EncodeCallback {
    let mut step_bytes = [0u8; 16];
    step_bytes[0..4].copy_from_slice(&step_x.to_ne_bytes());
    step_bytes[4..8].copy_from_slice(&step_y.to_ne_bytes());

    Box::new(move |device, encoder, inputs, output| {
        assert!(!inputs.is_empty(), "blur task: expected one input view");
        let input_view = &inputs[0];

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
