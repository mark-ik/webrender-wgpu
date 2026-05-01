/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 4 / Phase 5 batch builders.
//!
//! **Rects** (Phase 4): classifies, sorts, and uploads solid-color rects.
//! Opaques → front-to-back, depth write ON. Alphas → painter order, blend.
//!
//! **Images** (Phase 5): classifies and uploads textured rects.
//! Same depth / blend policy as rects. Grouped by `ImageKey` so each
//! unique texture gets exactly one `DrawIntent` per pipeline variant.
//!
//! Z assignment — unified across rects and images (images are "on top"):
//!   N_total = N_rects + N_images
//!   Rect at painter index i  → z = (N_total − i)        / (N_total + 1)
//!   Image at painter index j → z = (N_total − N_rects − j) / (N_total + 1)
//! Front rects (high painter index) → small z (near). Back → large z (far).

use std::collections::HashMap;

use netrender_device::{
    BrushConicGradientPipeline, BrushImagePipeline, BrushLinearGradientPipeline,
    BrushRadialGradientPipeline, BrushRectSolidPipeline, DrawIntent,
};

use crate::image_cache::ImageCache;
use crate::scene::{ImageKey, Scene};

// ── Shared frame resources ────────────────────────────────────────────

/// GPU buffers that are identical for every draw call in a frame:
/// the transform palette and the orthographic per-frame uniform.
/// Built once in `prepare()` and passed by reference to both batch
/// builders so each frame allocates exactly two shared buffers instead
/// of one pair per batch type.
pub(crate) struct FrameResources {
    pub transforms: wgpu::Buffer,
    pub per_frame: wgpu::Buffer,
}

impl FrameResources {
    pub fn new(scene: &Scene, device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            transforms: make_transforms_buf(scene, device, queue),
            per_frame: make_per_frame_buf(scene, device, queue),
        }
    }
}

// ── Rect batch ────────────────────────────────────────────────────────

/// Build all [`DrawIntent`]s for solid-color rects in `scene`.
/// Opaques first (front-to-back), then alphas (back-to-front).
pub(crate) fn build_rect_batch(
    scene: &Scene,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    opaque_pipe: &BrushRectSolidPipeline,
    alpha_pipe: &BrushRectSolidPipeline,
    frame_res: &FrameResources,
) -> Vec<DrawIntent> {
    if scene.rects.is_empty() {
        return Vec::new();
    }

    // Unified depth range shared with image + all gradient batches.
    let n_total = scene.rects.len()
        + scene.images.len()
        + scene.linear_gradients.len()
        + scene.radial_gradients.len()
        + scene.conic_gradients.len();

    let mut opaque_order: Vec<(usize, f32)> = Vec::new();
    let mut alpha_order: Vec<(usize, f32)> = Vec::new();

    for (i, r) in scene.rects.iter().enumerate() {
        let z = (n_total - i) as f32 / (n_total + 1) as f32;
        if r.color[3] >= 1.0 {
            opaque_order.push((i, z));
        } else {
            alpha_order.push((i, z));
        }
    }

    // Opaques: ascending z = front first → early-Z benefit.
    opaque_order.sort_unstable_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

    let build_batch = |order: &[(usize, f32)], pipe: &BrushRectSolidPipeline| -> DrawIntent {
        let instance_count = order.len() as u32;
        let mut bytes: Vec<u8> = Vec::with_capacity(order.len() * 64);
        for &(idx, z) in order {
            let r = &scene.rects[idx];
            for f in [r.x0, r.y0, r.x1, r.y1] {
                bytes.extend_from_slice(&f.to_ne_bytes());
            }
            for f in r.color {
                bytes.extend_from_slice(&f.to_ne_bytes());
            }
            for f in r.clip_rect {
                bytes.extend_from_slice(&f.to_ne_bytes());
            }
            bytes.extend_from_slice(&r.transform_id.to_ne_bytes());
            bytes.extend_from_slice(&z.to_ne_bytes());
            bytes.extend_from_slice(&[0u8; 8]); // padding → 64 bytes
        }
        let instances_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brush_rect_solid instances"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&instances_buf, 0, &bytes);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush_rect_solid bind group"),
            layout: &pipe.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: instances_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: frame_res.transforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: frame_res.per_frame.as_entire_binding() },
            ],
        });

        DrawIntent {
            pipeline: pipe.pipeline.clone(),
            bind_group,
            vertex_buffers: vec![],
            vertex_range: 0..4,
            instance_range: 0..instance_count,
            dynamic_offsets: Vec::new(),
            push_constants: Vec::new(),
        }
    };

    let mut draws = Vec::new();
    if !opaque_order.is_empty() {
        draws.push(build_batch(&opaque_order, opaque_pipe));
    }
    if !alpha_order.is_empty() {
        draws.push(build_batch(&alpha_order, alpha_pipe));
    }
    draws
}

