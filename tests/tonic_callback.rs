// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Integration tests showing how to wrap tonic-style gRPC services with
//! [`sui_http::middleware::callback::CallbackLayer`], on both the server
//! side (inner service is a generated tonic server) and the client side
//! (inner service is `tonic::transport::Channel`).
//!
//! Tonic's generated servers and [`tonic::transport::Channel`] are
//! monomorphic on [`tonic::body::Body`] — they do not accept an arbitrary
//! `B: http_body::Body`. The callback middleware, on the other hand, is
//! body-polymorphic: it wraps the inbound body in a `RequestBody<B, H>`
//! and hands that to the inner service. Connecting the two requires one
//! small bridge: a `map_request` layer that reboxes the wrapped body
//! back into `tonic::body::Body` before it reaches the tonic service.
//! The same pattern applies whether the tonic service sits at the
//! server end of the stack or the client end.

use bytes::Buf;
use bytes::Bytes;
use http::Request;
use http::Response;
use http_body_util::BodyExt;
use http_body_util::Full;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::task::Context;
use std::task::Poll;
use sui_http::middleware::callback::CallbackLayer;
use sui_http::middleware::callback::MakeCallbackHandler;
use sui_http::middleware::callback::RequestBody;
use sui_http::middleware::callback::RequestHandler;
use sui_http::middleware::callback::ResponseBody;
use sui_http::middleware::callback::ResponseHandler;
use tower::Service;
use tower::ServiceBuilder;
use tower::ServiceExt;

/// A stand-in for a tonic-generated service. The important property is
/// that it is monomorphic on `tonic::body::Body` — it only accepts that
/// one concrete body type on both sides, just like real tonic servers.
#[derive(Clone, Default)]
struct GrpcEcho;

impl Service<Request<tonic::body::Body>> for GrpcEcho {
    type Response = Response<tonic::body::Body>;
    type Error = tonic::Status;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<tonic::body::Body>) -> Self::Future {
        Box::pin(async move {
            let (_parts, body) = req.into_parts();
            // Collect the request body and echo the bytes back as the
            // response body, all through `tonic::body::Body`.
            let collected = body
                .collect()
                .await
                .map_err(|e| tonic::Status::internal(format!("body error: {e}")))?;
            let bytes = collected.to_bytes();
            Ok(Response::new(tonic::body::Body::new(Full::new(bytes))))
        })
    }
}

#[derive(Debug, Default)]
struct Events {
    request_bytes: usize,
    request_end_seen: bool,
    response_bytes: usize,
    response_end_seen: bool,
    response_seen: bool,
    service_errors: Vec<String>,
}

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Events>>);

struct ReqH(Arc<Mutex<Events>>);
struct RespH(Arc<Mutex<Events>>);

impl RequestHandler for ReqH {
    fn on_body_chunk<B: Buf>(&mut self, chunk: &B) {
        self.0.lock().unwrap().request_bytes += chunk.remaining();
    }
    fn on_end_of_stream(&mut self, _trailers: Option<&http::HeaderMap>) {
        self.0.lock().unwrap().request_end_seen = true;
    }
}

impl ResponseHandler for RespH {
    fn on_response(&mut self, _parts: &http::response::Parts) {
        self.0.lock().unwrap().response_seen = true;
    }
    fn on_service_error<E: std::fmt::Display + 'static>(&mut self, error: &E) {
        self.0
            .lock()
            .unwrap()
            .service_errors
            .push(error.to_string());
    }
    fn on_body_chunk<B: Buf>(&mut self, chunk: &B) {
        self.0.lock().unwrap().response_bytes += chunk.remaining();
    }
    fn on_end_of_stream(&mut self, _trailers: Option<&http::HeaderMap>) {
        self.0.lock().unwrap().response_end_seen = true;
    }
}

impl MakeCallbackHandler for Recorder {
    type RequestHandler = ReqH;
    type ResponseHandler = RespH;

    fn make_handler(
        &self,
        _request: &http::request::Parts,
    ) -> (Self::RequestHandler, Self::ResponseHandler) {
        (ReqH(self.0.clone()), RespH(self.0.clone()))
    }
}

