// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::time::Duration;

// Matches hyper's default.
const DEFAULT_HTTP1_HEADER_READ_TIMEOUT_SECS: u64 = 30;
const DEFAULT_HTTP2_KEEPALIVE_INTERVAL_SECS: u64 = 60;
const DEFAULT_HTTP2_KEEPALIVE_TIMEOUT_SECS: u64 = 20;
const DEFAULT_TCP_KEEPALIVE_SECS: u64 = 60;
// Matches hyper's post-Rapid-Reset (CVE-2023-44487) hardened default.
const DEFAULT_MAX_CONCURRENT_STREAMS: u32 = 200;
const DEFAULT_TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_MAX_PENDING_CONNECTIONS: usize = 4096;

#[derive(Debug, Clone)]
pub struct Config {
    init_stream_window_size: Option<u32>,
    init_connection_window_size: Option<u32>,
    max_concurrent_streams: Option<u32>,
    pub(crate) tcp_keepalive: Option<Duration>,
    pub(crate) tcp_nodelay: bool,
    http2_keepalive_interval: Option<Duration>,
    http2_keepalive_timeout: Option<Duration>,
    http2_adaptive_window: Option<bool>,
    http2_max_pending_accept_reset_streams: Option<usize>,
    http2_max_header_list_size: Option<u32>,
    max_frame_size: Option<u32>,
    http1_header_read_timeout: Option<Duration>,
    pub(crate) accept_http1: bool,
    enable_connect_protocol: bool,
    pub(crate) max_connection_age: Option<Duration>,
    pub(crate) max_connection_age_grace: Option<Duration>,
    pub(crate) tls_handshake_timeout: Duration,
    pub(crate) max_pending_connections: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            init_stream_window_size: None,
            init_connection_window_size: None,
            max_concurrent_streams: Some(DEFAULT_MAX_CONCURRENT_STREAMS),
            tcp_keepalive: Some(Duration::from_secs(DEFAULT_TCP_KEEPALIVE_SECS)),
            tcp_nodelay: true,
            http2_keepalive_interval: Some(Duration::from_secs(
                DEFAULT_HTTP2_KEEPALIVE_INTERVAL_SECS,
            )),
            http2_keepalive_timeout: None,
            http2_adaptive_window: None,
            http2_max_pending_accept_reset_streams: None,
            http2_max_header_list_size: None,
            max_frame_size: None,
            http1_header_read_timeout: Some(Duration::from_secs(
                DEFAULT_HTTP1_HEADER_READ_TIMEOUT_SECS,
            )),
            accept_http1: true,
            enable_connect_protocol: true,
            max_connection_age: None,
            max_connection_age_grace: None,
            tls_handshake_timeout: DEFAULT_TLS_HANDSHAKE_TIMEOUT,
            max_pending_connections: DEFAULT_MAX_PENDING_CONNECTIONS,
        }
    }
}

impl Config {
    /// Sets the [`SETTINGS_INITIAL_WINDOW_SIZE`][spec] option for HTTP2
    /// stream-level flow control.
    ///
    /// If `None` is specified, hyper's default is used (currently 1 MiB;
    /// the HTTP/2 spec default of 65,535 bytes only applies to
    /// implementations that never adjust it).
    ///
    /// [spec]: https://httpwg.org/specs/rfc9113.html#InitialWindowSize
    pub fn initial_stream_window_size(self, sz: impl Into<Option<u32>>) -> Self {
        Self {
            init_stream_window_size: sz.into(),
            ..self
        }
    }

    /// Sets the max connection-level flow control for HTTP2
    ///
    /// If `None` is specified, hyper's default is used (currently 1 MiB).
    ///
    /// Note that hyper's default equals the per-stream window, so a single
    /// stream stalled mid-upload can pin the entire connection receive
    /// window and starve every other stream on the connection. Workloads
    /// with large or streaming request bodies should consider raising this
    /// to a multiple of the stream window.
    pub fn initial_connection_window_size(self, sz: impl Into<Option<u32>>) -> Self {
        Self {
            init_connection_window_size: sz.into(),
            ..self
        }
    }

    /// Sets the [`SETTINGS_MAX_CONCURRENT_STREAMS`][spec] option for HTTP2
    /// connections.
    ///
    /// Default is 200, matching hyper's hardened default. Passing `None`
    /// removes the limit entirely and advertises unlimited concurrent
    /// streams to the peer; this makes the server vulnerable to
    /// Rapid-Reset-style resource exhaustion and should be an explicit,
    /// deliberate choice.
    ///
    /// [spec]: https://httpwg.org/specs/rfc9113.html#n-stream-concurrency
    pub fn max_concurrent_streams(self, max: impl Into<Option<u32>>) -> Self {
        Self {
            max_concurrent_streams: max.into(),
            ..self
        }
    }

