/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! wgpu backend device stub — Phase 1.
//!
//! This module is compiled only when `wgpu_backend` is enabled. It provides a
//! `WgpuDevice` that will eventually implement the same `GpuDevice` trait as
//! the GL backend.  For now the struct is empty and draw calls are no-ops so
//! that the feature-flag infrastructure can be exercised and tested without
//! requiring any GPU work.

/// A WebGPU-backed rendering device.
///
/// ## Current status
/// Phase 1 stub only — no wgpu resources are created yet.  The struct will
/// grow `wgpu::Instance`, `wgpu::Adapter`, `wgpu::Device`, and `wgpu::Queue`
/// fields once the `GpuDevice` trait is defined and the first render passes
/// are implemented (Phase 2).
pub struct WgpuDevice {
    // Phase 2 will add:
    //   instance: wgpu::Instance,
    //   adapter:  wgpu::Adapter,
    //   device:   wgpu::Device,
    //   queue:    wgpu::Queue,
}

impl WgpuDevice {
    /// Construct a headless `WgpuDevice` with no surface, suitable for
    /// off-screen rendering and unit tests.
    pub fn new_headless() -> Self {
        WgpuDevice {}
    }
}