#[tokio::test]
async fn callback_layer_bridges_into_tonic_service() {
    let recorder = Recorder::default();
    let events = recorder.0.clone();

    // Build the middleware stack.
    //
    // Order is outermost-first:
    //   1. CallbackLayer wraps the request body as `RequestBody<_, _>`.
    //   2. `map_request` reboxes the wrapped body back to
    //      `tonic::body::Body` — this is the bridge tonic requires.
    //   3. `map_response` does the mirror-image rebox on the response
    //      body that the tonic service produces, so the outer caller
    //      sees `Response<tonic::body::Body>` (wrapped once more by
    //      CallbackLayer as `Response<ResponseBody<tonic::body::Body,
    //      RespH>>`, which is still a body-polymorphic type).
    //   4. GrpcEcho — the monomorphic "tonic" service.
    let mut stack = ServiceBuilder::new()
        .layer(CallbackLayer::new(recorder))
        .map_request(|req: Request<RequestBody<Full<Bytes>, ReqH>>| {
            req.map(tonic::body::Body::new)
        })
        .map_response(|resp: Response<tonic::body::Body>| {
            // Identity in practice — included to show the symmetric
            // adaptation point for callers that further transform the
            // response body. For a non-trivial rebox you might write
            // `resp.map(tonic::body::Body::new)` after a different
            // inner service.
            resp
        })
        .service(GrpcEcho);

    let request = Request::new(Full::new(Bytes::from_static(b"hello tonic")));
    let response = stack.ready().await.unwrap().call(request).await.unwrap();

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, Bytes::from_static(b"hello tonic"));

    let events = events.lock().unwrap();
    // Request-side observation fired before the body was reboxed.
    assert_eq!(events.request_bytes, b"hello tonic".len());
    assert!(events.request_end_seen);
    // Response-side observation fired after the tonic service produced
    // its response body.
    assert!(events.response_seen);
    assert_eq!(events.response_bytes, b"hello tonic".len());
    assert!(events.response_end_seen);
    assert!(events.service_errors.is_empty());
}

#[tokio::test]
async fn callback_layer_observes_tonic_service_error() {
    #[derive(Clone, Default)]
    struct FailingGrpc;

    impl Service<Request<tonic::body::Body>> for FailingGrpc {
        type Response = Response<tonic::body::Body>;
        type Error = tonic::Status;
        type Future =
            Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _req: Request<tonic::body::Body>) -> Self::Future {
            Box::pin(async { Err(tonic::Status::unavailable("nope")) })
        }
    }

    let recorder = Recorder::default();
    let events = recorder.0.clone();

    let mut stack = ServiceBuilder::new()
        .layer(CallbackLayer::new(recorder))
        .map_request(|req: Request<RequestBody<Full<Bytes>, ReqH>>| {
            req.map(tonic::body::Body::new)
        })
        .service(FailingGrpc);

    let request = Request::new(Full::new(Bytes::from_static(b"ping")));
    let result = stack.ready().await.unwrap().call(request).await;
    let status = match result {
        Ok(_) => panic!("expected tonic::Status error"),
        Err(status) => status,
    };
    // tonic::Status renders its code; be lenient about the exact text.
    let rendered = status.to_string();
    assert!(
        rendered.contains("nope") || rendered.to_lowercase().contains("unavailable"),
        "unexpected status display: {rendered}"
    );

    let events = events.lock().unwrap();
    // Service-level error routed to the response handler.
    assert!(!events.service_errors.is_empty());
    assert!(
        events
            .service_errors
            .iter()
            .any(|s| s.contains("nope") || s.to_lowercase().contains("unavailable")),
        "unexpected service_errors: {:?}",
        events.service_errors
    );
    // The response itself never materialized.
    assert!(!events.response_seen);
    assert_eq!(events.response_bytes, 0);
}

// ---------------------------------------------------------------------------
// Client side
// ---------------------------------------------------------------------------
//
// On the client side the middleware stack sits between a generated tonic
// stub and `tonic::transport::Channel`. A stub like `GreeterClient<T>`
// is generic over its transport `T`, but it requires `T` to implement
// `Service<http::Request<tonic::body::Body>>` with a matching response —
// exactly what `Channel` implements, and exactly the body-type
// monomorphism that forced the rebox on the server side. Inserting
// `CallbackLayer` between the stub and the channel therefore uses the
// same bridge: a `map_request` that reboxes `RequestBody<_, _>` back
// into `tonic::body::Body`. A user who also wants the stub to receive
// `Response<tonic::body::Body>` (the shape `tonic::client::Grpc<T>`
// expects) adds a `map_response` one-liner too; the example below does
// both.

