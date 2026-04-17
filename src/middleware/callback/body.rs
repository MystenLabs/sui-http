// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use super::RequestHandler;
use super::ResponseHandler;
use http_body::Body;
use pin_project_lite::pin_project;
use std::fmt;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use std::task::ready;

pin_project! {
    /// Request body wrapper for [`Callback`].
    ///
    /// Forwards frames from the inner request body unchanged, surfacing
    /// every event to the configured [`RequestHandler`] via
    /// `on_body_chunk`, `on_end_of_stream`, or `on_body_error`.
    ///
    /// [`Callback`]: super::Callback
    pub struct RequestBody<B, H> {
        #[pin]
        pub(crate) inner: B,
        pub(crate) handler: H,
        // Ensures `on_end_of_stream` fires at most once. A body that emits
        // a trailers frame and then `Poll::Ready(None)` would otherwise
        // trigger two end-of-stream callbacks.
        pub(crate) ended: bool,
    }
}

impl<B, H> Body for RequestBody<B, H>
where
    B: Body,
    B::Error: fmt::Display + 'static,
    H: RequestHandler,
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
                if let Some(chunk) = frame.data_ref() {
                    this.handler.on_body_chunk(chunk);
                } else if let Some(trailers) = frame.trailers_ref()
                    && !*this.ended
                {
                    this.handler.on_end_of_stream(Some(trailers));
                    *this.ended = true;
                }

                Poll::Ready(Some(Ok(frame)))
            }
            Some(Err(err)) => {
                this.handler.on_body_error(&err);

                Poll::Ready(Some(Err(err)))
            }
            None => {
                if !*this.ended {
                    this.handler.on_end_of_stream(None);
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

pin_project! {
    /// Response body wrapper for [`Callback`].
    ///
    /// Forwards frames from the inner response body unchanged, surfacing
    /// every event to the configured [`ResponseHandler`] via
    /// `on_body_chunk`, `on_end_of_stream`, or `on_body_error`.
    ///
    /// [`Callback`]: super::Callback
    pub struct ResponseBody<B, H> {
        #[pin]
        pub(crate) inner: B,
        pub(crate) handler: H,
        pub(crate) ended: bool,
    }
}

impl<B, H> Body for ResponseBody<B, H>
where
    B: Body,
    B::Error: fmt::Display + 'static,
    H: ResponseHandler,
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
                if let Some(chunk) = frame.data_ref() {
                    this.handler.on_body_chunk(chunk);
                } else if let Some(trailers) = frame.trailers_ref()
                    && !*this.ended
                {
                    this.handler.on_end_of_stream(Some(trailers));
                    *this.ended = true;
                }

                Poll::Ready(Some(Ok(frame)))
            }
            Some(Err(err)) => {
                this.handler.on_body_error(&err);

                Poll::Ready(Some(Err(err)))
            }
            None => {
                if !*this.ended {
                    this.handler.on_end_of_stream(None);
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
