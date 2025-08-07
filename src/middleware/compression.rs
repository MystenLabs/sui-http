// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

//! A middleware that compresses response bodies.
//!
//! This is a re-export of the `tower_http::compression::CompressionLayer`.

pub use tower_http::compression::CompressionLayer;