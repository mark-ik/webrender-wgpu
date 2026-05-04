/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Shared test helpers for the integration tests in this directory.
//!
//! Each `tests/<name>.rs` file is its own crate, so any helper used
//! by more than one test file gets duplicated unless it lives in a
//! shared module under `tests/common/`. Cargo treats `tests/common/`
//! specially — files inside it are NOT compiled as separate test
//! binaries; they're available only via `mod common;` in a test file.
//!
//! Most of what used to live here is now public in
//! `netrender::filter`; this file keeps a thin re-export so existing
//! `use common::*` imports in p6 / p9 keep working without churn.

#![allow(dead_code, unused_imports)]

pub use netrender::filter::{blur_pass_callback, clip_rectangle_callback, make_bilinear_sampler};
