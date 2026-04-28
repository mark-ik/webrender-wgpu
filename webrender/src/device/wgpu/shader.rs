/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! WGSL module loading + cache (`include_str!`-based); WGSL `override`
//! specialization. See plan §4.9, §6 S1.

/// Solid-colour brush shader. Authored WGSL; exercises override,
/// dynamic uniform, storage buffer, and push-constant tiers.
pub const BRUSH_SOLID_WGSL: &str = include_str!("shaders/brush_solid.wgsl");
