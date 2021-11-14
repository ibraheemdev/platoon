//! Asynchronous networking primitives.
use super::{as_raw, async_read_write, from_into_std, Async};
use crate::core::Direction;
use crate::{sys, util, Runtime};

use std::fmt;
use std::io;
use std::net::{self, SocketAddr};
use std::task::{Context, Poll};

use socket2::{Domain, Protocol};

/// A TCP socket server, listening for connections.
///
/// ```no_run
/// use platoon::net::{TcpListener, TcpStream};
///
/// async fn handle_client(stream: TcpStream) {
///     // ...
/// }
///
/// fn main() -> std::io::Result<()> {
///     platoon::block_on(async {
///         let listener = TcpListener::bind(([127, 0, 0, 1], 8080)).await?;
///
///         loop {
///             let (stream, _) = listener.accept().await?;
///             handle_client(stream).await;
///         }
///     })
/// }
/// ```
pub struct TcpListener(Async<net::TcpListener>);

impl TcpListener {
    /// Creates a new `TcpListener` bound to the specified address.
    pub async fn bind(addr: impl Into<SocketAddr>) -> io::Result<TcpListener> {
        let sys = net::TcpListener::bind(addr.into())?;
        sys.set_nonblocking(true)?;
        Async::new(sys, Runtime::unwrap_current()).map(Self)
    }

    /// Accepts a new incoming connection from this listener.
    ///
    /// This function will establish a new TCP connection asynchronously. When
    /// established, the corresponding [`TcpStream`] and the remote peer’s address
    /// will be returned.
    pub async fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let (sys, addr) = self.0.do_io(Direction::Read, |sys| sys.accept()).await?;
        sys.set_nonblocking(true)?;
        Async::new(sys, self.0.runtime.clone()).map(|s| (TcpStream(s), addr))
    }

    /// Attempts to accept a new incoming connection from this listener.
    pub fn poll_accept(&self, cx: &mut Context) -> Poll<io::Result<(TcpStream, SocketAddr)>> {
        self.0
            .poll_io(Direction::Read, |sys| sys.accept(), cx)
            .map(|result| {
                let (sys, addr) = result?;
                sys.set_nonblocking(true)?;
                Async::new(sys, self.0.runtime.clone()).map(|s| (TcpStream(s), addr))
            })
    }

    /// Returns the local address that this listener is bound to.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.0.sys().local_addr()
    }

    /// Gets the value of the `IP_TTL` option for this socket.
    ///
    /// For more information about this option, see [`set_ttl`](Self::set_ttl).
    pub fn ttl(&self) -> io::Result<u32> {
        self.0.sys().ttl()
    }

    /// Sets the value for the `IP_TTL` option on this socket.
    ///
    /// This value sets the time-to-live field that is used in every packet sent
    /// from this socket.
    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.0.sys().set_ttl(ttl)
    }
}

impl fmt::Debug for TcpListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.sys().fmt(f)
    }
}

as_raw! { TcpListener }
from_into_std! { "listener": TcpListener => std::net::TcpListener }

/// A TCP stream between a local and a remote socket.
///
/// A TCP stream can either be created by connecting to
/// an endpoint, via the [`connect`] method, or by accepting
/// a connection from a [`TcpListener`].
///
/// Reading and writing to a TcpStream is usually done using
/// the convenience methods found on the [`AsyncReadExt`] and
/// [`AsyncWriteExt`] traits.
///
/// # Examples
///
/// ```no_run
/// use platoon::net::TcpStream;
/// use futures_util::AsyncWriteExt;
///
/// fn main() -> std::io::Result<()> {
///     platoon::block_on(async {
///         // Connect to a peer
///         let mut stream = TcpStream::connect(([127, 0, 0, 1], 8080)).await?;
///
///         // Write some data.
///         stream.write_all(b"hello world!").await?;
///
///         Ok(())
///     })
/// }
/// ```
///
/// Closing the stream in the write direction can be done
/// by calling the [`close`] method.
///
/// [`AsyncReadExt`]: futures_util::AsyncReadExt
/// [`AsyncWriteExt`]: futures_util::AsyncWriteExt
/// [`close`]: futures_util::AsyncWriteExt::close
/// [`connect`]: Self::connect
pub struct TcpStream(Async<net::TcpStream>);

impl TcpStream {
    /// Opens a TCP connection to a remote host.
    pub async fn connect(addr: impl Into<SocketAddr>) -> io::Result<Self> {
        let addr = addr.into();
        let socket = sys::connect(addr.into(), Domain::for_address(addr), Some(Protocol::TCP))?;
        let stream = Async::new(net::TcpStream::from(socket), Runtime::unwrap_current())?;

        util::poll_fn(|cx| {
            stream
                .runtime
                .core
                .poll_ready(stream.id, Direction::Write, cx)
        })
        .await?;

        match stream.sys().take_error()? {
            None => Ok(Self(stream)),
            Some(err) => Err(err),
        }
    }

    /// Returns the local address that this stream is bound to.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.0.sys().local_addr()
    }

    /// Returns the socket address of the remote peer of this TCP connection.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.0.sys().peer_addr()
    }

    /// Attempts to receive data on the socket, without removing that data
    /// from the queue. On success, returns the number of bytes peeked.
    pub fn poll_peek(&self, buf: &mut [u8], cx: &mut Context<'_>) -> Poll<io::Result<usize>> {
        self.0.poll_io(Direction::Read, |sys| sys.peek(buf), cx)
    }

    /// Receives data on the socket from the remote address to which it is
    /// connected, without removing that data from the queue. On success,
    /// returns the number of bytes peeked.
    ///
    /// Successive calls return the same data. This is accomplished by passing
    /// `MSG_PEEK` as a flag to the underlying recv system call.
    pub async fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.do_io(Direction::Read, |sys| sys.peek(buf)).await
    }

    /// Gets the value of the `TCP_NODELAY` option on this socket.
    ///
    /// For more information about this option, see [`set_nodelay`](Self::set_nodelay).
    pub fn nodelay(&self) -> io::Result<bool> {
        self.0.sys().nodelay()
    }

    /// Sets the value of the `TCP_NODELAY` option on this socket.
    ///
    /// If set, this option disables the Nagle algorithm. This means that
    /// segments are always sent as soon as possible, even if there is only a
    /// small amount of data. When not set, data is buffered until there is a
    /// sufficient amount to send out, thereby avoiding the frequent sending of
    /// small packets.
    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        self.0.sys().set_nodelay(nodelay)
    }

    /// Gets the value of the `IP_TTL` option for this socket.
    ///
    /// For more information about this option, see [`set_ttl`](Self::set_ttl).
    pub fn ttl(&self) -> io::Result<u32> {
        self.0.sys().ttl()
    }

    /// Sets the value for the `IP_TTL` option on this socket.
    ///
    /// This value sets the time-to-live field that is used in every packet sent
    /// from this socket.
    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.0.sys().set_ttl(ttl)
    }

    fn _poll_close(&self, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(self.0.sys().shutdown(net::Shutdown::Write))
    }
}

as_raw! { TcpStream }
from_into_std! { "stream": TcpStream => std::net::TcpStream }
async_read_write! { TcpStream, &'a TcpStream }
