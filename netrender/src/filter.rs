/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 11c' — render-graph encode callbacks for the WGSL filter
//! pipelines (`cs_clip_rectangle` mask + separable `brush_blur`)
//! that produce intermediate textures consumed by vello scenes via
//! `Renderer::insert_image_vello`.
//!
//! These were previously test-only helpers under `tests/common/`;
//! promoted here once the box-shadow helper made them production
//! callers. The high-level `Renderer::build_box_shadow_mask`
//! orchestration that consumes them lives in `renderer/mod.rs`.

use std::sync::Arc;

use netrender_device::{BrushBlurPipeline, ClipRectanglePipeline};

use crate::render_graph::EncodeCallback;

/// Bilinear-clamp sampler. `brush_blur` and other filter passes use
/// this to sample their input textures.
pub fn make_bilinear_sampler(device: &wgpu::Device) -> Arc<wgpu::Sampler> {
    Arc::new(device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("netrender bilinear clamp"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    }))
}

/// Build a render-graph encode callback that runs `cs_clip_rectangle`
/// to produce a rounded-rect coverage mask. `bounds` are in
/// target-pixel space; `corner_radius` is uniform on all four
/// corners (the underlying WGSL accepts per-corner radii but the
/// `vec4` slot is filled with `radius` in this helper for parity
/// with the original test usage).
pub fn clip_rectangle_callback(
    pipe: ClipRectanglePipeline,
    bounds: [f32; 4],
    corner_radius: f32,
) -> EncodeCallback {
    // ClipParams: bounds (vec4) + radii (vec4) = 32 bytes.
    let mut bytes = [0u8; 32];
    for (i, f) in bounds.iter().enumerate() {
        bytes[i * 4..(i + 1) * 4].copy_from_slice(&f.to_ne_bytes());
    }
    for i in 4..8 {
        bytes[i * 4..(i + 1) * 4].copy_from_slice(&corner_radius.to_ne_bytes());
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

/// Build a render-graph encode callback for one `brush_blur` pass.
/// `step_x` / `step_y` are the texel-space sample offsets:
/// `(1/W, 0)` for horizontal, `(0, 1/H)` for vertical. Run two
/// callbacks back-to-back (H then V) for a separable 2-D blur.
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