    /// Sets the maximum time option in milliseconds that a connection may exist
    ///
    /// When a connection reaches its maximum age it is shut down
    /// gracefully: for HTTP/2 a GOAWAY is sent, and in-flight requests are
    /// allowed to complete. See [`Config::max_connection_age_grace`] for
    /// bounding how long that completion may take.
    ///
    /// Default is no limit (`None`).
    pub fn max_connection_age(self, max_connection_age: Duration) -> Self {
        Self {
            max_connection_age: Some(max_connection_age),
            ..self
        }
    }

    /// Sets the grace period allowed after a graceful shutdown of a
    /// connection is initiated before the connection is forcefully closed.
    ///
    /// The grace period applies however the graceful shutdown was
    /// triggered: [`Config::max_connection_age`] expiring,
    /// `ConnectionInfo::close`, or `ServerHandle::trigger_shutdown`.
    ///
    /// A graceful shutdown waits for in-flight requests to complete, but a
    /// stream that can make no progress -- for example, a response wedged
    /// behind HTTP/2 flow-control windows that a stalled or vanished peer
    /// never reopens -- would keep the connection alive forever. Once the
    /// grace period expires the connection is dropped along with any
    /// streams still in flight, following the semantics of grpc-go's
    /// `MAX_CONNECTION_AGE_GRACE`. This is the only server-side mechanism
    /// that reclaims send-stalled streams: middleware cannot do it because
    /// the response body is no longer polled once the stream stalls.
    ///
    /// Default is an unlimited grace period (`None`): the connection stays
    /// open until every in-flight request completes.
    pub fn max_connection_age_grace(self, max_connection_age_grace: Duration) -> Self {
        Self {
            max_connection_age_grace: Some(max_connection_age_grace),
            ..self
        }
    }

    /// Set whether HTTP2 Ping frames are enabled on accepted connections.
    ///
    /// If `None` is specified, HTTP2 keepalive is disabled, otherwise the duration
    /// specified will be the time interval between HTTP2 Ping frames.
    /// The timeout for receiving an acknowledgement of the keepalive ping
    /// can be set with [`Config::http2_keepalive_timeout`].
    ///
    /// Default is a 60 second interval, so dead connections are detected
    /// and reclaimed instead of lingering until the peer sends a TCP RST
    /// (which may never happen).
    pub fn http2_keepalive_interval(self, http2_keepalive_interval: Option<Duration>) -> Self {
        Self {
            http2_keepalive_interval,
            ..self
        }
    }

    /// Sets a timeout for receiving an acknowledgement of the keepalive ping.
    ///
    /// If the ping is not acknowledged within the timeout, the connection will be closed.
    /// Does nothing if http2_keep_alive_interval is disabled.
    ///
    /// Default is 20 seconds.
    pub fn http2_keepalive_timeout(self, http2_keepalive_timeout: Option<Duration>) -> Self {
        Self {
            http2_keepalive_timeout,
            ..self
        }
    }

    /// Sets whether to use an adaptive flow control. Defaults to false.
    /// Enabling this will override the limits set in http2_initial_stream_window_size and
    /// http2_initial_connection_window_size.
    ///
    /// Warning: enabling this resets both receive windows to the HTTP/2
    /// spec default of 65,535 bytes until BDP probing ramps them back up.
    /// Until then the whole connection has a single stalled stream's worth
    /// of window, so one slow reader can starve every other stream on the
    /// connection. For multiplexed streaming workloads this measurably
    /// underperforms the static defaults; prefer setting explicit window
    /// sizes instead.
    pub fn http2_adaptive_window(self, enabled: Option<bool>) -> Self {
        Self {
            http2_adaptive_window: enabled,
            ..self
        }
    }

    /// Configures the maximum number of pending reset streams allowed before a GOAWAY will be sent.
    ///
    /// This will default to whatever the default in h2 is. As of v0.3.17, it is 20.
    ///
    /// See <https://github.com/hyperium/hyper/issues/2877> for more information.
    pub fn http2_max_pending_accept_reset_streams(self, max: Option<usize>) -> Self {
        Self {
            http2_max_pending_accept_reset_streams: max,
            ..self
        }
    }

    /// Set whether TCP keepalive messages are enabled on accepted connections.
    ///
    /// If `None` is specified, keepalive is disabled, otherwise the duration
    /// specified will be the time to remain idle before sending TCP keepalive
    /// probes.
    ///
    /// Default is a 60 second idle time, so connections whose peer has
    /// vanished (crashed host, dropped NAT entry) are detected at the
    /// transport layer even for protocols without their own keepalive.
    pub fn tcp_keepalive(self, tcp_keepalive: Option<Duration>) -> Self {
        Self {
            tcp_keepalive,
            ..self
        }
    }

    /// Set the value of `TCP_NODELAY` option for accepted connections. Enabled by default.
    pub fn tcp_nodelay(self, enabled: bool) -> Self {
        Self {
            tcp_nodelay: enabled,
            ..self
        }
    }

