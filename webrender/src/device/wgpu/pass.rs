/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Render-pass batching. Ingests `DrawIntent`s; flushes per pass; one
//! `BeginRenderPass` per target switch. See plan §4.8, §6 S1.

use std::ops::Range;

/// Recorded but not-yet-executed draw. Display-list traversal records
/// these into per-pass buckets; `flush_pass` flips them into wgpu calls
/// inside a single render-pass scope (per §4.8 — record, never execute
/// inline).
pub struct DrawIntent {
    pub vertex_range: Range<u32>,
    pub instance_range: Range<u32>,
    /// Dynamic offset into the bound uniform arena (per §4.7).
    pub uniform_offset: u32,
    /// Push-constant payload (per §4.7); stage VERTEX. Empty if the
    /// pipeline has no push-constant range.
    pub push_constants: Vec<u8>,
}

/// Flush a list of draw intents into a single render pass with one
/// pipeline binding. One `BeginRenderPass` per call; pipeline switches
/// inside a pass are S6's job once multiple shader families exist.
pub fn flush_pass(
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
    clear: wgpu::Color,
    label: &str,
    draws: &[DrawIntent],
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(clear),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipeline);
    for draw in draws {
        pass.set_bind_group(0, bind_group, &[draw.uniform_offset]);
        if !draw.push_constants.is_empty() {
            // wgpu 29: `set_immediates(offset, data)` — stage is fixed
            // by the pipeline's `immediate_size` declaration; no stage
            // arg here.
            pass.set_immediates(0, &draw.push_constants);
        }
        pass.draw(draw.vertex_range.clone(), draw.instance_range.clone());
    }
}
