// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Middleware for observing both the request and response streams of a
//! [`tower::Service`] via user-provided callback handlers.
//!
//! A [`MakeCallbackHandler`] produces a pair of handlers per request:
//! a [`RequestHandler`] (invoked as the request body is polled by the
//! inner service) and a [`ResponseHandler`] (invoked when the response
//! materializes and as its body is polled by the caller). A single
//! [`CallbackBody`] type wraps the body in both positions, with a
//! direction marker ([`RequestDirection`] or [`ResponseDirection`])
//! selecting which handler trait the body dispatches through.
//!
//! Either side can be a no-op by using the unit type `()`, which has a
//! blanket [`RequestHandler`] impl provided by this crate.
//!
//! # Example
//!
//! ```
//! use http::request;
//! use http::response;
//! use sui_http::middleware::callback::CallbackLayer;
//! use sui_http::middleware::callback::MakeCallbackHandler;
//! use sui_http::middleware::callback::RequestHandler;
//! use sui_http::middleware::callback::ResponseHandler;
//!
//! /// A handler that counts bytes observed on one side of the exchange.
//! #[derive(Default)]
//! struct ByteCounter {
//!     bytes: usize,
//! }
//!
//! impl RequestHandler for ByteCounter {
//!     fn on_body_chunk<B: bytes::Buf>(&mut self, chunk: &B) {
//!         self.bytes += chunk.remaining();
//!     }
//! }
//!
//! impl ResponseHandler for ByteCounter {
//!     fn on_response(&mut self, _parts: &response::Parts) {}
//!     fn on_service_error<E: std::fmt::Display + 'static>(&mut self, _error: &E) {}
//!     fn on_body_chunk<B: bytes::Buf>(&mut self, chunk: &B) {
//!         self.bytes += chunk.remaining();
//!     }
//! }
//!
//! #[derive(Clone)]
//! struct MakeByteCounter;
//!
//! impl MakeCallbackHandler for MakeByteCounter {
//!     type RequestHandler = ByteCounter;
//!     type ResponseHandler = ByteCounter;
//!
//!     fn make_handler(
//!         &self,
//!         _request: &request::Parts,
//!     ) -> (Self::RequestHandler, Self::ResponseHandler) {
//!         (ByteCounter::default(), ByteCounter::default())
//!     }
//! }
//!
//! let _layer = CallbackLayer::new(MakeByteCounter);
//! ```
//!
//! # Body type change
//!
//! The wrapped [`Callback`] service hands the inner service a
//! `Request<CallbackBody<B, M::RequestHandler, RequestDirection>>` rather
//! than the original `Request<B>`. For body-polymorphic inner services
//! (e.g. `axum::Router` or generic `tower` services), this is transparent.
//!
//! Monomorphic inner services that require a specific body type — for
//! example `tonic::transport::Channel`, which expects `tonic::body::Body` —
//! must rebox the wrapped body at the call site:
//!
//! ```ignore
//! let wrapped: CallbackBody<_, _, RequestDirection> = /* received */;
//! let reboxed = tonic::body::Body::new(wrapped);
//! ```
//!
//! [`Callback`]: self::Callback

use http::HeaderMap;
use http::request;
use http::response;

mod body;
mod future;
mod layer;
mod service;

pub use self::body::CallbackBody;
pub use self::body::RequestDirection;
pub use self::body::ResponseDirection;
pub use self::future::ResponseFuture;
pub use self::layer::CallbackLayer;
pub use self::service::Callback;

/// Factory for per-request callback handler pairs.
///
/// A single [`MakeCallbackHandler`] implementation produces, for each
/// inbound request, one [`RequestHandler`] (observes the request body)
/// and one [`ResponseHandler`] (observes the response and its body).
pub trait MakeCallbackHandler {
    /// Handler invoked while the request body is polled by the inner
    /// service.
    type RequestHandler: RequestHandler;
    /// Handler invoked when the response materializes and while its body
    /// is polled.
    type ResponseHandler: ResponseHandler;

    /// Build the handler pair for a single request.
    fn make_handler(
        &self,
        request: &request::Parts,
    ) -> (Self::RequestHandler, Self::ResponseHandler);
}

