/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

mod gl;
pub mod query_gl;
pub mod traits;

pub use self::gl::*;
pub use self::query_gl as query;
pub use self::traits::{BlendMode, GpuFrame, GpuPass, GpuResources, GpuShaders};

/// Alias retained so renderer code that still names `Device` resolves to
/// the (renamed) `GlDevice`. P0c rename: the GL backend's concrete type is
/// `GlDevice`; the alias preserves source compatibility for the ~116
/// external call sites that name `Device` in field types and function
/// signatures. Migrating those sites to `GlDevice` directly is a future
/// cosmetic cleanup; the alias is permanent for now.
pub type Device = self::gl::GlDevice;
