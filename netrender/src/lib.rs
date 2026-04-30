/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Netrender — wgpu-native 2D renderer skeleton.
//!
//! Forked from WebRender 0.68 with the GL backend and the GL-shaped
//! frame-builder architecture removed (see Phase D in
//! [`netrender-notes/`](../../netrender-notes/)). What's here is a
//! wgpu device adapter (`device::wgpu`), a minimal `Renderer` that
//! owns it, and the `brush_solid` WGSL pipeline + tests proving the
//! device path renders correctly through `encode_pass`.
//!
//! The frame-builder layer (display-list ingestion → `Frame` →
//! batches → draw calls) is *not yet authored* — the upstream
//! version was shaped around GL thread-model assumptions that don't
//! survive contact with wgpu (`Send+Sync` device, explicit texture
//! handles, no cross-thread queues). Whatever lives there next will
//! be authored against this skeleton, not retrofitted.

#![allow(
    clippy::unreadable_literal,
    clippy::new_without_default,
    clippy::too_many_arguments,
    unknown_lints,
    mismatched_lifetime_syntaxes
)]

mod device;
mod renderer;

pub use crate::renderer::{Renderer, RendererError};
pub use crate::renderer::init::{NetrenderOptions, create_netrender_instance};
pub use crate::device::wgpu::core::{WgpuHandles, REQUIRED_FEATURES};
pub use crate::device::wgpu::adapter::WgpuDevice;
