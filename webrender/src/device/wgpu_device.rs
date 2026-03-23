/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! wgpu backend device — headless prototype.
//!
//! `WgpuDevice` can initialise a real `wgpu::Device` and `wgpu::Queue`
//! (no surface required), manage frame lifecycle, and create/upload/delete
//! textures.  Draw calls and pixel readback are not yet implemented.

use api::{ImageBufferKind, ImageFormat};
use super::{GpuFrameId, TextureFilter};
use crate::internal_types::RenderTargetInfo;

// ── Opaque resource types ─────────────────────────────────────────────────────

/// A wgpu-backed texture handle.
pub struct WgpuTexture {
    texture: wgpu::Texture,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
}

/// A wgpu-backed shader pipeline.  Stub until render pipelines are wired up.
pub struct WgpuProgram;

// ── Device ────────────────────────────────────────────────────────────────────

pub struct WgpuDevice {
    device: wgpu::Device,
    queue: wgpu::Queue,
    #[allow(dead_code)]
    features: wgpu::Features,
    frame_id: GpuFrameId,
}

impl WgpuDevice {
    /// Create a headless device (no surface/window required).
    pub fn new_headless() -> Option<Self> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(
            instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })
        ).ok()?;

        let mut required_features = wgpu::Features::empty();
        if adapter.features().contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM) {
            required_features |= wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;
        }

        let (device, queue) = pollster::block_on(
            adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("WgpuDevice"),
                required_features,
                ..Default::default()
            })
        ).ok()?;

        Some(WgpuDevice {
            features: device.features(),
            device,
            queue,
            frame_id: GpuFrameId::new(0),
        })
    }

    pub fn begin_frame(&mut self) -> GpuFrameId {
        self.frame_id = self.frame_id + 1;
        self.frame_id
    }

    pub fn end_frame(&mut self) {
        let _ = self.device.poll(wgpu::PollType::Wait);
    }

    pub fn create_texture(
        &mut self,
        _target: ImageBufferKind,
        format: ImageFormat,
        width: i32,
        height: i32,
        _filter: TextureFilter,
        render_target: Option<RenderTargetInfo>,
    ) -> WgpuTexture {
        let wgpu_format = image_format_to_wgpu(format);
        let mut usage = wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC;
        if render_target.is_some() {
            usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
        }

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: width as u32,
                height: height as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu_format,
            usage,
            view_formats: &[],
        });

        WgpuTexture {
            texture,
            format: wgpu_format,
            width: width as u32,
            height: height as u32,
        }
    }

    pub fn upload_texture_immediate(&mut self, texture: &WgpuTexture, pixels: &[u8]) {
        let bpp = wgpu_format_bytes_per_pixel(texture.format);
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(texture.width * bpp),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: texture.width,
                height: texture.height,
                depth_or_array_layers: 1,
            },
        );
    }

    pub fn clear_texture(&mut self, texture: &WgpuTexture) {
        let view = texture.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                ..Default::default()
            });
        }
        self.queue.submit(Some(encoder.finish()));
    }

    pub fn delete_texture(&mut self, texture: WgpuTexture) {
        drop(texture);
    }
}

// ── Format helpers ────────────────────────────────────────────────────────────

fn image_format_to_wgpu(format: ImageFormat) -> wgpu::TextureFormat {
    match format {
        ImageFormat::R8     => wgpu::TextureFormat::R8Unorm,
        ImageFormat::R16    => wgpu::TextureFormat::R16Unorm,
        ImageFormat::BGRA8  => wgpu::TextureFormat::Bgra8Unorm,
        ImageFormat::RGBA8  => wgpu::TextureFormat::Rgba8Unorm,
        ImageFormat::RG8    => wgpu::TextureFormat::Rg8Unorm,
        ImageFormat::RG16   => wgpu::TextureFormat::Rg16Unorm,
        ImageFormat::RGBAF32 => wgpu::TextureFormat::Rgba32Float,
        ImageFormat::RGBAI32 => wgpu::TextureFormat::Rgba32Sint,
    }
}

fn wgpu_format_bytes_per_pixel(format: wgpu::TextureFormat) -> u32 {
    match format {
        wgpu::TextureFormat::R8Unorm     => 1,
        wgpu::TextureFormat::R16Unorm    => 2,
        wgpu::TextureFormat::Rg8Unorm    => 2,
        wgpu::TextureFormat::Rg16Unorm   => 4,
        wgpu::TextureFormat::Bgra8Unorm  => 4,
        wgpu::TextureFormat::Rgba8Unorm  => 4,
        wgpu::TextureFormat::Rgba32Float => 16,
        wgpu::TextureFormat::Rgba32Sint  => 16,
        _ => panic!("unsupported wgpu format: {:?}", format),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn skip_if_no_gpu() -> Option<WgpuDevice> {
        WgpuDevice::new_headless()
    }

    #[test]
    fn headless_init_and_frame_lifecycle() {
        let mut dev = match skip_if_no_gpu() {
            Some(d) => d,
            None => { eprintln!("skipping: no wgpu adapter"); return; }
        };
        let id1 = dev.begin_frame();
        dev.end_frame();
        let id2 = dev.begin_frame();
        assert!(id2 > id1);
        dev.end_frame();
    }

    #[test]
    fn texture_create_and_upload() {
        let mut dev = match skip_if_no_gpu() {
            Some(d) => d,
            None => { eprintln!("skipping: no wgpu adapter"); return; }
        };
        let tex = dev.create_texture(
            ImageBufferKind::Texture2D, ImageFormat::RGBA8,
            4, 4, TextureFilter::Linear, None,
        );
        let pixels = vec![0u8; 4 * 4 * 4];
        dev.upload_texture_immediate(&tex, &pixels);
        dev.delete_texture(tex);
    }

    #[test]
    fn clear_render_target() {
        let mut dev = match skip_if_no_gpu() {
            Some(d) => d,
            None => { eprintln!("skipping: no wgpu adapter"); return; }
        };
        let tex = dev.create_texture(
            ImageBufferKind::Texture2D, ImageFormat::RGBA8,
            64, 64, TextureFilter::Nearest,
            Some(RenderTargetInfo { has_depth: false }),
        );
        dev.clear_texture(&tex);
        dev.delete_texture(tex);
    }
}
