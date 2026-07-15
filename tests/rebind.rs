// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Regression test for rebinding a port immediately after shutdown.
//!
//! When a server shuts down, its side of each accepted connection closes
//! first, leaving sockets bound to the listen port in TIME_WAIT. Without
//! SO_REUSEADDR on the listener, binding the same port again fails with
//! EADDRINUSE until TIME_WAIT expires (up to 2*MSL), which breaks fast
//! restarts.

use axum::Router;

#[tokio::test]
async fn rebind_immediately_after_shutdown() {
    const MESSAGE: &str = "Hello, World!";

    let app = Router::new().route("/", axum::routing::get(|| async { MESSAGE }));

    let handle = sui_http::Builder::new()
        .serve(("localhost", 0), app.clone())
        .unwrap();
    let addr = *handle.local_addr();

    // Serve a request so an accepted connection exists and enters
    // TIME_WAIT on the server side once the server shuts down.
    let response = reqwest::get(format!("http://{addr}"))
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(response, MESSAGE.as_bytes());

    handle.shutdown().await;

    // Rebinding the same port must succeed immediately.
    let handle = sui_http::Builder::new()
        .serve(addr, app)
        .expect("rebinding the port immediately after shutdown failed");

    let response = reqwest::get(format!("http://{}", handle.local_addr()))
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(response, MESSAGE.as_bytes());
}