/// Observes the request body as it is polled by the inner service.
///
/// All methods default to no-ops, so implementors only override the
/// events they care about. The unit type `()` has a blanket impl with
/// every method a no-op; use `type RequestHandler = ();` when only the
/// response side is interesting.
pub trait RequestHandler {
    /// Called once per data frame yielded by the request body.
    fn on_body_chunk<B>(&mut self, _chunk: &B)
    where
        B: bytes::Buf,
    {
        // do nothing
    }

    /// Called at most once when the request body stream ends.
    ///
    /// `trailers` is `Some` if the final frame was a trailers frame,
    /// otherwise `None`.
    fn on_end_of_stream(&mut self, _trailers: Option<&HeaderMap>) {
        // do nothing
    }

    /// Called when polling the request body yields an error.
    fn on_body_error<E>(&mut self, _error: &E)
    where
        E: std::fmt::Display + 'static,
    {
        // do nothing
    }
}

impl RequestHandler for () {}

/// Observes the response as seen by the caller: the response parts, the
/// response body, and the service-level error that occurs if the inner
/// service's future resolves to `Err` before any response is produced.
///
/// Body-level methods default to no-ops.
pub trait ResponseHandler {
    /// Called exactly once when the inner service produces a response.
    fn on_response(&mut self, response: &response::Parts);

    /// Called when the inner service's future resolves to `Err` (no
    /// response is produced). Response body errors are reported
    /// separately through [`Self::on_body_error`].
    fn on_service_error<E>(&mut self, error: &E)
    where
        E: std::fmt::Display + 'static;

    /// Called once per data frame yielded by the response body.
    fn on_body_chunk<B>(&mut self, _chunk: &B)
    where
        B: bytes::Buf,
    {
        // do nothing
    }

    /// Called at most once when the response body stream ends.
    fn on_end_of_stream(&mut self, _trailers: Option<&HeaderMap>) {
        // do nothing
    }

    /// Called when polling the response body yields an error.
    fn on_body_error<E>(&mut self, _error: &E)
    where
        E: std::fmt::Display + 'static,
    {
        // do nothing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Buf;
    use bytes::Bytes;
    use futures::stream;
    use http::Request;
    use http::Response;
    use http_body::Body;
    use http_body_util::BodyExt;
    use http_body_util::Full;
    use http_body_util::StreamBody;
    use std::convert::Infallible;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tower::ServiceBuilder;
    use tower::ServiceExt;

    /// Events recorded by a test handler pair. We share one `Arc<Mutex<_>>`
    /// between the request and response handlers so the test can assert on
    /// the complete, ordered event log.
    #[derive(Debug, Default, PartialEq, Eq)]
    struct Events {
        request_chunks: Vec<Vec<u8>>,
        request_end_trailers: Vec<Option<HeaderMap>>,
        request_body_errors: Vec<String>,
        response_seen: u32,
        response_chunks: Vec<Vec<u8>>,
        response_end_trailers: Vec<Option<HeaderMap>>,
        response_body_errors: Vec<String>,
        response_service_errors: Vec<String>,
    }

    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Events>>);

    struct ReqH(Arc<Mutex<Events>>);
    struct RespH(Arc<Mutex<Events>>);

