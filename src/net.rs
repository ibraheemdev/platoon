use crate::core::Direction;
use crate::sys::AsRaw;
use crate::{util, Runtime};

use std::io::{self, IoSlice, IoSliceMut, Read, Write};
use std::net::{self as sys, SocketAddr};
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_io::{AsyncRead, AsyncWrite};

pub struct TcpListener {
    inner: Async<sys::TcpListener>,
}

into_std! {
    /// ...
    TcpListener => sys::TcpListener
}

from_std! { sys::TcpListener => TcpListener }

impl TcpListener {
    pub async fn bind<A>(addr: A) -> io::Result<TcpListener>
    where
        A: Into<SocketAddr>,
    {
        let sys = sys::TcpListener::bind(addr.into())?;
        sys.set_nonblocking(true)?;
        Async::new(sys, Runtime::unwrap_current()).map(|inner| Self { inner })
    }

    pub fn poll_accept(&self, cx: &mut Context) -> Poll<io::Result<(TcpStream, SocketAddr)>> {
        self.inner
            .poll_io(Direction::Read, |sys| sys.accept(), cx)
            .map(|result| {
                let (sys, addr) = result?;
                sys.set_nonblocking(true)?;
                let stream = TcpStream {
                    inner: Async::new(sys, self.inner.runtime.clone())?,
                };
                Ok((stream, addr))
            })
    }

    pub async fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let (sys, addr) = self
            .inner
            .do_io(Direction::Read, |sys| sys.accept())
            .await?;
        sys.set_nonblocking(true)?;

        let stream = TcpStream {
            inner: Async::new(sys, self.inner.runtime.clone())?,
        };

        Ok((stream, addr))
    }
}

pub struct TcpStream {
    inner: Async<sys::TcpStream>,
}

from_std! { sys::TcpStream => TcpStream }
into_std! { TcpStream => sys::TcpStream }
async_read_write! { TcpStream }

struct Async<T> {
    id: usize,
    sys: Option<T>,
    runtime: Runtime,
}

impl<T> Async<T>
where
    T: AsRaw,
{
    fn new(sys: T, runtime: Runtime) -> io::Result<Self> {
        runtime.core.insert_source(&sys).map(|id| Self {
            id,
            sys: Some(sys),
            runtime,
        })
    }

    fn sys(&self) -> &T {
        debug_assert!(self.sys.is_some());
        match &self.sys {
            Some(sys) => sys,
            None => unsafe { std::hint::unreachable_unchecked() },
        }
    }

    fn into_sys(mut self) -> io::Result<T> {
        let sys = self.sys.take().unwrap();
        self.runtime.core.remove_source(self.id)?;
        Ok(sys)
    }
}

impl<T: AsRaw> Async<T> {
    fn poll_io<R>(
        &self,
        direction: Direction,
        mut f: impl FnMut(&T) -> io::Result<R>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<R>> {
        loop {
            match f(self.sys()) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return Poll::Ready(res),
            }

            if self
                .runtime
                .core
                .poll_ready(self.id, direction, cx)?
                .is_pending()
            {
                return Poll::Pending;
            }
        }
    }

    async fn do_io<R>(
        &self,
        direction: Direction,
        mut f: impl FnMut(&T) -> io::Result<R>,
    ) -> io::Result<R> {
        loop {
            match f(self.sys()) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return res,
            }

            util::poll_fn(|cx| self.runtime.core.poll_ready(self.id, direction, cx)).await?;
        }
    }
}

impl<T> Drop for Async<T> {
    fn drop(&mut self) {
        if self.sys.is_some() {
            let _ = self.runtime.core.remove_source(self.id);
        }
    }
}

macro_rules! async_read_write {
    ($ty:ty) => {
        impl AsyncRead for $ty {
            fn poll_read(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                buf: &mut [u8],
            ) -> Poll<io::Result<usize>> {
                self.inner
                    .poll_io(Direction::Read, |mut sys| sys.read(buf), cx)
            }

            fn poll_read_vectored(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                bufs: &mut [IoSliceMut<'_>],
            ) -> Poll<io::Result<usize>> {
                self.inner
                    .poll_io(Direction::Read, |mut sys| sys.read_vectored(bufs), cx)
            }
        }

        impl AsyncWrite for $ty {
            fn poll_write(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                buf: &[u8],
            ) -> Poll<io::Result<usize>> {
                self.inner
                    .poll_io(Direction::Write, |mut sys| sys.write(buf), cx)
            }

            fn poll_write_vectored(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                bufs: &[IoSlice<'_>],
            ) -> Poll<io::Result<usize>> {
                self.inner
                    .poll_io(Direction::Write, |mut sys| sys.write_vectored(bufs), cx)
            }

            fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                self.inner
                    .poll_io(Direction::Write, |mut sys| sys.flush(), cx)
            }

            fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                self.poll_flush(cx)
            }
        }
    };
}

macro_rules! into_std {
    ($(#[$meta:meta])* $ty:ty => $sys:ty) => {
        impl $ty {
            $(#[$meta])*
            pub fn into_std(self) -> io::Result<$sys> {
                self.inner.into_sys()
            }
        }
    };
}

macro_rules! from_std {
    ($(#[$meta:meta])* $sys:ty => $ty:ty) => {
        impl $ty {
            $(#[$meta])*
            pub fn from_std(sys: $sys) -> io::Result<$ty> {
                Async::new(sys, Runtime::unwrap_current()).map(|inner| Self { inner })
            }
        }
    };
}

pub(self) use async_read_write;
pub(self) use from_std;
pub(self) use into_std;
