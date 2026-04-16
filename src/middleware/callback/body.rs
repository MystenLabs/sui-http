// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use super::BodyObserver;
use http_body::Body;
use http_body::Frame;
use pin_project_lite::pin_project;
use std::fmt;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use std::task::ready;

pin_project! {
    /// Body wrapper used by [`Callback`] in both the request and response
    /// positions. Every frame forwarded from the inner body is surfaced
    /// to the configured [`BodyObserver`] via `on_body_chunk`,
    /// `on_end_of_stream`, or `on_body_error`.
    ///
    /// [`Callback`]: super::Callback
    pub struct CallbackBody<B, O> {
        #[pin]
        pub(crate) inner: B,
        pub(crate) observer: O,
        // Ensures `on_end_of_stream` fires at most once. A body that emits
        // a trailers frame and then `Poll::Ready(None)` would otherwise
        // trigger two end-of-stream callbacks.
        pub(crate) ended: bool,
    }
}

impl<B, O> Body for CallbackBody<B, O>
where
    B: Body,
    B::Error: fmt::Display + 'static,
    O: BodyObserver,
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
                        this.observer.on_body_chunk(&chunk);
                        Frame::data(chunk)
                    }
                    Err(frame) => frame,
                };

                let frame = match frame.into_trailers() {
                    Ok(trailers) => {
                        if !*this.ended {
                            this.observer.on_end_of_stream(Some(&trailers));
                            *this.ended = true;
                        }
                        Frame::trailers(trailers)
                    }
                    Err(frame) => frame,
                };

                Poll::Ready(Some(Ok(frame)))
            }
            Some(Err(err)) => {
                this.observer.on_body_error(&err);

                Poll::Ready(Some(Err(err)))
            }
            None => {
                if !*this.ended {
                    this.observer.on_end_of_stream(None);
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