// ── Image batch ───────────────────────────────────────────────────────

/// Build all [`DrawIntent`]s for textured-rect (`SceneImage`) entries.
/// Opaques first (front-to-back, grouped by key), then alphas (painter
/// order, grouped by key). Returns empty vec when `scene.images` is empty.
pub(crate) fn build_image_batch(
    scene: &Scene,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    opaque_pipe: &BrushImagePipeline,
    alpha_pipe: &BrushImagePipeline,
    image_cache: &ImageCache,
    sampler: &wgpu::Sampler,
    frame_res: &FrameResources,
) -> Vec<DrawIntent> {
    if scene.images.is_empty() {
        return Vec::new();
    }

    let n_rects = scene.rects.len();
    let n_total = n_rects
        + scene.images.len()
        + scene.linear_gradients.len()
        + scene.radial_gradients.len()
        + scene.conic_gradients.len();

    // Classify: (painter_index_j, z, key)
    let mut opaque_items: Vec<(usize, f32, ImageKey)> = Vec::new();
    let mut alpha_items: Vec<(usize, f32, ImageKey)> = Vec::new();

    for (j, img) in scene.images.iter().enumerate() {
        let global_idx = n_rects + j;
        let z = (n_total - global_idx) as f32 / (n_total + 1) as f32;
        if img.color[3] >= 1.0 {
            opaque_items.push((j, z, img.key));
        } else {
            alpha_items.push((j, z, img.key));
        }
    }

    // Opaques: sort front-to-back (ascending z).
    opaque_items.sort_unstable_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    // Alphas: keep original painter order.

    let mut draws = Vec::new();
    emit_image_draws(
        &opaque_items, scene, device, queue, opaque_pipe,
        image_cache, sampler, frame_res, &mut draws,
    );
    emit_image_draws(
        &alpha_items, scene, device, queue, alpha_pipe,
        image_cache, sampler, frame_res, &mut draws,
    );
    draws
}

