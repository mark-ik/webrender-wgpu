/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! wgpu primitives owned by the renderer: instance, adapter, device,
//! queue. These come from the embedder via [`WgpuHandles`] (production)
//! or from [`boot`] (headless tests / CI).

/// wgpu features the renderer requires at boot. Phase 0.5 of the
/// netrender design plan demotes this to `Features::empty()` per
/// axiom 10 — Phases 1–9 work on a baseline wgpu adapter; optional
/// features like `DUAL_SOURCE_BLENDING` (Phase 10 subpixel-AA) move
/// to the pipeline factory that needs them. `IMMEDIATES` (wgpu 29's
/// rename of push constants) was previously requested unconditionally
/// for the smoke pipeline, but `brush_solid` declares
/// `immediate_size: 0`, so it's pure carry-over and dropped here.
pub const REQUIRED_FEATURES: wgpu::Features = wgpu::Features::empty();

/// Features the renderer would *like* if the adapter supports them
/// but doesn't strictly require. [`boot`] requests the intersection
/// of this set with the adapter's reported features, opting in
/// opportunistically — pipelines that depend on a feature in this
/// set query `device.features()` at build time and fall back when
/// the feature is absent. This is what makes 10a.4's subpixel-AA
/// pipeline available on capable adapters without forcing baseline
/// adapters to fail at boot.
///
/// `with_external` does *not* expand the embedder's device features
/// — the embedder owns its device and chose what to enable. Pipeline
/// factories check `device.features()` for any optional capability
/// regardless of which path created the device.
pub const OPTIONAL_FEATURES: wgpu::Features = wgpu::Features::DUAL_SOURCE_BLENDING;

/// Bundle of wgpu primitives owned by the embedder and passed through
/// `create_netrender_instance` to the renderer. All four wgpu 29 handle
/// types are `Clone` (Arc-wrapped internally), so passing by value is
/// cheap.
///
/// The embedder is expected to have already created instance, adapter,
/// device, and queue for its own surface / compositor work; these
/// handles are *the same ones* the embedder uses, so external textures
/// (e.g. video frames) integrate naturally — they're created on the
/// same device and can be sampled here without copy.
#[derive(Clone)]
pub struct WgpuHandles {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

#[derive(Debug)]
pub enum BootError {
    Adapter(wgpu::RequestAdapterError),
    MissingFeatures(wgpu::Features),
    Device(wgpu::RequestDeviceError),
}

impl std::fmt::Display for BootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Adapter(e) => write!(f, "could not request a wgpu adapter: {e}"),
            Self::MissingFeatures(missing) => {
                write!(f, "adapter is missing required features: {missing:?}")
            }
            Self::Device(e) => write!(f, "device request failed: {e}"),
        }
    }
}

impl std::error::Error for BootError {}

impl From<wgpu::RequestAdapterError> for BootError {
    fn from(e: wgpu::RequestAdapterError) -> Self {
        Self::Adapter(e)
    }
}

impl From<wgpu::RequestDeviceError> for BootError {
    fn from(e: wgpu::RequestDeviceError) -> Self {
        Self::Device(e)
    }
}

/// Boot wgpu standalone: create the instance, pick an adapter, verify
/// [`REQUIRED_FEATURES`], request a device + queue. Production goes
/// through [`crate::WgpuDevice::with_external`] where the embedder
/// supplies the primitives; this helper exists for headless tests, CI
/// goldens, and tools that don't have an embedder fixture.
///
/// Phase 0.5 demoted [`REQUIRED_FEATURES`] to `Features::empty()`, so
/// this boots cleanly on Lavapipe / WARP / SwiftShader software
/// adapters.
pub fn boot() -> Result<WgpuHandles, BootError> {
    let instance = wgpu::Instance::default();

    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))?;

    let missing = REQUIRED_FEATURES - adapter.features();
    if !missing.is_empty() {
        return Err(BootError::MissingFeatures(missing));
    }

    // Opportunistic: enable any optional feature the adapter
    // supports. Pipeline factories that depend on these check
    // `device.features()` at build time and fall back when absent.
    let requested_features = REQUIRED_FEATURES | (OPTIONAL_FEATURES & adapter.features());

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("netrender device"),
        required_features: requested_features,
        required_limits: wgpu::Limits {
            max_inter_stage_shader_variables: 28,
            ..Default::default()
        },
        ..Default::default()
    }))?;

    Ok(WgpuHandles {
        instance,
        adapter,
        device,
        queue,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Boot the device, clear a 4×4 offscreen target to a known color,
    /// read it back, assert the pixel matches. Smallest end-to-end
    /// receipt for the device path.
    #[test]
    fn boot_clear_readback_smoke() {
        let dev = boot().expect("wgpu boot");

        let size = wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        };
        let texture = dev.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("S1 smoke target"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = dev
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("S1 smoke encoder"),
            });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("S1 smoke pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 1.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }

        let padded_bytes_per_row =
            (4 * 4_u32).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = dev.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("S1 smoke readback"),
            size: padded_bytes_per_row as u64 * 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: 0, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(4),
                },
            },
            size,
        );
        dev.queue.submit([encoder.finish()]);

        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        dev.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device poll");
        rx.recv()
            .expect("map_async sender dropped")
            .expect("map failed");

        let mapped = slice.get_mapped_range();
        // Rgba8Unorm: clear (1.0, 0.0, 0.0, 1.0) → (255, 0, 0, 255).
        assert_eq!(&mapped[0..4], &[255, 0, 0, 255]);
    }
}