/// A stand-in for [`tonic::transport::Channel`]. In real use, a
/// `Channel` performs an HTTP/2 round-trip to a gRPC server; here we
/// skip the network and echo the request body back as the response.
/// What matters for this test is the `Service` signature it exposes,
/// which is identical to `Channel`'s.
#[derive(Clone, Default)]
struct MockChannel;

impl Service<Request<tonic::body::Body>> for MockChannel {
    type Response = Response<tonic::body::Body>;
    type Error = tonic::transport::Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<tonic::body::Body>) -> Self::Future {
        Box::pin(async move {
            let (_parts, body) = req.into_parts();
            let collected = body
                .collect()
                .await
                .expect("MockChannel body collect cannot fail in this test");
            let bytes = collected.to_bytes();
            Ok(Response::new(tonic::body::Body::new(Full::new(bytes))))
        })
    }
}

#[tokio::test]
async fn callback_layer_wraps_tonic_client_channel() {
    let recorder = Recorder::default();
    let events = recorder.0.clone();

    // Client-side stack. Order is outermost-first:
    //
    //   1. `map_response` reboxes `ResponseBody<tonic::body::Body, _>`
    //      back to `tonic::body::Body` so the outer caller (a tonic
    //      stub, or `tonic::client::Grpc::new(_)`) sees the body type
    //      it expects.
    //   2. `CallbackLayer` wraps the outbound request body and observes
    //      the inbound response body.
    //   3. `map_request` reboxes the wrapped body back to
    //      `tonic::body::Body` for the channel.
    //   4. `MockChannel` stands in for `tonic::transport::Channel`.
    //
    // The resulting `client` is a
    // `Service<Request<tonic::body::Body>, Response = Response<tonic::body::Body>>`
    // — exactly the shape a generated `...Client<T>` stub will accept
    // as its transport.
    let mut client = ServiceBuilder::new()
        .map_response(
            |resp: Response<ResponseBody<tonic::body::Body, RespH>>| {
                resp.map(tonic::body::Body::new)
            },
        )
        .layer(CallbackLayer::new(recorder))
        .map_request(|req: Request<RequestBody<tonic::body::Body, ReqH>>| {
            req.map(tonic::body::Body::new)
        })
        .service(MockChannel);

    // Send a request the way a generated stub would: wrapped in
    // `tonic::body::Body` already.
    let request = Request::new(tonic::body::Body::new(Full::new(Bytes::from_static(
        b"outbound rpc",
    ))));
    let response = client.ready().await.unwrap().call(request).await.unwrap();

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body_bytes, Bytes::from_static(b"outbound rpc"));

    let events = events.lock().unwrap();
    // Outbound request body was observed on its way to the channel.
    assert_eq!(events.request_bytes, b"outbound rpc".len());
    assert!(events.request_end_seen);
    // Inbound response was observed on its way back to the stub.
    assert!(events.response_seen);
    assert_eq!(events.response_bytes, b"outbound rpc".len());
    assert!(events.response_end_seen);
    assert!(events.service_errors.is_empty());
}

#[tokio::test]
async fn callback_layer_observes_tonic_client_channel_error() {
    /// A channel that fails to connect. Mirrors the real `Channel`
    /// behaviour when the remote endpoint is unreachable: the future
    /// resolves to `Err(tonic::transport::Error)` before any response
    /// is produced.
    #[derive(Clone, Default)]
    struct UnreachableChannel;

    impl Service<Request<tonic::body::Body>> for UnreachableChannel {
        type Response = Response<tonic::body::Body>;
        type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
        type Future =
            Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _req: Request<tonic::body::Body>) -> Self::Future {
            Box::pin(async { Err("connection refused".into()) })
        }
    }

    let recorder = Recorder::default();
    let events = recorder.0.clone();

    let mut client = ServiceBuilder::new()
        .layer(CallbackLayer::new(recorder))
        .map_request(|req: Request<RequestBody<tonic::body::Body, ReqH>>| {
            req.map(tonic::body::Body::new)
        })
        .service(UnreachableChannel);

    let request = Request::new(tonic::body::Body::new(Full::new(Bytes::from_static(
        b"outbound rpc",
    ))));
    let result = client.ready().await.unwrap().call(request).await;
    let err = match result {
        Ok(_) => panic!("expected channel error"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("connection refused"));

    let events = events.lock().unwrap();
    // Channel error surfaced to the response handler.
    assert_eq!(events.service_errors.len(), 1);
    assert!(events.service_errors[0].contains("connection refused"));
    // No response materialized.
    assert!(!events.response_seen);
    assert_eq!(events.response_bytes, 0);
}
