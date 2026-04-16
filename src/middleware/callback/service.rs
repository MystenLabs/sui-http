// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use super::CallbackBody;
use super::CallbackLayer;
use super::MakeCallbackHandler;
use super::RequestDirection;
use super::ResponseDirection;
use super::ResponseFuture;
use http::Request;
use http::Response;
use std::task::Context;
use std::task::Poll;
use tower::Service;

/// Middleware that adds callbacks to a [`Service`].
///
/// See the [module docs](crate::middleware::callback) for an example.
///
/// [`Service`]: tower::Service
#[derive(Debug, Clone, Copy)]
pub struct Callback<S, M> {
    pub(crate) inner: S,
    pub(crate) make_callback_handler: M,
}

impl<S, M> Callback<S, M> {
    /// Create a new [`Callback`].
    pub fn new(inner: S, make_callback_handler: M) -> Self {
        Self {
            inner,
            make_callback_handler,
        }
    }

    /// Returns a new [`Layer`] that wraps services with a [`CallbackLayer`] middleware.
    ///
    /// [`Layer`]: tower::layer::Layer
    pub fn layer(make_handler: M) -> CallbackLayer<M>
    where
        M: MakeCallbackHandler,
    {
        CallbackLayer::new(make_handler)
    }

    /// Gets a reference to the underlying service.
    pub fn inner(&self) -> &S {
        &self.inner
    }

    /// Gets a mutable reference to the underlying service.
    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    /// Consumes `self`, returning the underlying service.
    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S, M, ReqBody, ResponseBodyT> Service<Request<ReqBody>> for Callback<S, M>
where
    S: Service<
            Request<CallbackBody<ReqBody, M::RequestHandler, RequestDirection>>,
            Response = Response<ResponseBodyT>,
            Error: std::fmt::Display + 'static,
        >,
    M: MakeCallbackHandler,
    ReqBody: http_body::Body<Error: std::fmt::Display + 'static>,
    ResponseBodyT: http_body::Body<Error: std::fmt::Display + 'static>,
{
    type Response = Response<CallbackBody<ResponseBodyT, M::ResponseHandler, ResponseDirection>>;
    type Error = S::Error;
    type Future = ResponseFuture<S::Future, M::ResponseHandler>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<ReqBody>) -> Self::Future {
        let (head, body) = request.into_parts();
        let (req_handler, resp_handler) = self.make_callback_handler.make_handler(&head);
        let request = Request::from_parts(head, CallbackBody::request(body, req_handler));

        ResponseFuture {
            inner: self.inner.call(request),
            handler: Some(resp_handler),
        }
    }
}