/// Emit one [`DrawIntent`] per unique `ImageKey` in `items`, maintaining
/// the relative ordering of instances within each key group.
fn emit_image_draws(
    items: &[(usize, f32, ImageKey)],
    scene: &Scene,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipe: &BrushImagePipeline,
    image_cache: &ImageCache,
    sampler: &wgpu::Sampler,
    frame_res: &FrameResources,
    out: &mut Vec<DrawIntent>,
) {
    if items.is_empty() {
        return;
    }

    // Group by key, preserving first-seen order (use Vec as ordered map).
    let mut groups: Vec<(ImageKey, Vec<(usize, f32)>)> = Vec::new();
    let mut key_to_group: HashMap<ImageKey, usize> = HashMap::new();
    for &(j, z, key) in items {
        if let Some(&gi) = key_to_group.get(&key) {
            groups[gi].1.push((j, z));
        } else {
            let gi = groups.len();
            key_to_group.insert(key, gi);
            groups.push((key, vec![(j, z)]));
        }
    }

    for (key, group_items) in &groups {
        let texture = match image_cache.get(*key) {
            Some(t) => t,
            None => continue, // key registered but not yet uploaded; skip
        };
        let tex_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Build 80-byte-stride instance buffer.
        let instance_count = group_items.len() as u32;
        let mut bytes: Vec<u8> = Vec::with_capacity(group_items.len() * 80);
        for &(j, z) in group_items {
            let img = &scene.images[j];
            // rect (16 bytes)
            for f in [img.x0, img.y0, img.x1, img.y1] {
                bytes.extend_from_slice(&f.to_ne_bytes());
            }
            // uv_rect (16 bytes)
            for f in img.uv {
                bytes.extend_from_slice(&f.to_ne_bytes());
            }
            // color (16 bytes)
            for f in img.color {
                bytes.extend_from_slice(&f.to_ne_bytes());
            }
            // clip (16 bytes)
            for f in img.clip_rect {
                bytes.extend_from_slice(&f.to_ne_bytes());
            }
            // transform_id (4 bytes)
            bytes.extend_from_slice(&img.transform_id.to_ne_bytes());
            // z_depth (4 bytes)
            bytes.extend_from_slice(&z.to_ne_bytes());
            // padding (8 bytes) → stride 80
            bytes.extend_from_slice(&[0u8; 8]);
        }

        let instances_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brush_image instances"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&instances_buf, 0, &bytes);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush_image bind group"),
            layout: &pipe.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: instances_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: frame_res.transforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: frame_res.per_frame.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&tex_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });

        out.push(DrawIntent {
            pipeline: pipe.pipeline.clone(),
            bind_group,
            vertex_buffers: vec![],
            vertex_range: 0..4,
            instance_range: 0..instance_count,
            dynamic_offsets: Vec::new(),
            push_constants: Vec::new(),
        });
    }
}

// ── Linear gradient batch (Phase 8A) ──────────────────────────────────

/// Build all [`DrawIntent`]s for 2-stop linear gradients in `scene`.
/// Opaque (both stops alpha >= 1.0) first, sorted front-to-back; then
/// alpha (any stop with alpha < 1.0) in painter order.
///
/// Z assignment: linear gradients occupy painter indices
/// `[n_rects + n_images, n_rects + n_images + n_linear)`, smaller z
/// (= nearer) than rects and images.
pub(crate) fn build_linear_gradient_batch(
    scene: &Scene,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    opaque_pipe: &BrushLinearGradientPipeline,
    alpha_pipe: &BrushLinearGradientPipeline,
    frame_res: &FrameResources,
) -> Vec<DrawIntent> {
    if scene.linear_gradients.is_empty() {
        return Vec::new();
    }

    let n_rects = scene.rects.len();
    let n_images = scene.images.len();
    let n_linear = scene.linear_gradients.len();
    let n_total =
        n_rects + n_images + n_linear + scene.radial_gradients.len() + scene.conic_gradients.len();

    let mut opaque_order: Vec<(usize, f32)> = Vec::new();
    let mut alpha_order: Vec<(usize, f32)> = Vec::new();

    for (k, g) in scene.linear_gradients.iter().enumerate() {
        let global_idx = n_rects + n_images + k;
        let z = (n_total - global_idx) as f32 / (n_total + 1) as f32;
        if g.color0[3] >= 1.0 && g.color1[3] >= 1.0 {
            opaque_order.push((k, z));
        } else {
            alpha_order.push((k, z));
        }
    }

    // Opaques: ascending z = front first → early-Z benefit.
    opaque_order.sort_unstable_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

    let build_batch =
        |order: &[(usize, f32)], pipe: &BrushLinearGradientPipeline| -> DrawIntent {
            let instance_count = order.len() as u32;
            let mut bytes: Vec<u8> = Vec::with_capacity(order.len() * 96);
            for &(idx, z) in order {
                let g = &scene.linear_gradients[idx];
                // rect (16)
                for f in [g.x0, g.y0, g.x1, g.y1] {
                    bytes.extend_from_slice(&f.to_ne_bytes());
                }
                // line: start.xy, end.xy (16)
                for f in [g.start_point[0], g.start_point[1], g.end_point[0], g.end_point[1]] {
                    bytes.extend_from_slice(&f.to_ne_bytes());
                }
                // color0 (16)
                for f in g.color0 {
                    bytes.extend_from_slice(&f.to_ne_bytes());
                }
                // color1 (16)
                for f in g.color1 {
                    bytes.extend_from_slice(&f.to_ne_bytes());
                }
                // clip (16)
                for f in g.clip_rect {
                    bytes.extend_from_slice(&f.to_ne_bytes());
                }
                // transform_id (4)
                bytes.extend_from_slice(&g.transform_id.to_ne_bytes());
                // z_depth (4)
                bytes.extend_from_slice(&z.to_ne_bytes());
                // padding (8) → stride 96
                bytes.extend_from_slice(&[0u8; 8]);
            }
            let instances_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("brush_linear_gradient instances"),
                size: bytes.len() as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&instances_buf, 0, &bytes);

            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("brush_linear_gradient bind group"),
                layout: &pipe.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: instances_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: frame_res.transforms.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: frame_res.per_frame.as_entire_binding() },
                ],
            });

            DrawIntent {
                pipeline: pipe.pipeline.clone(),
                bind_group,
                vertex_buffers: vec![],
                vertex_range: 0..4,
                instance_range: 0..instance_count,
                dynamic_offsets: Vec::new(),
                push_constants: Vec::new(),
            }
        };

    let mut draws = Vec::new();
    if !opaque_order.is_empty() {
        draws.push(build_batch(&opaque_order, opaque_pipe));
    }
    if !alpha_order.is_empty() {
        draws.push(build_batch(&alpha_order, alpha_pipe));
    }
    draws
}

