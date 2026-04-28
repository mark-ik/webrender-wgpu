/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! BindGroupLayout + BindGroup caches. Bind groups created with
//! `has_dynamic_offset: true` per §4.7. See plan §6 S1.
