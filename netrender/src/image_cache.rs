/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 5 image cache — maps `ImageKey → wgpu::Texture`.
//!
//! On first access for a key, the CPU pixels are uploaded once; all
//! subsequent `get_or_upload` calls return the cached handle. The cache
//! is held behind a `Mutex` in `Renderer` so `prepare()` can mutate it
//! from a `&self` context without requiring `&mut self`.

use std::collections::HashMap;
use std::sync::Arc;

use crate::scene::{ImageData, ImageKey};

pub(crate) struct ImageCache {
    textures: HashMap<ImageKey, Arc<wgpu::Texture>>,
}

impl ImageCache {
    pub fn new() -> Self {
        Self { textures: HashMap::new() }
    }

    /// Return the cached `wgpu::Texture` for `key`, uploading `data` on
    /// first access. Subsequent calls with the same key ignore `data`.
    pub fn get_or_upload(
        &mut self,
        key: ImageKey,
        data: &ImageData,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Arc<wgpu::Texture> {
        if let Some(tex) = self.textures.get(&key) {
            return tex.clone();
        }
        let texture = Arc::new(upload_image(data, device, queue));
        self.textures.insert(key, texture.clone());
        texture
    }

    pub fn get(&self, key: ImageKey) -> Option<Arc<wgpu::Texture>> {
        self.textures.get(&key).cloned()
    }

    /// Insert a pre-existing GPU texture directly, bypassing CPU upload.
    /// Used by the render graph to inject blur/filter outputs into the
    /// cache so they can be composited as images in the next scene pass.
    /// Overwrites any previously cached entry for `key`.
    pub fn insert_gpu(&mut self, key: ImageKey, tex: Arc<wgpu::Texture>) {
        self.textures.insert(key, tex);
    }
}

fn upload_image(data: &ImageData, device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("image cache entry"),
        size: wgpu::Extent3d {
            width: data.width,
            height: data.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &data.bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(data.width * 4),
            rows_per_image: Some(data.height),
        },
        wgpu::Extent3d {
            width: data.width,
            height: data.height,
            depth_or_array_layers: 1,
        },
    );
    texture
}