    impl RequestHandler for ReqH {
        fn on_body_chunk<B: Buf>(&mut self, chunk: &B) {
            self.0
                .lock()
                .unwrap()
                .request_chunks
                .push(chunk.chunk().to_vec());
        }
        fn on_end_of_stream(&mut self, trailers: Option<&HeaderMap>) {
            self.0
                .lock()
                .unwrap()
                .request_end_trailers
                .push(trailers.cloned());
        }
        fn on_body_error<E: std::fmt::Display + 'static>(&mut self, error: &E) {
            self.0
                .lock()
                .unwrap()
                .request_body_errors
                .push(error.to_string());
        }
    }

    impl ResponseHandler for RespH {
        fn on_response(&mut self, _parts: &response::Parts) {
            self.0.lock().unwrap().response_seen += 1;
        }
        fn on_service_error<E: std::fmt::Display + 'static>(&mut self, error: &E) {
            self.0
                .lock()
                .unwrap()
                .response_service_errors
                .push(error.to_string());
        }
        fn on_body_chunk<B: Buf>(&mut self, chunk: &B) {
            self.0
                .lock()
                .unwrap()
                .response_chunks
                .push(chunk.chunk().to_vec());
        }
        fn on_end_of_stream(&mut self, trailers: Option<&HeaderMap>) {
            self.0
                .lock()
                .unwrap()
                .response_end_trailers
                .push(trailers.cloned());
        }
        fn on_body_error<E: std::fmt::Display + 'static>(&mut self, error: &E) {
            self.0
                .lock()
                .unwrap()
                .response_body_errors
                .push(error.to_string());
        }
    }

    impl MakeCallbackHandler for Recorder {
        type RequestHandler = ReqH;
        type ResponseHandler = RespH;

        fn make_handler(
            &self,
            _request: &request::Parts,
        ) -> (Self::RequestHandler, Self::ResponseHandler) {
            (ReqH(self.0.clone()), RespH(self.0.clone()))
        }
    }

    /// Drives the request body to completion so the request handler's
    /// events fire. In a real server, hyper does this implicitly; in
    /// tests we have to poll the body ourselves.
    async fn drain<B: Body + Unpin>(body: B) -> Result<(), B::Error> {
        let collected = body.collect().await?;
        let _ = collected.to_bytes();
        Ok(())
    }

    #[tokio::test]
    async fn observes_request_chunks_and_clean_end() {
        let recorder = Recorder::default();
        let events = recorder.0.clone();

        let inner = tower::service_fn(
            |req: Request<CallbackBody<Full<Bytes>, ReqH, RequestDirection>>| async move {
                drain(req.into_body()).await.unwrap();
                Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(b"ok"))))
            },
        );
        let svc = ServiceBuilder::new()
            .layer(CallbackLayer::new(recorder))
            .service(inner);

        let request = Request::new(Full::new(Bytes::from_static(b"hello world")));
        let response = svc.oneshot(request).await.unwrap();
        drain(response.into_body()).await.unwrap();

        let events = events.lock().unwrap();
        assert_eq!(events.request_chunks, vec![b"hello world".to_vec()]);
        assert_eq!(events.request_end_trailers, vec![None]);
        assert!(events.request_body_errors.is_empty());
        // Regression guard on the response side.
        assert_eq!(events.response_seen, 1);
        assert_eq!(events.response_chunks, vec![b"ok".to_vec()]);
        assert_eq!(events.response_end_trailers, vec![None]);
        assert!(events.response_body_errors.is_empty());
        assert!(events.response_service_errors.is_empty());
    }

    #[tokio::test]
    async fn observes_request_trailers_on_end() {
        let recorder = Recorder::default();
        let events = recorder.0.clone();

        let mut trailers = HeaderMap::new();
        trailers.insert("x-req-trailer", "abc".parse().unwrap());
        let frames: Vec<Result<http_body::Frame<Bytes>, Infallible>> = vec![
            Ok(http_body::Frame::data(Bytes::from_static(b"chunk-1"))),
            Ok(http_body::Frame::data(Bytes::from_static(b"chunk-2"))),
            Ok(http_body::Frame::trailers(trailers.clone())),
        ];
        let body = StreamBody::new(stream::iter(frames));

        let inner = tower::service_fn(
            |req: Request<CallbackBody<StreamBody<_>, ReqH, RequestDirection>>| async move {
                drain(req.into_body()).await.unwrap();
                Ok::<_, Infallible>(Response::new(Full::new(Bytes::new())))
            },
        );
        let svc = ServiceBuilder::new()
            .layer(CallbackLayer::new(recorder))
            .service(inner);

        let response = svc.oneshot(Request::new(body)).await.unwrap();
        drain(response.into_body()).await.unwrap();

        let events = events.lock().unwrap();
        assert_eq!(
            events.request_chunks,
            vec![b"chunk-1".to_vec(), b"chunk-2".to_vec()]
        );
        assert_eq!(events.request_end_trailers.len(), 1);
        assert_eq!(events.request_end_trailers[0].as_ref(), Some(&trailers));
        assert!(events.request_body_errors.is_empty());
    }

    #[tokio::test]
    async fn observes_request_body_error() {
        #[derive(Debug)]
        struct BodyErr;
        impl std::fmt::Display for BodyErr {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("boom")
            }
        }
        impl std::error::Error for BodyErr {}

        let recorder = Recorder::default();
        let events = recorder.0.clone();

        let frames: Vec<Result<http_body::Frame<Bytes>, BodyErr>> = vec![
            Ok(http_body::Frame::data(Bytes::from_static(b"partial"))),
            Err(BodyErr),
        ];
        let body = StreamBody::new(stream::iter(frames));

        let inner = tower::service_fn(
            |req: Request<CallbackBody<StreamBody<_>, ReqH, RequestDirection>>| async move {
                // Ignore the error; we just want to trigger it.
                let _ = drain(req.into_body()).await;
                Ok::<_, Infallible>(Response::new(Full::new(Bytes::new())))
            },
        );
        let svc = ServiceBuilder::new()
            .layer(CallbackLayer::new(recorder))
            .service(inner);

        let response = svc.oneshot(Request::new(body)).await.unwrap();
        drain(response.into_body()).await.unwrap();

        let events = events.lock().unwrap();
        assert_eq!(events.request_chunks, vec![b"partial".to_vec()]);
        assert_eq!(events.request_body_errors, vec!["boom".to_string()]);
        // An error terminates the stream; no clean end-of-stream fires.
        assert!(events.request_end_trailers.is_empty());
    }

    /// Compile-time and runtime check that `type RequestHandler = ();`
    /// works, is zero-cost in the ordinary sense (no observable side
    /// effects), and leaves the response side fully functional.
    #[tokio::test]
    async fn unit_request_handler_is_noop() {
        #[derive(Clone)]
        struct MakeResponseOnly(Arc<Mutex<u32>>);

        struct CountResp(Arc<Mutex<u32>>);
        impl ResponseHandler for CountResp {
            fn on_response(&mut self, _parts: &response::Parts) {
                *self.0.lock().unwrap() += 1;
            }
            fn on_service_error<E: std::fmt::Display + 'static>(&mut self, _error: &E) {}
        }

        impl MakeCallbackHandler for MakeResponseOnly {
            type RequestHandler = ();
            type ResponseHandler = CountResp;

            fn make_handler(
                &self,
                _request: &request::Parts,
            ) -> (Self::RequestHandler, Self::ResponseHandler) {
                ((), CountResp(self.0.clone()))
            }
        }

        let counter = Arc::new(Mutex::new(0));
        let make = MakeResponseOnly(counter.clone());

        let inner = tower::service_fn(
            |req: Request<CallbackBody<Full<Bytes>, (), RequestDirection>>| async move {
                drain(req.into_body()).await.unwrap();
                Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(b"hi"))))
            },
        );
        let svc = ServiceBuilder::new()
            .layer(CallbackLayer::new(make))
            .service(inner);

        let response = svc
            .oneshot(Request::new(Full::new(Bytes::from_static(b"ping"))))
            .await
            .unwrap();
        drain(response.into_body()).await.unwrap();

        assert_eq!(*counter.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn observes_response_trailers_on_end() {
        let recorder = Recorder::default();
        let events = recorder.0.clone();

        let mut trailers = HeaderMap::new();
        trailers.insert("x-resp-trailer", "xyz".parse().unwrap());
        let frames: Vec<Result<http_body::Frame<Bytes>, Infallible>> = vec![
            Ok(http_body::Frame::data(Bytes::from_static(b"part-1"))),
            Ok(http_body::Frame::data(Bytes::from_static(b"part-2"))),
            Ok(http_body::Frame::trailers(trailers.clone())),
        ];
        // `StreamBody` isn't `Clone` and `service_fn` takes an `Fn`, so we
        // smuggle the single-use body through a `Mutex<Option<_>>`.
        let body_slot = Arc::new(Mutex::new(Some(StreamBody::new(stream::iter(frames)))));

        let inner = tower::service_fn({
            let body_slot = body_slot.clone();
            move |req: Request<CallbackBody<Full<Bytes>, ReqH, RequestDirection>>| {
                let body = body_slot.lock().unwrap().take().expect("called once");
                async move {
                    drain(req.into_body()).await.unwrap();
                    Ok::<_, Infallible>(Response::new(body))
                }
            }
        });
        let svc = ServiceBuilder::new()
            .layer(CallbackLayer::new(recorder))
            .service(inner);

        let response = svc
            .oneshot(Request::new(Full::new(Bytes::from_static(b"ping"))))
            .await
            .unwrap();
        drain(response.into_body()).await.unwrap();

        let events = events.lock().unwrap();
        assert_eq!(events.response_seen, 1);
        assert_eq!(
            events.response_chunks,
            vec![b"part-1".to_vec(), b"part-2".to_vec()]
        );
        assert_eq!(events.response_end_trailers.len(), 1);
        assert_eq!(events.response_end_trailers[0].as_ref(), Some(&trailers));
        assert!(events.response_body_errors.is_empty());
        assert!(events.response_service_errors.is_empty());
    }

    #[tokio::test]
    async fn observes_response_body_error() {
        #[derive(Debug)]
        struct BodyErr;
        impl std::fmt::Display for BodyErr {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("body-boom")
            }
        }
        impl std::error::Error for BodyErr {}

        let recorder = Recorder::default();
        let events = recorder.0.clone();

        let inner = tower::service_fn(
            |req: Request<CallbackBody<Full<Bytes>, ReqH, RequestDirection>>| async move {
                drain(req.into_body()).await.unwrap();
                let frames: Vec<Result<http_body::Frame<Bytes>, BodyErr>> = vec![
                    Ok(http_body::Frame::data(Bytes::from_static(b"partial"))),
                    Err(BodyErr),
                ];
                Ok::<_, Infallible>(Response::new(StreamBody::new(stream::iter(frames))))
            },
        );
        let svc = ServiceBuilder::new()
            .layer(CallbackLayer::new(recorder))
            .service(inner);

        let response = svc
            .oneshot(Request::new(Full::new(Bytes::new())))
            .await
            .unwrap();
        // Drain but ignore the body error; we only care about the callback.
        let _ = drain(response.into_body()).await;

        let events = events.lock().unwrap();
        assert_eq!(events.response_seen, 1);
        assert_eq!(events.response_chunks, vec![b"partial".to_vec()]);
        assert_eq!(events.response_body_errors, vec!["body-boom".to_string()]);
        assert!(events.response_service_errors.is_empty());
        // An error terminates the stream; no clean end-of-stream fires.
        assert!(events.response_end_trailers.is_empty());
    }

    #[tokio::test]
    async fn observes_service_error() {
        #[derive(Debug)]
        struct SvcErr;
        impl std::fmt::Display for SvcErr {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("svc-boom")
            }
        }
        impl std::error::Error for SvcErr {}

        let recorder = Recorder::default();
        let events = recorder.0.clone();

        let inner = tower::service_fn(
            |_req: Request<CallbackBody<Full<Bytes>, ReqH, RequestDirection>>| async move {
                Err::<Response<Full<Bytes>>, _>(SvcErr)
            },
        );
        let svc = ServiceBuilder::new()
            .layer(CallbackLayer::new(recorder))
            .service(inner);

        let result = svc
            .oneshot(Request::new(Full::new(Bytes::from_static(b"ping"))))
            .await;
        let err = match result {
            Ok(_) => panic!("expected service error"),
            Err(err) => err,
        };
        assert_eq!(err.to_string(), "svc-boom");

        let events = events.lock().unwrap();
        // The response itself never materialized.
        assert_eq!(events.response_seen, 0);
        assert!(events.response_chunks.is_empty());
        assert!(events.response_end_trailers.is_empty());
        assert!(events.response_body_errors.is_empty());
        // Service error routed to the response handler.
        assert_eq!(events.response_service_errors, vec!["svc-boom".to_string()]);
    }
}