// ── Radial gradient batch (Phase 8B) ──────────────────────────────────

/// Build all [`DrawIntent`]s for 2-stop radial gradients in `scene`.
///
/// Same opaque/alpha bucketing and z-sort shape as the linear-gradient
/// batch. Z range: painter indices
/// `[n_rects + n_images + n_linear, n_total)` — radial gradients paint
/// in front of every other primitive family in Phase 8B (this
/// linear-then-radial ordering is documented as a Phase 8 limitation;
/// 8D's unified gradient list will preserve user push order).
pub(crate) fn build_radial_gradient_batch(
    scene: &Scene,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    opaque_pipe: &BrushRadialGradientPipeline,
    alpha_pipe: &BrushRadialGradientPipeline,
    frame_res: &FrameResources,
) -> Vec<DrawIntent> {
    if scene.radial_gradients.is_empty() {
        return Vec::new();
    }

    let n_rects = scene.rects.len();
    let n_images = scene.images.len();
    let n_linear = scene.linear_gradients.len();
    let n_radial = scene.radial_gradients.len();
    let n_total = n_rects + n_images + n_linear + n_radial + scene.conic_gradients.len();

    let mut opaque_order: Vec<(usize, f32)> = Vec::new();
    let mut alpha_order: Vec<(usize, f32)> = Vec::new();

    for (k, g) in scene.radial_gradients.iter().enumerate() {
        let global_idx = n_rects + n_images + n_linear + k;
        let z = (n_total - global_idx) as f32 / (n_total + 1) as f32;
        if g.color0[3] >= 1.0 && g.color1[3] >= 1.0 {
            opaque_order.push((k, z));
        } else {
            alpha_order.push((k, z));
        }
    }

    opaque_order.sort_unstable_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

    let build_batch =
        |order: &[(usize, f32)], pipe: &BrushRadialGradientPipeline| -> DrawIntent {
            let instance_count = order.len() as u32;
            let mut bytes: Vec<u8> = Vec::with_capacity(order.len() * 96);
            for &(idx, z) in order {
                let g = &scene.radial_gradients[idx];
                // rect (16)
                for f in [g.x0, g.y0, g.x1, g.y1] {
                    bytes.extend_from_slice(&f.to_ne_bytes());
                }
                // params: center.xy, radii.xy (16)
                for f in [g.center[0], g.center[1], g.radii[0], g.radii[1]] {
                    bytes.extend_from_slice(&f.to_ne_bytes());
                }
                // color0 (16)
                for f in g.color0 {
                    bytes.extend_from_slice(&f.to_ne_bytes());
                }
                // color1 (16)
                for f in g.color1 {
                    bytes.extend_from_slice(&f.to_ne_bytes());
                }
                // clip (16)
                for f in g.clip_rect {
                    bytes.extend_from_slice(&f.to_ne_bytes());
                }
                bytes.extend_from_slice(&g.transform_id.to_ne_bytes());
                bytes.extend_from_slice(&z.to_ne_bytes());
                bytes.extend_from_slice(&[0u8; 8]); // pad → stride 96
            }
            let instances_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("brush_radial_gradient instances"),
                size: bytes.len() as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&instances_buf, 0, &bytes);

            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("brush_radial_gradient bind group"),
                layout: &pipe.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: instances_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: frame_res.transforms.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: frame_res.per_frame.as_entire_binding() },
                ],
            });

            DrawIntent {
                pipeline: pipe.pipeline.clone(),
                bind_group,
                vertex_buffers: vec![],
                vertex_range: 0..4,
                instance_range: 0..instance_count,
                dynamic_offsets: Vec::new(),
                push_constants: Vec::new(),
            }
        };

    let mut draws = Vec::new();
    if !opaque_order.is_empty() {
        draws.push(build_batch(&opaque_order, opaque_pipe));
    }
    if !alpha_order.is_empty() {
        draws.push(build_batch(&alpha_order, alpha_pipe));
    }
    draws
}

