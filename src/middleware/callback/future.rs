// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use super::ResponseBody;
use super::ResponseHandler;
use http::Response;
use pin_project_lite::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

/// Errors that can happen when using the [`Callback`] middleware.
///
/// [`Callback`]: super::Callback
#[derive(Debug)]
pub enum Error<E> {
    /// The inner service produced an error.
    Inner(E),
    /// The future was polled after it had already completed.
    ///
    /// This is a bug in the caller and should be fixed.
    PolledAfterCompletion,
}

impl<E> std::fmt::Display for Error<E>
where
    E: std::fmt::Display,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Inner(e) => write!(f, "{}", e),
            Error::PolledAfterCompletion => write!(f, "future polled after completion"),
        }
    }
}

impl<E> std::error::Error for Error<E>
where
    E: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Inner(e) => Some(e),
            Error::PolledAfterCompletion => None,
        }
    }
}

pin_project! {
    /// Response future for [`Callback`].
    ///
    /// [`Callback`]: super::Callback
    pub struct ResponseFuture<F, ResponseHandler> {
        #[pin]
        pub(crate) inner: F,
        pub(crate) handler: Option<ResponseHandler>,
    }
}

impl<Fut, B, E, ResponseHandlerT> Future for ResponseFuture<Fut, ResponseHandlerT>
where
    Fut: Future<Output = Result<Response<B>, E>>,
    B: http_body::Body<Error: std::fmt::Display + 'static>,
    E: std::fmt::Display + 'static,
    ResponseHandlerT: ResponseHandler,
{
    type Output = Result<Response<ResponseBody<B, ResponseHandlerT>>, Error<E>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        let mut handler = match this.handler.take() {
            Some(handler) => handler,
            None => {
                return Poll::Ready(Err(Error::PolledAfterCompletion));
            }
        };

        let result = futures_core::ready!(this.inner.poll(cx));

        let result = match result {
            Ok(response) => {
                let (head, body) = response.into_parts();
                handler.on_response(&head);
                Ok(Response::from_parts(
                    head,
                    ResponseBody {
                        inner: body,
                        handler,
                    },
                ))
            }
            Err(error) => {
                handler.on_error(&error);
                Err(Error::Inner(error))
            }
        };

        Poll::Ready(result)
    }
}
