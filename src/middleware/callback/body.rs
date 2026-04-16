// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use super::RequestHandler;
use super::ResponseHandler;
use http_body::Body;
use http_body::Frame;
use pin_project_lite::pin_project;
use std::fmt;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use std::task::ready;

pin_project! {
    /// Response body for [`Callback`].
    ///
    /// [`Callback`]: super::Callback
    pub struct ResponseBody<B, ResponseHandler> {
        #[pin]
        pub(crate) inner: B,
        pub(crate) handler: ResponseHandler,
    }
}

impl<B, ResponseHandlerT> Body for ResponseBody<B, ResponseHandlerT>
where
    B: Body,
    B::Error: fmt::Display + 'static,
    ResponseHandlerT: ResponseHandler,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let this = self.project();
        let result = ready!(this.inner.poll_frame(cx));

        match result {
            Some(Ok(frame)) => {
                let frame = match frame.into_data() {
                    Ok(chunk) => {
                        this.handler.on_body_chunk(&chunk);
                        Frame::data(chunk)
                    }
                    Err(frame) => frame,
                };

                let frame = match frame.into_trailers() {
                    Ok(trailers) => {
                        this.handler.on_end_of_stream(Some(&trailers));
                        Frame::trailers(trailers)
                    }
                    Err(frame) => frame,
                };

                Poll::Ready(Some(Ok(frame)))
            }
            Some(Err(err)) => {
                this.handler.on_error(&err);

                Poll::Ready(Some(Err(err)))
            }
            None => {
                this.handler.on_end_of_stream(None);

                Poll::Ready(None)
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

pin_project! {
    /// Request body for [`Callback`].
    ///
    /// Wraps the inbound request body so that the configured
    /// [`RequestHandler`] observes every frame as the inner service
    /// polls it. Frames themselves are forwarded unchanged.
    ///
    /// [`Callback`]: super::Callback
    pub struct RequestBody<B, RequestHandler> {
        #[pin]
        pub(crate) inner: B,
        pub(crate) handler: RequestHandler,
        // Ensures `on_request_end_of_stream` fires at most once. A body
        // that emits a trailers frame and then `Poll::Ready(None)` would
        // otherwise trigger two end-of-stream callbacks.
        pub(crate) ended: bool,
    }
}

impl<B, RequestHandlerT> Body for RequestBody<B, RequestHandlerT>
where
    B: Body,
    B::Error: fmt::Display + 'static,
    RequestHandlerT: RequestHandler,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let this = self.project();
        let result = ready!(this.inner.poll_frame(cx));

        match result {
            Some(Ok(frame)) => {
                let frame = match frame.into_data() {
                    Ok(chunk) => {
                        this.handler.on_request_chunk(&chunk);
                        Frame::data(chunk)
                    }
                    Err(frame) => frame,
                };

                let frame = match frame.into_trailers() {
                    Ok(trailers) => {
                        if !*this.ended {
                            this.handler.on_request_end_of_stream(Some(&trailers));
                            *this.ended = true;
                        }
                        Frame::trailers(trailers)
                    }
                    Err(frame) => frame,
                };

                Poll::Ready(Some(Ok(frame)))
            }
            Some(Err(err)) => {
                this.handler.on_request_error(&err);

                Poll::Ready(Some(Err(err)))
            }
            None => {
                if !*this.ended {
                    this.handler.on_request_end_of_stream(None);
                    *this.ended = true;
                }

                Poll::Ready(None)
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}