    /// Sets the max size of received header frames.
    ///
    /// This will default to whatever the default in hyper is. As of v1.4.1, it is 16 KiB.
    pub fn http2_max_header_list_size(self, max: impl Into<Option<u32>>) -> Self {
        Self {
            http2_max_header_list_size: max.into(),
            ..self
        }
    }

    /// Sets the maximum frame size to use for HTTP2.
    ///
    /// Passing `None` will do nothing.
    ///
    /// If not set, will default from underlying transport.
    pub fn max_frame_size(self, frame_size: impl Into<Option<u32>>) -> Self {
        Self {
            max_frame_size: frame_size.into(),
            ..self
        }
    }

    /// Sets a timeout for receiving the complete header block of an HTTP/1
    /// request.
    ///
    /// If a client does not transmit its entire header block within this
    /// duration the connection is closed. This is the defense against
    /// slowloris-style attacks, where clients hold sockets open
    /// indefinitely by sending partial requests. Pass `None` to disable
    /// the timeout.
    ///
    /// Has no effect on HTTP/2 connections, whose liveness is covered by
    /// [`Config::http2_keepalive_interval`].
    ///
    /// Default is 30 seconds, matching hyper.
    pub fn http1_header_read_timeout(self, timeout: Option<Duration>) -> Self {
        Self {
            http1_header_read_timeout: timeout,
            ..self
        }
    }

    /// Allow this accepting http1 requests.
    ///
    /// Default is `true`.
    pub fn accept_http1(self, accept_http1: bool) -> Self {
        Config {
            accept_http1,
            ..self
        }
    }

    /// Sets the timeout for TLS handshakes on incoming connections.
    ///
    /// Connections that do not complete the TLS handshake within this duration are dropped.
    ///
    /// Default is 5 seconds.
    pub fn tls_handshake_timeout(self, timeout: Duration) -> Self {
        Config {
            tls_handshake_timeout: timeout,
            ..self
        }
    }

    /// Sets the maximum number of pending TLS handshakes.
    ///
    /// When this limit is reached, new incoming connections are dropped until existing
    /// handshakes complete or time out.
    ///
    /// Default is 4096.
    pub fn max_pending_connections(self, max: usize) -> Self {
        Config {
            max_pending_connections: max,
            ..self
        }
    }

    pub(crate) fn connection_builder(
        &self,
    ) -> hyper_util::server::conn::auto::Builder<hyper_util::rt::TokioExecutor> {
        let mut builder =
            hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new());

        if !self.accept_http1 {
            builder = builder.http2_only();
        }

        if self.enable_connect_protocol {
            builder.http2().enable_connect_protocol();
        }

        let http2_keepalive_timeout = self
            .http2_keepalive_timeout
            .unwrap_or_else(|| Duration::new(DEFAULT_HTTP2_KEEPALIVE_TIMEOUT_SECS, 0));

        // The timer is required for the header read timeout to take
        // effect: hyper silently disables its defaulted timeout when no
        // timer is set.
        builder
            .http1()
            .timer(hyper_util::rt::TokioTimer::new())
            .header_read_timeout(self.http1_header_read_timeout);

        builder
            .http2()
            .timer(hyper_util::rt::TokioTimer::new())
            .initial_connection_window_size(self.init_connection_window_size)
            .initial_stream_window_size(self.init_stream_window_size)
            .max_concurrent_streams(self.max_concurrent_streams)
            .keep_alive_interval(self.http2_keepalive_interval)
            .keep_alive_timeout(http2_keepalive_timeout)
            .adaptive_window(self.http2_adaptive_window.unwrap_or_default())
            .max_pending_accept_reset_streams(self.http2_max_pending_accept_reset_streams)
            .max_frame_size(self.max_frame_size);

        if let Some(max_header_list_size) = self.http2_max_header_list_size {
            builder.http2().max_header_list_size(max_header_list_size);
        }

        builder
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These defaults are security- and availability-relevant: hyper treats
    /// an explicit `None` for `max_concurrent_streams` as "remove the
    /// limit", so defaulting the field to `None` silently erased hyper's
    /// hardened 200-stream default. Pin them so they cannot regress.
    #[test]
    fn default_advertises_a_concurrent_stream_limit() {
        let config = Config::default();
        assert_eq!(config.max_concurrent_streams, Some(200));
    }

    /// Without keepalives, a connection whose peer has vanished (or whose
    /// transport is wedged) is never detected and lingers forever. Pin the
    /// defaults so they cannot silently regress to disabled.
    #[test]
    fn default_enables_keepalives() {
        let config = Config::default();
        assert_eq!(
            config.http2_keepalive_interval,
            Some(Duration::from_secs(60))
        );
        assert_eq!(config.tcp_keepalive, Some(Duration::from_secs(60)));
    }

    /// The header read timeout is the slowloris defense for HTTP/1
    /// connections; pin the default so it cannot silently regress to
    /// disabled.
    #[test]
    fn default_enables_http1_header_read_timeout() {
        let config = Config::default();
        assert_eq!(
            config.http1_header_read_timeout,
            Some(Duration::from_secs(30))
        );
    }
}
