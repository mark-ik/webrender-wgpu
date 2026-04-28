/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Render-pass batching. Ingests `DrawIntent`s; flushes per pass; one
//! `BeginRenderPass` per target switch. See plan §4.8, §6 S1.