// ── Conic gradient batch (Phase 8C) ───────────────────────────────────

/// Build all [`DrawIntent`]s for 2-stop conic gradients in `scene`.
///
/// Same opaque/alpha bucketing as the linear and radial batches.
/// Z range: painter indices `[n_rects + n_images + n_linear + n_radial,
/// n_total)` — conic gradients paint in front of every other family
/// in Phase 8C (this conic-on-top ordering is documented as a Phase 8
/// limitation; 8D's unified gradient list will preserve user push
/// order across all gradient kinds).
pub(crate) fn build_conic_gradient_batch(
    scene: &Scene,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    opaque_pipe: &BrushConicGradientPipeline,
    alpha_pipe: &BrushConicGradientPipeline,
    frame_res: &FrameResources,
) -> Vec<DrawIntent> {
    if scene.conic_gradients.is_empty() {
        return Vec::new();
    }

    let n_rects = scene.rects.len();
    let n_images = scene.images.len();
    let n_linear = scene.linear_gradients.len();
    let n_radial = scene.radial_gradients.len();
    let n_total = n_rects + n_images + n_linear + n_radial + scene.conic_gradients.len();

    let mut opaque_order: Vec<(usize, f32)> = Vec::new();
    let mut alpha_order: Vec<(usize, f32)> = Vec::new();

    for (k, g) in scene.conic_gradients.iter().enumerate() {
        let global_idx = n_rects + n_images + n_linear + n_radial + k;
        let z = (n_total - global_idx) as f32 / (n_total + 1) as f32;
        if g.color0[3] >= 1.0 && g.color1[3] >= 1.0 {
            opaque_order.push((k, z));
        } else {
            alpha_order.push((k, z));
        }
    }

    opaque_order.sort_unstable_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

    let build_batch =
        |order: &[(usize, f32)], pipe: &BrushConicGradientPipeline| -> DrawIntent {
            let instance_count = order.len() as u32;
            let mut bytes: Vec<u8> = Vec::with_capacity(order.len() * 96);
            for &(idx, z) in order {
                let g = &scene.conic_gradients[idx];
                // rect (16)
                for f in [g.x0, g.y0, g.x1, g.y1] {
                    bytes.extend_from_slice(&f.to_ne_bytes());
                }
                // params: center.xy, start_angle, _pad (16)
                for f in [g.center[0], g.center[1], g.start_angle, 0.0_f32] {
                    bytes.extend_from_slice(&f.to_ne_bytes());
                }
                // color0 (16)
                for f in g.color0 {
                    bytes.extend_from_slice(&f.to_ne_bytes());
                }
                // color1 (16)
                for f in g.color1 {
                    bytes.extend_from_slice(&f.to_ne_bytes());
                }
                // clip (16)
                for f in g.clip_rect {
                    bytes.extend_from_slice(&f.to_ne_bytes());
                }
                bytes.extend_from_slice(&g.transform_id.to_ne_bytes());
                bytes.extend_from_slice(&z.to_ne_bytes());
                bytes.extend_from_slice(&[0u8; 8]); // pad → stride 96
            }
            let instances_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("brush_conic_gradient instances"),
                size: bytes.len() as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&instances_buf, 0, &bytes);

            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("brush_conic_gradient bind group"),
                layout: &pipe.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: instances_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: frame_res.transforms.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: frame_res.per_frame.as_entire_binding() },
                ],
            });

            DrawIntent {
                pipeline: pipe.pipeline.clone(),
                bind_group,
                vertex_buffers: vec![],
                vertex_range: 0..4,
                instance_range: 0..instance_count,
                dynamic_offsets: Vec::new(),
                push_constants: Vec::new(),
            }
        };

    let mut draws = Vec::new();
    if !opaque_order.is_empty() {
        draws.push(build_batch(&opaque_order, opaque_pipe));
    }
    if !alpha_order.is_empty() {
        draws.push(build_batch(&alpha_order, alpha_pipe));
    }
    draws
}

