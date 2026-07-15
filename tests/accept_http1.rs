// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Regression tests for `Config::accept_http1`.
//!
//! hyper-util's `serve_connection_with_upgrades` always sniffs the protocol
//! from the first bytes of the connection and silently ignores
//! `http2_only()`, so serving every connection through it meant
//! `accept_http1(false)` was not enforced for plain-text connections: an
//! HTTP/1.1 client was served normally. HTTP/2-only configs must go through
//! `serve_connection`, where the pinned protocol version is honored.

const MESSAGE: &str = "Hello, World!";

fn app() -> axum::Router {
    axum::Router::new().route("/", axum::routing::get(|| async { MESSAGE }))
}

#[tokio::test]
async fn accept_http1_false_rejects_plaintext_http1() {
    let config = sui_http::Config::default().accept_http1(false);
    let handle = sui_http::Builder::new()
        .config(config)
        .serve(("localhost", 0), app())
        .unwrap();

    // A plain HTTP/1.1 request must be refused, not served.
    let result = reqwest::get(format!("http://{}", handle.local_addr())).await;
    assert!(
        result.is_err(),
        "accept_http1(false) was ignored: HTTP/1.1 request succeeded"
    );
}

#[tokio::test]
async fn accept_http1_false_serves_http2_prior_knowledge() {
    let config = sui_http::Config::default().accept_http1(false);
    let handle = sui_http::Builder::new()
        .config(config)
        .serve(("localhost", 0), app())
        .unwrap();

    let client = reqwest::Client::builder()
        .http2_prior_knowledge()
        .build()
        .unwrap();
    let response = client
        .get(format!("http://{}", handle.local_addr()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.version(), http::Version::HTTP_2);
    assert_eq!(response.bytes().await.unwrap(), MESSAGE.as_bytes());
}

/// The default config must keep serving both protocols on one listener.
#[tokio::test]
async fn default_config_serves_both_protocols() {
    let handle = sui_http::Builder::new()
        .serve(("localhost", 0), app())
        .unwrap();
    let url = format!("http://{}", handle.local_addr());

    let h1_response = reqwest::get(&url).await.unwrap();
    assert_eq!(h1_response.version(), http::Version::HTTP_11);
    assert_eq!(h1_response.bytes().await.unwrap(), MESSAGE.as_bytes());

    let h2_client = reqwest::Client::builder()
        .http2_prior_knowledge()
        .build()
        .unwrap();
    let h2_response = h2_client.get(&url).send().await.unwrap();
    assert_eq!(h2_response.version(), http::Version::HTTP_2);
    assert_eq!(h2_response.bytes().await.unwrap(), MESSAGE.as_bytes());
}
