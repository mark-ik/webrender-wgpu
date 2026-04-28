/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Adapter / Device / Queue boot, surface lifecycle, `REQUIRED_FEATURES`
//! check, debug-label population. See plan §6 S1.

/// wgpu features the device requires. Adapters without these are rejected
/// at boot. See plan §4.10.
///
/// `IMMEDIATES` is wgpu 29's rename of push constants (per WebGPU spec
/// evolution); same underlying GPU primitive — carries the smallest tier
/// of the §4.7 uniform hierarchy. `DUAL_SOURCE_BLENDING` is needed for
/// subpixel AA in the `PsTextRunDualSource` shader family.
pub const REQUIRED_FEATURES: wgpu::Features =
    wgpu::Features::IMMEDIATES.union(wgpu::Features::DUAL_SOURCE_BLENDING);

/// Owned wgpu device handles. Constructed once per process; passed by
/// reference to every other `wgpu/*` module.
pub struct Device {
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

/// Boot wgpu: create the instance, pick an adapter, verify required
/// features are present, request a device + queue. Synchronous via
/// `pollster::block_on`.
pub fn boot() -> Result<Device, BootError> {
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

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("webrender-wgpu device"),
        required_features: REQUIRED_FEATURES,
        required_limits: wgpu::Limits {
            max_inter_stage_shader_variables: 28,
            // Per §4.7: push-constant tier requires non-zero
            // `max_immediate_size`. 128B matches Vulkan's portable
            // minimum and is enough for per-draw flags / indices.
            max_immediate_size: 128,
            ..Default::default()
        },
        ..Default::default()
    }))?;

    Ok(Device {
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
    /// receipt for plan §6 S1.
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
