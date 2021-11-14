//! Asynchronous networking primitives.
mod tcp;
pub use tcp::{TcpListener, TcpStream};

use crate::core::Direction;
use crate::sys::AsRaw;
use crate::{util, Runtime};

use std::io;
use std::task::{Context, Poll};

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

    fn into_sys(mut self) -> io::Result<T> {
        let sys = self.sys.take().unwrap();
        self.runtime.core.remove_source(self.id)?;
        Ok(sys)
    }

    fn sys(&self) -> &T {
        debug_assert!(self.sys.is_some());
        match &self.sys {
            Some(sys) => sys,
            None => unsafe { std::hint::unreachable_unchecked() },
        }
    }
}

impl<T: AsRaw> Async<T> {
    async fn do_io<F, O>(&self, direction: Direction, mut f: F) -> io::Result<O>
    where
        F: FnMut(&T) -> io::Result<O>,
    {
        loop {
            match f(self.sys()) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return res,
            }

            util::poll_fn(|cx| self.runtime.core.poll_ready(self.id, direction, cx)).await?;
        }
    }

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
}

impl<T> Drop for Async<T> {
    fn drop(&mut self) {
        if self.sys.is_some() {
            let _ = self.runtime.core.remove_source(self.id);
        }
    }
}

macro_rules! from_into_std {
    ($name:literal: $ty:ty => $std:ty) => {
        impl $ty {
            #[doc = concat!(
                "Creates a new [`", stringify!($ty), "`] from a [`", stringify!($std), "`].\n",
                "This function performs the conversion as-is; it is up to the caller to set the ",
                $name, " in non-blocking mode",
            )]
            pub fn from_std(listener: $std) -> std::io::Result<Self> {
                Async::new(listener, Runtime::unwrap_current()).map(Self)
            }

            #[doc = concat!(
                "Converts a [`", stringify!($ty), "`] into a [`", stringify!($std), "`].\n",
                "The returned ", $name, " will have non-blocking set as `true`"
            )]
            pub fn into_std(self) -> std::io::Result<$std> {
                self.0.into_sys()
            }
        }
    };
}

macro_rules! async_read_write {
    ($($ty:ty),* | close: |$this:pat| $close:block) => {$(
        #[allow(unused_lifetimes)]
        impl<'a> futures_io::AsyncRead for $ty {
            fn poll_read(
                self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
                buf: &mut [u8],
            ) -> std::task::Poll<std::io::Result<usize>> {
                use std::io::Read;
                self.0.poll_io(Direction::Read, |mut sys| sys.read(buf), cx)
            }

            fn poll_read_vectored(
                self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
                bufs: &mut [std::io::IoSliceMut<'_>],
            ) -> std::task::Poll<std::io::Result<usize>> {
                use std::io::Read;
                self.0
                    .poll_io(Direction::Read, |mut sys| sys.read_vectored(bufs), cx)
            }
        }

        #[allow(unused_lifetimes)]
        impl<'a> futures_io::AsyncWrite for $ty {
            fn poll_write(
                self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
                buf: &[u8],
            ) -> std::task::Poll<std::io::Result<usize>> {
                use std::io::Write;
                self.0
                    .poll_io(Direction::Write, |mut sys| sys.write(buf), cx)
            }

            fn poll_write_vectored(
                self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
                bufs: &[std::io::IoSlice<'_>],
            ) -> std::task::Poll<std::io::Result<usize>> {
                use std::io::Write;
                self.0
                    .poll_io(Direction::Write, |mut sys| sys.write_vectored(bufs), cx)
            }

            fn poll_flush(
                self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                use std::io::Write;
                self.0.poll_io(Direction::Write, |mut sys| sys.flush(), cx)
            }

            fn poll_close(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                let $this = self;
                $close
            }
        }
    )*};
}

macro_rules! as_raw {
    ($ty:ty) => {
        #[cfg(unix)]
        impl std::os::unix::io::AsRawFd for $ty {
            fn as_raw_fd(&self) -> std::os::unix::io::RawFd {
                std::os::unix::io::AsRawFd::as_raw_fd(self.0.sys())
            }
        }

        #[cfg(windows)]
        impl std::os::windows::io::AsRawSocket for $ty {
            fn as_raw_socket(&self) -> std::os::windows::io::RawSocket {
                std::os::windows::io::AsRawSocket::as_raw_socket(self.0.sys())
            }
        }
    };
}

pub(self) use {as_raw, async_read_write, from_into_std};
