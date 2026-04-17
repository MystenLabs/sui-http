// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::time::Duration;

/// Types that can listen for connections.
pub trait Listener: Send + 'static {
    /// The listener's IO type.
    type Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static;

    /// The listener's address type.
    // all these bounds are necessary to add this information in a request extension
    type Addr: Clone + Send + Sync + 'static;

    /// Accept a new incoming connection to this listener.
    ///
    /// If the underlying accept call can return an error, this function must
    /// take care of logging and retrying.
    fn accept(&mut self) -> impl std::future::Future<Output = (Self::Io, Self::Addr)> + Send;

    /// Returns the local address that this listener is bound to.
    fn local_addr(&self) -> std::io::Result<Self::Addr>;
}

/// Extensions to [`Listener`].
pub trait ListenerExt: Listener + Sized {
    /// Run a mutable closure on every accepted `Io`.
    ///
    /// # Example
    ///
    /// ```
    /// use sui_http::ListenerExt;
    /// use tracing::trace;
    ///
    /// # async {
    /// let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
    ///     .await
    ///     .unwrap()
    ///     .tap_io(|tcp_stream| {
    ///         if let Err(err) = tcp_stream.set_nodelay(true) {
    ///             trace!("failed to set TCP_NODELAY on incoming connection: {err:#}");
    ///         }
    ///     });
    /// # };
    /// ```
    fn tap_io<F>(self, tap_fn: F) -> TapIo<Self, F>
    where
        F: FnMut(&mut Self::Io) + Send + 'static,
    {
        TapIo {
            listener: self,
            tap_fn,
        }
    }
}

impl<L: Listener> ListenerExt for L {}

impl Listener for tokio::net::TcpListener {
    type Io = tokio::net::TcpStream;
    type Addr = std::net::SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let mut backoff = AcceptBackoff::new();
        loop {
            match Self::accept(self).await {
                Ok(tup) => return tup,
                Err(e) => backoff.handle_accept_error(e).await,
            }
        }
    }

    #[inline]
    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Self::local_addr(self)
    }
}

#[derive(Debug)]
pub struct TcpListenerWithOptions {
    inner: tokio::net::TcpListener,
    nodelay: bool,
    keepalive: Option<Duration>,
}

impl TcpListenerWithOptions {
    pub fn new<A: std::net::ToSocketAddrs>(
        addr: A,
        nodelay: bool,
        keepalive: Option<Duration>,
    ) -> Result<Self, crate::BoxError> {
        let std_listener = std::net::TcpListener::bind(addr)?;
        std_listener.set_nonblocking(true)?;
        let listener = tokio::net::TcpListener::from_std(std_listener)?;

        Ok(Self::from_listener(listener, nodelay, keepalive))
    }

    /// Creates a new `TcpIncoming` from an existing `tokio::net::TcpListener`.
    pub fn from_listener(
        listener: tokio::net::TcpListener,
        nodelay: bool,
        keepalive: Option<Duration>,
    ) -> Self {
        Self {
            inner: listener,
            nodelay,
            keepalive,
        }
    }

    // Consistent with hyper-0.14, this function does not return an error.
    fn set_accepted_socket_options(&self, stream: &tokio::net::TcpStream) {
        if self.nodelay && let Err(e) = stream.set_nodelay(true) {
            tracing::warn!("error trying to set TCP nodelay: {}", e);
        }

        if let Some(timeout) = self.keepalive {
            let sock_ref = socket2::SockRef::from(&stream);
            let sock_keepalive = socket2::TcpKeepalive::new().with_time(timeout);

            if let Err(e) = sock_ref.set_tcp_keepalive(&sock_keepalive) {
                tracing::warn!("error trying to set TCP keepalive: {}", e);
            }
        }
    }
}

impl Listener for TcpListenerWithOptions {
    type Io = tokio::net::TcpStream;
    type Addr = std::net::SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (io, addr) = Listener::accept(&mut self.inner).await;
        self.set_accepted_socket_options(&io);
        (io, addr)
    }

    #[inline]
    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Listener::local_addr(&self.inner)
    }
}

// Uncomment once we update tokio to >=1.41.0
// #[cfg(unix)]
// impl Listener for tokio::net::UnixListener {
//     type Io = tokio::net::UnixStream;
//     type Addr = std::os::unix::net::SocketAddr;

//     async fn accept(&mut self) -> (Self::Io, Self::Addr) {
//         loop {
//             match Self::accept(self).await {
//                 Ok((io, addr)) => return (io, addr.into()),
//                 Err(e) => handle_accept_error(e).await,
//             }
//         }
//     }

//     #[inline]
//     fn local_addr(&self) -> std::io::Result<Self::Addr> {
//         Self::local_addr(self).map(Into::into)
//     }
// }

/// Return type of [`ListenerExt::tap_io`].
///
/// See that method for details.
pub struct TapIo<L, F> {
    listener: L,
    tap_fn: F,
}

impl<L, F> std::fmt::Debug for TapIo<L, F>
where
    L: Listener + std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TapIo")
            .field("listener", &self.listener)
            .finish_non_exhaustive()
    }
}

impl<L, F> Listener for TapIo<L, F>
where
    L: Listener,
    F: FnMut(&mut L::Io) + Send + 'static,
{
    type Io = L::Io;
    type Addr = L::Addr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (mut io, addr) = self.listener.accept().await;
        (self.tap_fn)(&mut io);
        (io, addr)
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

/// Exponential backoff for recoverable `accept()` errors.
///
/// Certain errors (notably `EMFILE`/`ENFILE`, when the process has exhausted its
/// file descriptor limit) leave the listener in a persistently-readable state,
/// causing `accept()` to return immediately on retry. Without backoff the serve
/// loop would spin a CPU core and flood logs.
///
/// A fixed 1 second sleep (as in hyper 0.14 and still in axum today) avoids the
/// spin but delays recovery once descriptors free up. Instead we follow Go's
/// `net/http` and HashiCorp Vault: start at 5ms and double on each consecutive
/// error, capped at 1 second. Reset-on-success is implicit because a fresh
/// `AcceptBackoff` is constructed per call to `accept()`.
struct AcceptBackoff {
    next_delay: Duration,
}

impl AcceptBackoff {
    const MIN: Duration = Duration::from_millis(5);
    const MAX: Duration = Duration::from_secs(1);

    fn new() -> Self {
        Self {
            next_delay: Self::MIN,
        }
    }

    async fn handle_accept_error(&mut self, e: std::io::Error) {
        if is_connection_error(&e) {
            return;
        }

        tracing::error!(backoff = ?self.next_delay, "accept error: {e}");
        tokio::time::sleep(self.next_delay).await;
        self.next_delay = (self.next_delay * 2).min(Self::MAX);
    }
}

fn is_connection_error(e: &std::io::Error) -> bool {
    use std::io::ErrorKind;

    matches!(
        e.kind(),
        ErrorKind::ConnectionRefused
            | ErrorKind::ConnectionAborted
            | ErrorKind::ConnectionReset
            | ErrorKind::BrokenPipe
            | ErrorKind::Interrupted
            | ErrorKind::WouldBlock
            | ErrorKind::TimedOut
    )
}
