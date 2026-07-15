// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Regression test for the HTTP/1 header read timeout.
//!
//! hyper defaults the HTTP/1 header read timeout to 30 seconds, but
//! silently disables it when no timer is configured on the HTTP/1 side of
//! the connection builder. With the timeout disabled, a client that sends
//! a partial request and goes silent (slowloris) holds its socket open
//! indefinitely, bounded only by file descriptors.

use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn slowloris_connection_is_reaped() {
    let app = axum::Router::new().route("/", axum::routing::get(|| async { "ok" }));

    // Short timeout to keep the test fast; the default (30s, hyper's
    // default) is pinned by a unit test.
    let config =
        sui_http::Config::default().http1_header_read_timeout(Some(Duration::from_millis(500)));
    let handle = sui_http::Builder::new()
        .config(config)
        .serve(("localhost", 0), app)
        .unwrap();

    let mut socket = tokio::net::TcpStream::connect(handle.local_addr())
        .await
        .unwrap();
    // Partial request: no terminating CRLF, then go silent.
    socket
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n")
        .await
        .unwrap();

    // Once the header read timeout fires the server must close the
    // connection: the read returns a 408 followed by EOF (or an error if
    // the server resets), and the connection count drops to zero.
    let read_until_closed = async {
        let mut buf = Vec::new();
        let _ = socket.read_to_end(&mut buf).await;
    };
    tokio::time::timeout(Duration::from_secs(10), read_until_closed)
        .await
        .expect("slowloris connection still open: header read timeout is not armed");

    tokio::time::timeout(Duration::from_secs(10), async {
        while handle.number_of_connections() > 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("connection was never removed from the active set");
}
