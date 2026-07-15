// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::future::Future;
use std::pin::Pin;
use std::pin::pin;
use std::time::Duration;

use http::Request;
use http::Response;
use hyper_util::rt::TokioExecutor;
use hyper_util::server::conn::auto;
use tracing::debug;
use tracing::trace;

use crate::ActiveConnections;
use crate::BoxError;
use crate::ConnectionId;
use crate::fuse::Fuse;

// This is moved to its own function as a way to get around
// https://github.com/rust-lang/rust/issues/102211
pub async fn serve_connection<IO, S, B, C>(
    hyper_io: IO,
    hyper_svc: S,
    builder: auto::Builder<TokioExecutor>,
    graceful_shutdown_token: tokio_util::sync::CancellationToken,
    max_connection_age: Option<Duration>,
    max_connection_age_grace: Option<Duration>,
    on_connection_close: C,
) where
    B: http_body::Body + Send + 'static,
    B::Data: Send,
    B::Error: Into<BoxError>,
    IO: hyper::rt::Read + hyper::rt::Write + Send + Unpin + 'static,
    S: hyper::service::Service<Request<hyper::body::Incoming>, Response = Response<B>> + 'static,
    S::Future: Send + 'static,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    // `serve_connection_with_upgrades` always sniffs the protocol from the
    // first bytes and silently ignores `http2_only`, so an HTTP/2-only
    // builder must use `serve_connection`, where the pinned version is
    // honored, the sniff is skipped, and anything that is not an HTTP/2
    // preface is rejected. The upgrades variant differs only in its HTTP/1
    // arm (hyper's `with_upgrades` wrapper); HTTP/2 extended CONNECT
    // behaves identically on both paths.
    if builder.is_http1_available() {
        let conn = pin!(builder.serve_connection_with_upgrades(hyper_io, hyper_svc));
        drive_connection(
            conn,
            graceful_shutdown_token,
            max_connection_age,
            max_connection_age_grace,
        )
        .await;
    } else {
        let conn = pin!(builder.serve_connection(hyper_io, hyper_svc));
        drive_connection(
            conn,
            graceful_shutdown_token,
            max_connection_age,
            max_connection_age_grace,
        )
        .await;
    }

    trace!("connection closed");
    drop(on_connection_close);
}

/// The connection future types produced by hyper-util's auto builder,
/// unified so [`drive_connection`] can drive either.
trait GracefulConnection: Future<Output = Result<(), BoxError>> {
    fn graceful_shutdown(self: Pin<&mut Self>);
}

impl<IO, S, B> GracefulConnection for auto::Connection<'_, IO, S, TokioExecutor>
where
    B: http_body::Body + Send + 'static,
    B::Data: Send,
    B::Error: Into<BoxError>,
    IO: hyper::rt::Read + hyper::rt::Write + Unpin + 'static,
    S: hyper::service::Service<Request<hyper::body::Incoming>, Response = Response<B>>,
    S::Future: Send + 'static,
    S::Error: Into<BoxError>,
{
    fn graceful_shutdown(self: Pin<&mut Self>) {
        auto::Connection::graceful_shutdown(self)
    }
}

impl<IO, S, B> GracefulConnection for auto::UpgradeableConnection<'_, IO, S, TokioExecutor>
where
    B: http_body::Body + Send + 'static,
    B::Data: Send,
    B::Error: Into<BoxError>,
    IO: hyper::rt::Read + hyper::rt::Write + Send + Unpin + 'static,
    S: hyper::service::Service<Request<hyper::body::Incoming>, Response = Response<B>>,
    S::Future: Send + 'static,
    S::Error: Into<BoxError>,
{
    fn graceful_shutdown(self: Pin<&mut Self>) {
        auto::UpgradeableConnection::graceful_shutdown(self)
    }
}

async fn drive_connection<C>(
    mut conn: Pin<&mut C>,
    graceful_shutdown_token: tokio_util::sync::CancellationToken,
    max_connection_age: Option<Duration>,
    max_connection_age_grace: Option<Duration>,
) where
    C: GracefulConnection,
{
    let mut sig = pin!(Fuse::new(graceful_shutdown_token.cancelled_owned()));

    let sleep = sleep_or_pending(max_connection_age);
    tokio::pin!(sleep);
    let mut in_grace_period = false;

    loop {
        tokio::select! {
            _ = &mut sig => {
                conn.as_mut().graceful_shutdown();
                // Bound the drain the same way an age-triggered shutdown
                // is bounded, so a wedged stream cannot keep the
                // connection alive past the grace period. If the age
                // timer already started the grace period, keep its
                // earlier deadline.
                if !in_grace_period {
                    in_grace_period = true;
                    sleep.set(sleep_or_pending(max_connection_age_grace));
                }
            }
            rv = &mut conn => {
                if let Err(err) = rv {
                    debug!("failed serving connection: {:#}", err);
                }
                break;
            },
            _ = &mut sleep  => {
                if in_grace_period {
                    // The grace period expired with streams still in
                    // flight. A stream wedged on flow control (e.g. a
                    // stalled peer that never reopens its receive window)
                    // can never complete a graceful shutdown, so dropping
                    // the connection is the only way to reclaim it.
                    debug!("max connection age grace period expired, closing connection");
                    break;
                }
                conn.as_mut().graceful_shutdown();
                in_grace_period = true;
                sleep.set(sleep_or_pending(max_connection_age_grace));
            },
        }
    }
}

async fn sleep_or_pending(wait_for: Option<Duration>) {
    match wait_for {
        Some(wait) => tokio::time::sleep(wait).await,
        None => std::future::pending().await,
    };
}

pub(crate) struct OnConnectionClose<A> {
    id: ConnectionId,
    active_connections: ActiveConnections<A>,
}

impl<A> OnConnectionClose<A> {
    pub(crate) fn new(id: ConnectionId, active_connections: ActiveConnections<A>) -> Self {
        Self {
            id,
            active_connections,
        }
    }
}

impl<A> Drop for OnConnectionClose<A> {
    fn drop(&mut self) {
        self.active_connections.write().unwrap().remove(&self.id);
    }
}