// ── Shared buffer helpers ─────────────────────────────────────────────

pub(crate) fn make_transforms_buf(
    scene: &Scene,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> wgpu::Buffer {
    let mut bytes: Vec<u8> = Vec::with_capacity(scene.transforms.len() * 64);
    for t in &scene.transforms {
        for f in &t.m {
            bytes.extend_from_slice(&f.to_ne_bytes());
        }
    }
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("brush_* transforms"),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buf, 0, &bytes);
    buf
}

fn make_per_frame_buf(
    scene: &Scene,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> wgpu::Buffer {
    make_per_frame_buf_for_rect(
        [0.0, 0.0, scene.viewport_width as f32, scene.viewport_height as f32],
        device,
        queue,
    )
}

/// Build a `per_frame` uniform whose orthographic projection maps
/// `world_rect = [x0, y0, x1, y1]` to NDC `(-1, +1)`–`(+1, -1)`.
///
/// For the full viewport this produces the same buffer as `make_per_frame_buf`;
/// Phase 7B uses the per-rect form to render each tile with a tile-local
/// projection so the existing brush pipelines can be reused unchanged.
pub(crate) fn make_per_frame_buf_for_rect(
    world_rect: [f32; 4],
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> wgpu::Buffer {
    let [x0, y0, x1, y1] = world_rect;
    let w = x1 - x0;
    let h = y1 - y0;
    // Column-major: x_ndc = 2*(x-x0)/w - 1, y_ndc = -2*(y-y0)/h + 1
    #[rustfmt::skip]
    let proj: [f32; 16] = [
        2.0 / w,            0.0,             0.0, 0.0,
        0.0,               -2.0 / h,         0.0, 0.0,
        0.0,                0.0,             1.0, 0.0,
       -2.0 * x0 / w - 1.0, 2.0 * y0 / h + 1.0, 0.0, 1.0,
    ];
    let bytes: Vec<u8> = proj.iter().flat_map(|f| f.to_ne_bytes()).collect();
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("brush_* per_frame (tile-local)"),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buf, 0, &bytes);
    buf
}
