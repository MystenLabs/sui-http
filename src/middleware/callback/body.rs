// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use super::RequestHandler;
use super::ResponseHandler;
use http_body::Body;
use http_body::Frame;
use pin_project_lite::pin_project;
use std::fmt;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use std::task::ready;

/// Direction marker indicating a request-side [`CallbackBody`], whose
/// handler must implement [`RequestHandler`].
#[derive(Debug, Default, Clone, Copy)]
pub struct RequestDirection;

/// Direction marker indicating a response-side [`CallbackBody`], whose
/// handler must implement [`ResponseHandler`].
#[derive(Debug, Default, Clone, Copy)]
pub struct ResponseDirection;

pin_project! {
    /// Body wrapper used by [`Callback`] in both the request and response
    /// positions. Every frame forwarded from the inner body is surfaced
    /// to the configured handler via `on_body_chunk`, `on_end_of_stream`,
    /// or `on_body_error`.
    ///
    /// The third type parameter `D` is a direction marker — either
    /// [`RequestDirection`] or [`ResponseDirection`] — that selects
    /// which of [`RequestHandler`] or [`ResponseHandler`] the body will
    /// dispatch through.
    ///
    /// [`Callback`]: super::Callback
    pub struct CallbackBody<B, H, D> {
        #[pin]
        pub(crate) inner: B,
        pub(crate) handler: H,
        // Ensures `on_end_of_stream` fires at most once. A body that emits
        // a trailers frame and then `Poll::Ready(None)` would otherwise
        // trigger two end-of-stream callbacks.
        pub(crate) ended: bool,
        // `fn() -> D` keeps `CallbackBody` `Send`/`Sync` regardless of
        // whether the marker type is; we never hold a `D` value.
        pub(crate) _direction: PhantomData<fn() -> D>,
    }
}

impl<B, H> Body for CallbackBody<B, H, RequestDirection>
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
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
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
                        if !*this.ended {
                            this.handler.on_end_of_stream(Some(&trailers));
                            *this.ended = true;
                        }
                        Frame::trailers(trailers)
                    }
                    Err(frame) => frame,
                };

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

impl<B, H> Body for CallbackBody<B, H, ResponseDirection>
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
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
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
                        if !*this.ended {
                            this.handler.on_end_of_stream(Some(&trailers));
                            *this.ended = true;
                        }
                        Frame::trailers(trailers)
                    }
                    Err(frame) => frame,
                };

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

// Convenience constructors used by the middleware implementation.
impl<B, H> CallbackBody<B, H, RequestDirection> {
    pub(crate) fn request(inner: B, handler: H) -> Self {
        Self {
            inner,
            handler,
            ended: false,
            _direction: PhantomData,
        }
    }
}

impl<B, H> CallbackBody<B, H, ResponseDirection> {
    pub(crate) fn response(inner: B, handler: H) -> Self {
        Self {
            inner,
            handler,
            ended: false,
            _direction: PhantomData,
        }
    }
}
