// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::time::Instant;
use futures_core::future::BoxFuture;
use http::{Request, Response};
use tower::{Layer, Service};
use tracing::{info, span, Level};

use crate::body::BoxBody;

/// A layer that adds logging to a service.
#[derive(Clone, Debug, Default)]
pub struct LoggingLayer;

impl<S> Layer<S> for LoggingLayer {
    type Service = LoggingService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        LoggingService { inner }
    }
}

/// A service that adds logging to an inner service.
#[derive(Clone, Debug)]
pub struct LoggingService<S> {
    inner: S,
}

use http_body::Body;

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for LoggingService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>> + Send + 'static,
    S::Error: Into<crate::BoxError>,
    S::Future: Send + 'static,
    ReqBody: Send + 'static,
    ResBody: Body<Data = bytes::Bytes> + Send + 'static,
    ResBody::Error: Into<crate::BoxError>,
{
    type Response = Response<BoxBody>;
    type Error = crate::BoxError;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let start = Instant::now();
        let span = span!(Level::INFO, "request", method = %req.method(), path = %req.uri().path());
        let _enter = span.enter();

        info!("started processing request");

        let future = self.inner.call(req);

        Box::pin(async move {
            let response = future.await.map_err(Into::into)?;
            let latency = start.elapsed();
            info!(
                status = %response.status().as_u16(),
                latency = ?latency,
                "finished processing request"
            );
            Ok(response.map(crate::body::boxed))
        })
    }
}