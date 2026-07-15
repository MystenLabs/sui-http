// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Regression tests for max connection age and its grace period.
//!
//! A graceful shutdown (GOAWAY) waits for in-flight streams to complete,
//! but a stream that can make no progress keeps the connection alive
//! forever: a handler that never finishes, or a response wedged behind
//! HTTP/2 flow-control windows that a stalled client never reopens. The
//! grace period is the only server-side mechanism that reclaims such
//! connections.

use std::convert::Infallible;
use std::time::Duration;

use bytes::Bytes;
use sui_http::Config;

async fn wait_for_no_connections(handle: &sui_http::ServerHandle) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while handle.number_of_connections() > 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("connection was never reclaimed");
}

/// A connection wedged by a handler that never completes must be
/// force-closed once the age-triggered graceful shutdown exceeds the grace
/// period.
#[tokio::test]
async fn age_with_grace_force_closes_wedged_connection() {
    let app = axum::Router::new().route(
        "/",
        axum::routing::get(|| async { std::future::pending::<String>().await }),
    );

    let config = Config::default()
        .max_connection_age(Duration::from_millis(300))
        .max_connection_age_grace(Duration::from_millis(300));
    let handle = sui_http::Builder::new()
        .config(config)
        .serve(("localhost", 0), app)
        .unwrap();

    let client = reqwest::Client::builder()
        .http2_prior_knowledge()
        .build()
        .unwrap();
    let url = format!("http://{}", handle.local_addr());

    // The request must resolve with an error once the connection is
    // dropped, rather than hanging forever on the wedged handler.
    let result = tokio::time::timeout(Duration::from_secs(10), client.get(url).send())
        .await
        .expect("request hung: connection was never force-closed");
    assert!(result.is_err());

    wait_for_no_connections(&handle).await;
}

/// A connection with a send-stalled stream -- the server has response DATA
/// queued but the client never reopens its flow-control window -- must be
/// reclaimed by age + grace. Nothing else can reclaim it: hyper parks the
/// send pipe in poll_capacity and stops polling the response body, and the
/// stream's trailers queue behind the unsendable DATA.
#[tokio::test]
async fn age_with_grace_reclaims_send_stalled_connection() {
    let app = axum::Router::new().route(
        "/stream",
        axum::routing::get(|| async {
            let stream = futures::stream::repeat_with(|| {
                Ok::<_, Infallible>(Bytes::from_static(&[0u8; 16 * 1024]))
            });
            axum::body::Body::from_stream(stream)
        }),
    );

    let config = Config::default()
        .max_connection_age(Duration::from_millis(300))
        .max_connection_age_grace(Duration::from_millis(300));
    let handle = sui_http::Builder::new()
        .config(config)
        .serve(("localhost", 0), app)
        .unwrap();
    let addr = *handle.local_addr();

    let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let (mut send_request, connection) = h2::client::handshake(tcp).await.unwrap();
    tokio::spawn(connection);

    let request = http::Request::builder()
        .uri(format!("http://{addr}/stream"))
        .body(())
        .unwrap();
    send_request = send_request.ready().await.unwrap();
    let (response, _) = send_request.send_request(request, true).unwrap();
    let response = response.await.unwrap();
    assert!(response.status().is_success());

    // Read one chunk without releasing flow-control capacity, then stall.
    // The server fills the stream's send window and parks; its response can
    // never complete, so a graceful shutdown alone would wait forever.
    let mut body = response.into_body();
    let first = body.data().await.expect("first chunk").unwrap();
    assert!(!first.is_empty());

    // Hold the stalled stream (and its connection) alive while the server
    // ages the connection out and the grace period expires.
    wait_for_no_connections(&handle).await;
    drop(body);
    drop(send_request);
}

/// Without a grace period, an age-triggered graceful shutdown must keep
/// waiting for in-flight requests instead of dropping them.
#[tokio::test]
async fn age_without_grace_waits_for_in_flight_requests() {
    let (release_tx, release_rx) = tokio::sync::watch::channel(false);
    let app = axum::Router::new().route(
        "/",
        axum::routing::get(move || {
            let mut release_rx = release_rx.clone();
            async move {
                let _ = release_rx.wait_for(|released| *released).await;
                "ok"
            }
        }),
    );

    let config = Config::default().max_connection_age(Duration::from_millis(200));
    let handle = sui_http::Builder::new()
        .config(config)
        .serve(("localhost", 0), app)
        .unwrap();

    let client = reqwest::Client::builder()
        .http2_prior_knowledge()
        .build()
        .unwrap();
    let url = format!("http://{}", handle.local_addr());

    let request = tokio::spawn(client.get(url).send());

    // Well past the connection's maximum age, the held request must still
    // be in flight on a live connection.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(handle.number_of_connections(), 1);
    assert!(!request.is_finished());

    // Once released, the request completes successfully and the graceful
    // shutdown can finish.
    release_tx.send(true).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(10), request)
        .await
        .expect("request did not complete after release")
        .unwrap()
        .unwrap();
    assert!(response.status().is_success());
    wait_for_no_connections(&handle).await;
}
