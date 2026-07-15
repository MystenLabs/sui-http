// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Regression test for the default HTTP/2 concurrent stream limit.
//!
//! hyper treats an explicit `None` for `max_concurrent_streams` as "remove
//! the limit", so a `Config` that defaulted the field to `None` and passed
//! it through unconditionally silently erased hyper's hardened default and
//! advertised unlimited concurrent streams per connection. Verify that the
//! default config caps a single connection at 200 concurrent streams.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

const STREAM_LIMIT: usize = 200;
const ATTEMPTED_STREAMS: usize = 250;

#[tokio::test]
async fn default_config_limits_concurrent_streams_per_connection() {
    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let (release_tx, release_rx) = tokio::sync::watch::channel(false);

    let app = {
        let in_flight = in_flight.clone();
        let peak = peak.clone();
        axum::Router::new()
            .route("/warmup", axum::routing::get(|| async { "ok" }))
            .route(
                "/hold",
                axum::routing::get(move || {
                    let in_flight = in_flight.clone();
                    let peak = peak.clone();
                    let mut release_rx = release_rx.clone();
                    async move {
                        let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(now, Ordering::SeqCst);
                        // Hold the stream open until the test releases all
                        // handlers so that streams accumulate on the
                        // connection.
                        let _ = release_rx.wait_for(|released| *released).await;
                        in_flight.fetch_sub(1, Ordering::SeqCst);
                        "ok"
                    }
                }),
            )
    };

    let handle = sui_http::Builder::new()
        .serve(("localhost", 0), app)
        .unwrap();
    let addr = *handle.local_addr();

    let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let (mut send_request, connection) = h2::client::handshake(tcp).await.unwrap();
    tokio::spawn(connection);

    // Complete one request before fanning out so the server's SETTINGS
    // frame (which carries the stream limit) has been received and applied.
    let warmup = http::Request::builder()
        .uri(format!("http://{addr}/warmup"))
        .body(())
        .unwrap();
    send_request = send_request.ready().await.unwrap();
    let (response, _) = send_request.send_request(warmup, true).unwrap();
    assert!(response.await.unwrap().status().is_success());

    // Issue more streams than the advertised limit, all on this one
    // connection. `ready()` does not resolve while the peer's
    // SETTINGS_MAX_CONCURRENT_STREAMS is exhausted, so this task parks at
    // the limit until handlers are released; it must not run on the test
    // task, which is what performs the release.
    let issuer = tokio::spawn(async move {
        let mut responses = Vec::with_capacity(ATTEMPTED_STREAMS);
        for _ in 0..ATTEMPTED_STREAMS {
            send_request = send_request.ready().await.unwrap();
            let request = http::Request::builder()
                .uri(format!("http://{addr}/hold"))
                .body(())
                .unwrap();
            let (response, _) = send_request.send_request(request, true).unwrap();
            responses.push(tokio::spawn(async move {
                let response = response.await.unwrap();
                assert!(response.status().is_success());
                let mut body = response.into_body();
                let mut flow_control = body.flow_control().clone();
                while let Some(chunk) = body.data().await {
                    let chunk = chunk.unwrap();
                    let _ = flow_control.release_capacity(chunk.len());
                }
            }));
        }
        responses
    });

    // Wait for the connection to fill up to the limit.
    tokio::time::timeout(Duration::from_secs(10), async {
        while in_flight.load(Ordering::SeqCst) < STREAM_LIMIT {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("server never reached the advertised stream limit");

    // Give any over-limit streams a chance to arrive, then verify none did.
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(in_flight.load(Ordering::SeqCst), STREAM_LIMIT);
    assert_eq!(peak.load(Ordering::SeqCst), STREAM_LIMIT);

    // Release the held handlers; every attempted stream must now complete.
    release_tx.send(true).unwrap();
    let responses = tokio::time::timeout(Duration::from_secs(10), issuer)
        .await
        .expect("issuer did not finish after release")
        .unwrap();
    assert_eq!(responses.len(), ATTEMPTED_STREAMS);
    for response in responses {
        tokio::time::timeout(Duration::from_secs(10), response)
            .await
            .expect("response did not complete after release")
            .unwrap();
    }
    assert_eq!(peak.load(Ordering::SeqCst), STREAM_LIMIT);
}
